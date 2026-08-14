//! Wire types for code structure: the outline behind Ctrl+F12, the caret's `mod › impl › fn`
//! trail, the member walk, and the project-wide symbol picker. (M12)
//!
//! # Why the outline is a three-state union and not a `Vec<Symbol>`
//!
//! The same reason `ui/src/sidebar/ProblemsPanel/model.ts` is a three-state union: an empty
//! outline because the file genuinely declares nothing is the *opposite claim* from an empty
//! outline because nothing parsed it, and a `Vec<Symbol>` cannot tell them apart — `[]` means
//! both. A `.md` file has no members and never will; a `.rs` file past the size cap has members
//! nobody looked for. So [`FileOutline::Unsupported`] and [`FileOutline::Failed`] are
//! first-class states, and `Ready { symbols: [] }` is a real, reportable zero.
//!
//! # Why there are two spans on every symbol
//!
//! [`Symbol::range`] is the whole declaration — what a fold would fold, and what the breadcrumb
//! tests the caret against. [`Symbol::selection`] is the name's own span — where the caret goes
//! when the user picks the row. Collapsing them into one puts the caret on the `p` of
//! `pub fn foo` when you navigate to it, and makes the breadcrumb lose every caret sitting
//! inside a function body, which is most of them.
//!
//! # Units
//!
//! Lines and columns are 1-based, and columns are **UTF-16 code units**, matching `RevealTarget`
//! in `ui/src/editor/revealRequest.ts` — which documents the exact bug the other choices cause:
//! a byte column lands the caret inside the wrong word on any line with a non-ASCII character
//! before the name. tree-sitter counts bytes, so the conversion happens once in `cide-lang`,
//! where the line's text is still in hand.
//!
//! # A note for the frontend: [`Symbol`] shadows a JavaScript global
//!
//! `ui/src/ipc/generated.ts` emits `export type Symbol`, and `client.ts` re-exports it with
//! `export type *`. TypeScript's own `Symbol` interface is therefore shadowed **in any module
//! that imports ours** — and only there, because the shadowing is per-import rather than
//! ambient. Checked rather than assumed: `tsc --noEmit` is clean, and `import type` brings no
//! value binding, so `Symbol.iterator` still resolves to the global in those modules too.
//!
//! Kept as `Symbol` because it is the head of a family — `SymbolKind`, `SymbolSpan`,
//! `SymbolRow`, `SymbolFrame` — and renaming one member to dodge a shadow that costs nothing
//! would make the other four read as if they belonged to something else. A frontend module that
//! wants both should alias on import: `import type { Symbol as CodeSymbol } from '@/ipc/client'`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What a symbol is.
///
/// LSP's `SymbolKind` narrowed to what Rust and Go actually declare — 17 rather than 26,
/// because a kind nothing can emit is a branch in every renderer that no fixture ever reaches.
/// The one addition is [`SymbolKind::Impl`], which LSP has no name for; see its own comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SymbolKind {
    /// Rust `mod`.
    Module,
    /// Go `package`.
    Package,
    Struct,
    Enum,
    Union,
    /// Go `interface`.
    Interface,
    /// Rust `trait`.
    Trait,
    /// A Rust `impl` block.
    ///
    /// Not an LSP kind, and it is here because it is a **container the breadcrumb has to
    /// name**: `Foo::new` and `Display::fmt for Foo` are two different answers to "where is the
    /// caret", and both are `impl` blocks in the same file. Dropping it would make the trail
    /// read `foo › new` for every one of a type's three impls.
    Impl,
    Function,
    /// A `fn` inside an `impl` or `trait`, or a Go method with a receiver.
    Method,
    Constant,
    Static,
    Variable,
    TypeAlias,
    /// Rust `macro_rules!`.
    Macro,
    /// A struct field. Emitted into an outline, never into the project index — see
    /// [`SymbolRow`].
    Field,
    /// An enum variant. Same treatment as [`SymbolKind::Field`].
    Variant,
}

