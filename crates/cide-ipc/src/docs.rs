//! A symbol's documentation, as a page any language server can produce. (M60)
//!
//! # Why one page shape for every language
//!
//! The request was Godot's: go to definition on `Dictionary` and read its docs. Godot has no
//! file to jump to — a native class lives in the engine — so the editor sends its documentation
//! over the wire instead, in its own BBCode. But the *shape* of the answer is not Godot's: a
//! Java server hands javadoc back as hover markdown for a JDK class the user cannot open either,
//! rust-analyzer does the same for `std`, gopls for the standard library. Every server has a
//! symbol the user cannot open and a description of it, and the description is markdown in
//! every case but one — LSP's `textDocument/hover` is markdown by contract, and Godot's BBCode
//! converts to it in a screenful of Rust (`cide_lsp::docs::godot`).
//!
//! So this is the page. A provider in `cide_lsp::docs` fills it — from hover for any server, from
//! the native-symbol payload for Godot — and the frontend draws it with the markdown preview it
//! already has, one component for every language. A second page shape per language would be
//! the panel-per-source drift `cide_ipc::SourceStatus` exists to prevent, one layer up.
//!
//! # What is deliberately a `String`
//!
//! `kind` is a word (`class`, `method`, `property`, `signal`, …) and not an enum, for the reason
//! Docker's vocabulary is carried as text: a kind a server adds must render as itself rather
//! than as blank. The frontend draws the word; nothing matches on it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One documentation page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SymbolDocs {
    /// What the page is about: `Dictionary`, `Node.add_child`, `HashMap<K, V>`.
    pub title: String,
    /// `class`, `method`, `property`, `constant`, `signal`, `enum`, `symbol` — a word, see the
    /// module note.
    pub kind: String,
    /// The declaration, as the server spells it, drawn as a code line under the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub signature: Option<String>,
    /// The body, markdown. In-page references use the fragment form `#ref/<kind>/<target>`
    /// (`#ref/method/Node.add_child`), which the page hands back to [`DocsLink::href`]-style
    /// following; everything else is an ordinary link.
    pub markdown: String,
    /// A class's members, each a short page of its own. Empty for a leaf symbol.
    #[serde(default)]
    pub members: Vec<DocsMember>,
    /// Where else to read about it — the online reference, a source file. Drawn under the title.
    #[serde(default)]
    pub links: Vec<DocsLink>,
    /// Which server answered: the registry name (`godot`, `rust-analyzer`), for the page header
    /// and for following a reference back to the same server.
    pub source: String,
    /// How to ask for this very page again — a `#ref/…` fragment — when the provider can
    /// answer one. A Godot page can (its native symbols are addressable by name); a hover page
    /// cannot, and is asked for again by position instead. See [`DocsSubject`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reference: Option<String>,
}

/// What a documentation tab is *about* — enough to ask the server again, never the page itself.
///
/// The same argument `TabKind::Docker` makes: a tab is durable and a page is not. A page for
/// `Node` is a claim about the engine the user runs today; the tab carries the question, and the
/// pane asks it on every mount. Two shapes because two providers answer two kinds of question —
/// a Godot native symbol has a stable name, a hover has only the place it was asked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DocsSubject {
    /// A `#ref/<kind>/<target>` the named server can follow (`godot`: `method`,
    /// `Node.add_child`). `ref_kind` and not `kind`, because `kind` is this enum's own tag on
    /// the wire and serde refuses a field that shadows it.
    Reference {
        source: String,
        ref_kind: String,
        target: String,
    },
    /// The symbol at a position in a file, for a server that answers by hover. `word` is what
    /// was under the caret, for the title — the hover reply names nothing.
    Position {
        #[ts(type = "string")]
        path: std::path::PathBuf,
        line: u32,
        column: u32,
        word: String,
    },
}

impl DocsSubject {
    /// The tab label a subject earns before any page has been read.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Reference { target, .. } => target.clone(),
            Self::Position { word, .. } => word.clone(),
        }
    }
}

/// Documentation for what is under the caret — or why cide cannot say.
///
/// `DefinitionAnswer`'s three-way rule, one gesture over: "the server has nothing to say about
/// this" and "nobody could be asked" are different sentences, and a rejected promise carries
/// neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DocsAnswer {
    /// A page, and the subject a tab should carry to ask for it again. Boxed because a page is
    /// a screenful and the other two arms are a sentence; the binding is the same.
    Found {
        page: Box<SymbolDocs>,
        subject: DocsSubject,
    },
    /// The server answered, and had nothing to say about what is here.
    NotFound,
    /// Nobody could be asked. `reason` is shown to the user verbatim.
    Unavailable { reason: String },
}

/// One member of a class page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocsMember {
    pub name: String,
    /// See [`SymbolDocs::kind`].
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub signature: Option<String>,
    /// Markdown; may be empty for a member the server has no words for.
    pub markdown: String,
    /// The `#ref/…` fragment that opens this member as a page of its own, when the provider
    /// can answer one (Godot can, through `textDocument/nativeSymbol`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
}

/// A link drawn under the title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocsLink {
    pub label: String,
    /// An `https://` URL, or a `#ref/…` fragment the same provider can follow.
    pub href: String,
}