/// A span, in the units the editor and every stack trace use.
///
/// See the module docs: 1-based lines, 1-based UTF-16 columns. [`Self::end_column`] is
/// **exclusive**, matching `RevealTarget::endColumn`, so a selection is
/// `[start_column, end_column)` and an empty span has `start == end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SymbolSpan {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// One declaration, with whatever it contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Symbol {
    pub kind: SymbolKind,
    /// The identifier alone: `new`, `Serve`, `MAX_FRAME`.
    ///
    /// Never qualified. The qualification lives in [`Self::container`] so a renderer can dim one
    /// and not the other, and so the project picker can match on the name the user is actually
    /// typing — see [`SymbolRow::name`] for what happens when it cannot.
    pub name: String,
    /// The rest of the signature, for a row's dimmed right-hand side: `(x: u32) -> Result<T>`
    /// for a function, `impl<T> Display for Foo<T>` for an impl, `struct` for a Go type spec.
    /// `None` when the name is the whole story.
    ///
    /// Plain `Option`, so TypeScript sees `detail: string | null` — **not** `#[ts(optional)]`.
    /// That attribute changes only the emitted type and not what serde writes, so on an
    /// outbound type it promises `detail?: string` while putting `null` on the wire. Every
    /// outbound optional in this crate is spelled this way (`HeadlessResult::session`,
    /// `SearchFrame::error`); `#[ts(optional)]` is for the inbound ones, which the frontend
    /// builds and Rust only ever reads.
    pub detail: Option<String>,
    /// The trail above this symbol, already joined: `net::server` in Rust, `*Server` in Go.
    /// `None` at the top level.
    ///
    /// Precomputed rather than derived from [`Self::children`] by the receiver, because the
    /// project index ships a **flat** list where the tree is gone — and two producers of one
    /// breadcrumb string is two spellings of it.
    pub container: Option<String>,
    /// The whole declaration, body included. What the breadcrumb tests the caret against.
    pub range: SymbolSpan,
    /// The name's own span. Where the caret lands.
    pub selection: SymbolSpan,
    pub children: Vec<Symbol>,
}

/// What is known about one file's structure.
///
/// Three states, three different claims — see the module docs for why this is not a `Vec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum FileOutline {
    /// No parser for this file — every language but Rust and Go, today.
    ///
    /// `reason` is shown verbatim, so it names the file's language rather than saying
    /// "unsupported": *"cide parses Rust and Go. This file is TypeScript."*
    Unsupported { reason: String },
    /// A parser exists and could not be run: past the size cap, not UTF-8, or the parse hit its
    /// time budget.
    ///
    /// Distinct from `Unsupported` because the honest render differs and because a bug report
    /// needs the right one of the two — "too big to outline" and "no parser for TOML" are
    /// different facts about a file that both draw an empty list.
    Failed { reason: String },
    Ready {
        /// `Rust` / `Go`, spelled as the status bar spells it.
        language: String,
        symbols: Vec<Symbol>,
        /// A cap stopped the extraction, so `symbols` is a prefix rather than the whole file.
        truncated: bool,
    },
}

/// One row of the project-wide symbol picker.
///
/// Deliberately **not** a [`crate::PickerRow`], for the reason [`crate::SearchHit`] is not one
/// either (see the M11 section of `search.rs`): a `PickerRow` carries one `text` that is
/// matched *and* drawn, and a symbol row draws four things — kind glyph, name, dimmed
/// container, dimmed file — of which exactly one is matched. Packing them into `text` would
/// make the highlight offsets meaningless and put the file path into the fuzzy score.
///
/// `Field` and `Variant` never appear here. Nobody uses go-to-symbol to find a struct field,
/// and they are a large fraction of the count in both languages — withholding them roughly
/// halves the index. They *are* in [`FileOutline`], because the structure popup wants them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SymbolRow {
    pub kind: SymbolKind,
    /// The matched string, and the only one [`Self::indices`] indexes.
    ///
    /// The bare identifier, never `Container::name`: with `text = "Server.Serve"`, typing
    /// `serve` also matches `S-e-r-v-e` inside `Server`, and the row the user wants is diluted
    /// by six characters they never typed.
    pub name: String,
    /// `string | null` on the wire — see [`Symbol::detail`] for why not `#[ts(optional)]`.
    pub container: Option<String>,
    /// Absolute. What opening the row acts on.
    pub path: String,
    /// Relative to its root, root-label-prefixed in a multi-root project — the same string the
    /// file picker and the search panel draw, so one file is named the same in all three.
    pub rel: String,
    pub line: u32,
    pub column: u32,
    /// Exclusive, so the jump can select the name rather than only placing the caret.
    pub end_column: u32,
    /// Offsets in [`Self::name`] that the query matched, for the highlight.
    ///
    /// **Chars**, exactly as [`crate::PickerRow::indices`] documents: highlight against
    /// `Array.from(name)`, not `name[i]`.
    pub indices: Vec<u32>,
}

/// One frame of the project symbol picker.
///
/// The same poll shape as [`crate::PickerFrame`], and `running` means the same thing — the walk
/// is still injecting, so ask again. A separate type only because its rows are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SymbolFrame {
    pub items: Vec<SymbolRow>,
    /// How many symbols the query matches.
    ///
    /// A **ceiling** rather than an exact figure while a re-index is folding a watcher burst
    /// in: the matcher is append-only (`cide_search::Matcher` has no `remove`), so a symbol a
    /// keystroke ago deleted is still counted here until the next rebuild. It can never be
    /// *drawn* — `items` is filtered against the authoritative store before it leaves Rust —
    /// which is the half that matters. Same honesty as [`crate::SearchFrame::total`].
    pub matched: u32,
    pub total: u32,
    pub running: bool,
    /// Echoed, so a frame arriving after the user typed on is discarded rather than painted.
    pub query: String,
    /// The index hit its whole-project cap and stopped. Counts are floors.
    pub truncated: bool,
}

/// What a project's symbol index is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SymbolIndexStatus {
    /// True between the walk starting and finishing. The picker is usable throughout; this
    /// only says whether the counts are still climbing.
    pub indexing: bool,
    /// Source files parsed — not files walked, since a `.md` is neither.
    pub files: u32,
    pub symbols: u32,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_line: u32, start_column: u32, end_line: u32, end_column: u32) -> SymbolSpan {
        SymbolSpan {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    #[test]
    fn an_outline_survives_the_json_round_trip_under_its_wire_names() {
        // Equality alone would pass with snake_case on both sides, and the frontend would then
        // read `undefined` for every multi-word field. The literals are the contract.
        let outline = FileOutline::Ready {
            language: "Rust".into(),
            symbols: vec![Symbol {
                kind: SymbolKind::Method,
                name: "new".into(),
                detail: Some("fn new() -> Self".into()),
                container: Some("impl Foo".into()),
                range: span(10, 5, 12, 6),
                selection: span(10, 12, 10, 15),
                children: Vec::new(),
            }],
            truncated: false,
        };

        let json = serde_json::to_string(&outline).expect("serialize");
        assert!(json.contains(r#""kind":"ready""#), "{json}");
        assert!(json.contains(r#""startLine":10"#), "{json}");
        assert!(json.contains(r#""endColumn":15"#), "{json}");
        assert!(json.contains(r#""kind":"method""#), "{json}");
        assert_eq!(
            serde_json::from_str::<FileOutline>(&json).expect("round trip"),
            outline
        );
    }

    #[test]
    fn an_empty_ready_outline_is_a_different_document_from_an_unsupported_one() {
        // The whole reason this is a union. If these two ever serialize to the same thing, the
        // popup cannot tell "this file declares nothing" from "nothing parsed this file".
        let checked = serde_json::to_string(&FileOutline::Ready {
            language: "Rust".into(),
            symbols: Vec::new(),
            truncated: false,
        })
        .expect("serialize");
        let unlooked = serde_json::to_string(&FileOutline::Unsupported {
            reason: "cide parses Rust and Go. This file is TOML.".into(),
        })
        .expect("serialize");

        assert_ne!(checked, unlooked);
        assert!(checked.contains(r#""symbols":[]"#), "{checked}");
        assert!(!unlooked.contains("symbols"), "{unlooked}");
    }

    #[test]
    fn an_absent_container_is_null_on_the_wire_and_the_emitted_type_says_so() {
        // Pinning the convention rather than the mechanism. `#[ts(optional)]` would emit
        // `container?: string` while serde still wrote `null`, and a frontend checking
        // `container === undefined` would then miss every top-level symbol. This crate's
        // outbound optionals are all `T | null`, and this asserts these are too.
        let top_level = Symbol {
            kind: SymbolKind::Function,
            name: "main".into(),
            detail: None,
            container: None,
            range: span(1, 1, 3, 2),
            selection: span(1, 4, 1, 8),
            children: Vec::new(),
        };
        let json = serde_json::to_string(&top_level).expect("serialize");
        assert!(json.contains(r#""container":null"#), "{json}");
        assert!(json.contains(r#""detail":null"#), "{json}");
        assert_eq!(
            serde_json::from_str::<Symbol>(&json).expect("round trip"),
            top_level
        );
    }

    #[test]
    fn nesting_is_preserved_so_a_breadcrumb_can_be_built_from_one_response() {
        let outline = FileOutline::Ready {
            language: "Rust".into(),
            symbols: vec![Symbol {
                kind: SymbolKind::Module,
                name: "net".into(),
                detail: None,
                container: None,
                range: span(1, 1, 20, 2),
                selection: span(1, 5, 1, 8),
                children: vec![Symbol {
                    kind: SymbolKind::Impl,
                    name: "impl Display for Server".into(),
                    detail: Some("impl Display for Server".into()),
                    container: Some("net".into()),
                    range: span(3, 5, 18, 6),
                    selection: span(3, 5, 3, 28),
                    children: vec![Symbol {
                        kind: SymbolKind::Method,
                        name: "fmt".into(),
                        detail: None,
                        container: Some("net::impl Display for Server".into()),
                        range: span(4, 9, 6, 10),
                        selection: span(4, 16, 4, 19),
                        children: Vec::new(),
                    }],
                }],
            }],
            truncated: false,
        };

        let back: FileOutline =
            serde_json::from_str(&serde_json::to_string(&outline).expect("serialize"))
                .expect("round trip");
        let FileOutline::Ready { symbols, .. } = &back else {
            panic!("not ready: {back:?}");
        };
        assert_eq!(symbols[0].children[0].children[0].name, "fmt");
        assert_eq!(
            symbols[0].children[0].children[0].container.as_deref(),
            Some("net::impl Display for Server")
        );
    }
}
