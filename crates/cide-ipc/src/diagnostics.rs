//! Wire types for problems: what the analysers found, who found it, and who has not answered
//! yet. (M12)
//!
//! # The one failure this shape exists to prevent
//!
//! `ui/src/sidebar/ProblemsPanel/model.ts` states it, and every decision here follows from it:
//!
//! > a problems panel has exactly one failure mode it cannot survive: showing a confident empty
//! > list when nothing has looked.
//!
//! `[]` from a language server and `[]` because none is running are opposite claims about the
//! workspace, and an array cannot tell them apart. So [`DiagnosticsSnapshot`] is a tagged union
//! with `Unavailable` as a first-class state, and `Ready { items: [] }` is the only shape that
//! means "something looked and found nothing".
//!
//! # Why a snapshot carries *both* a joined label and a per-source array
//!
//! There are up to four sources — `rust-analyzer`, `gopls`, tree-sitter, and Claude — and one
//! `kind` for the whole snapshot. With rust-analyzer still indexing while gopls has answered
//! with twelve findings, a single-source snapshot must be either `Scanning` (hiding twelve real
//! findings while saying "the analyser has not reported yet") or `Ready` (a clean-ish bill of
//! health for the Rust half of the workspace). Both are the failure above.
//!
//! So `Scanning` may carry the items that *are* known, and every arm carries
//! [`SourceReport`]s — which is also the only way the panel can render "rust-analyzer is not
//! installed" beside "gopls found nothing". Crucially this changes none of the load-bearing
//! predicates on the frontend: a `Scanning` snapshot with items is still not `checked()`, still
//! yields `null` from `statusBarCounts` (so the bar shows `✗ —`, not `✗ 0`), and still headlines
//! as `unknown`. The only thing that changes is that the panel may show rows it genuinely has,
//! beneath a headline naming who has not answered.
//!
//! # Units
//!
//! Lines and columns are 1-based, and columns are **UTF-16 code units** — LSP's own
//! `Position.character` plus one. Deliberately not converted to scalar values: the consumer is
//! CodeMirror, which is UTF-16-native, so the unconverted offset is what makes "jump to this
//! problem" land in the right place. See `cide_lsp::convert` for the full argument; it is the
//! kind of decision a later tidying pass "corrects" into a bug.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The severities an analyser can report, in LSP's own set.
///
/// All four are rendered rather than the two the status bar counts: dropping info and hint at
/// the wire boundary would mean the panel could not show a diagnostic that exists, which is the
/// same class of lie as an empty list.
///
/// IDEA's *weak warning* is this enum's `Info` is LSP's `Information` — one thing, three names.
/// A fifth variant for it would mean changing `SEVERITIES`, `SEVERITY_RANK`, `SeverityCounts`,
/// `summaryLine` and every assertion in `check-problems.mjs`, for a distinction neither
/// rust-analyzer nor gopls makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Whether a diagnostic is about the shape of the text or about its meaning.
///
/// Set by the **producer**, never inferred by the UI. "tree-sitter means syntax" happens to be
/// true today and stops being true the moment a language server reports a parse error, so the
/// per-editor `Syntax only` highlighting level reads this field rather than guessing from
/// [`Diagnostic::source`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DiagnosticKind {
    Syntax,
    Semantic,
}

/// Which analyser produced something.
///
/// # Why this stopped being an enum in M22
///
/// It was four unit variants — `rustAnalyzer`, `gopls`, `treeSitter`, `claude` — and the argument
/// for that was good: a closed set on the *report* is what lets the settings screen and the store
/// agree on which toggles exist. [`Diagnostic::source`] stayed a string, because it carries the
/// producer's own name (`clippy` arrives inside rust-analyzer's stream) and an unrecognised one
/// must survive to the panel rather than be dropped.
///
/// The set stopped being closed when a language server could arrive from a manifest. There is no
/// variant for `yaml-language-server` and there cannot be one, because cide is not compiled
/// knowing about it — and the alternative, folding every contributed server into a single
/// `Extension` variant, would put four servers behind one toggle and one *Restart* button, which
/// is the one thing this type exists to prevent.
///
/// So it is a newtype over the string the enum already serialised as. The four names below are
/// unchanged on the wire, every existing `extensions.json`, settings file and stored filter keeps
/// working, and a contributed server is its own binary name — which is also what the *Restart*
/// button needs to name and what `Reported by …` should say.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export, type = "string")]
pub struct DiagnosticSourceId(pub String);

impl DiagnosticSourceId {
    /// tree-sitter, which is cide's own parser and not a server.
    pub const TREE_SITTER: &'static str = "treeSitter";
    /// A `claude --print` one-shot inspection.
    pub const CLAUDE: &'static str = "claude";

    #[must_use]
    pub fn tree_sitter() -> Self {
        Self(Self::TREE_SITTER.to_string())
    }

    /// rust-analyzer, by the id it has always serialised as.
    #[must_use]
    pub fn rust_analyzer() -> Self {
        Self::for_server("rust-analyzer")
    }

    /// gopls, likewise.
    #[must_use]
    pub fn gopls() -> Self {
        Self::for_server("gopls")
    }

    #[must_use]
    pub fn claude() -> Self {
        Self(Self::CLAUDE.to_string())
    }

    /// The id a language server reports under.
    ///
    /// The two builtins keep the ids they have always had, because a user's saved source filters
    /// name them and a rename would silently un-hide something they hid. Everything else is its
    /// own binary name, which is what a person would call it.
    #[must_use]
    pub fn for_server(binary: &str) -> Self {
        Self(match binary {
            "rust-analyzer" => "rustAnalyzer".to_string(),
            other => other.to_string(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// What the user sees. Not derived from the id: `rustAnalyzer` is not how anyone writes it,
    /// and this string appears in "Reported by …" and in the settings rows.
    #[must_use]
    pub fn label(&self) -> String {
        match self.0.as_str() {
            "rustAnalyzer" => "rust-analyzer".to_string(),
            Self::TREE_SITTER => "tree-sitter".to_string(),
            other => other.to_string(),
        }
    }

    /// Whether this source is a language server, and so has a *Restart* to offer.
    ///
    /// A rule rather than a list. It used to be `['rustAnalyzer', 'gopls']` in the frontend's
    /// `model.ts`, which was a third place the set of servers was written down and would have gone
    /// stale the first time a manifest added one.
    #[must_use]
    pub fn is_server(&self) -> bool {
        self.0 != Self::TREE_SITTER && self.0 != Self::CLAUDE
    }
}

impl std::fmt::Display for DiagnosticSourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Diagnostic {
    /// Workspace-relative, forward slashes — the form the tree, the tab strip and the panel's
    /// group headings already use. What the row *displays*.
    pub path: String,
    /// Absolute. What `file.open` and `requestReveal` act on.
    ///
    /// Carried beside `path` rather than derived from it, because the frontend has no reliable
    /// way back: a multi-root project prefixes `rel` with a root label, and re-joining that onto
    /// a root is a second place to get the mapping wrong.
    pub abs_path: String,
    pub line: u32,
    pub column: u32,
    /// The extent of the squiggle. Exclusive, like every other end in this crate.
    pub end_line: u32,
    pub end_column: u32,
    pub severity: Severity,
    pub kind: DiagnosticKind,
    pub message: String,
    /// Who said so — `rust-analyzer`, `clippy`, `gopls`, `tree-sitter`, `claude`.
    ///
    /// **Required, not optional.** It is a filter axis, and an unnamed source cannot be toggled
    /// off. The producer always knows its own name.
    pub source: String,
    /// The producer's own code, e.g. `E0308`. Rendered dimmed after the message.
    ///
    /// `string | null` on the wire, not `code?: string`: `#[ts(optional)]` changes the emitted
    /// type without changing what serde writes, so on an outbound DTO it promises an absent
    /// field and sends a null one. See [`crate::Symbol::detail`].
    pub code: Option<String>,
    /// The file this is about changed on disk after this diagnostic was published. (M18)
    ///
    /// # Why the panel needs this, and why it is a field on the item
    ///
    /// A diagnostic carries the line number it had *when it was published*. Nothing rewrites that
    /// number when the file moves underneath it, and nothing can: the analyser is the only thing
    /// that knows where the finding is now, and until it re-reports we are holding a position
    /// that may name a different line. The user report this exists for is exactly that — *"after
    /// claude fixes some hints I still see them and clicking on it goes to some comments"*. The
    /// row was not wrong about there having been an error; it was wrong about where.
    ///
    /// Set by [`cide_core::diagnostics::DiagnosticStore::snapshot`] from the set of paths marked
    /// dirty since their last publish, and cleared the moment the source republishes that path.
    ///
    /// **Over-reporting is the safe direction and is deliberate.** A row that says "may be out of
    /// date" when it is not costs the user a glance; a row that is silently wrong costs them a
    /// jump into the wrong part of a file, which is the bug. The same asymmetry
    /// [`crate::DiagnosticsSnapshot`] is built around one level up.
    ///
    /// Not `#[ts(optional)]`, for the reason [`Self::code`] states: an outbound DTO that promises
    /// an absent field and sends `false` is a lie about the wire.
    pub stale: bool,
}

/// What one analyser is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SourceStatus {
    /// Not running, and `reason` says why in a sentence the user can act on: *"rust-analyzer is
    /// not on PATH. Install it with `rustup component add rust-analyzer`."*
    ///
    /// The prose is built in Rust, where the fact is known — the same call
    /// `cmd::file::not_connected` makes.
    Unavailable { reason: String },
    /// Started, and has not finished its first pass. `detail` is the progress line, because
    /// "Waiting for rust-analyzer" with nothing after it for two minutes is indistinguishable
    /// from a hang. `percentage` is the most recent `$/progress` report's number when the
    /// server sent one (rust-analyzer's indexing phases do, its `cargo check` does not) —
    /// `None` means "busy, length unknown", which the panel draws as an indeterminate bar
    /// rather than a bar frozen at zero.
    Scanning {
        detail: String,
        percentage: Option<u8>,
    },
    /// Answered. Note that this is reached when the last `$/progress` token ends **and** the
    /// handshake completed — never "when a diagnostic has been seen". A clean Rust workspace
    /// publishes nothing at all, and a ready-requires-a-diagnostic rule would leave it
    /// `Scanning` for ever: the mirror image of the failure this module exists to prevent.
    Ready,
}

/// One analyser's line in the panel's source list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceReport {
    pub id: DiagnosticSourceId,
    /// [`DiagnosticSourceId::label`], carried on the wire so the frontend never rebuilds it.
    pub label: String,
    pub status: SourceStatus,
    /// How many of the snapshot's items came from here.
    pub items: u32,
}

/// What the app knows about a project's problems right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DiagnosticsSnapshot {
    /// Nothing is analysing this workspace. `reason` is what both the panel body and the status
    /// bar's tooltip say.
    Unavailable {
        reason: String,
        sources: Vec<SourceReport>,
    },
    /// At least one enabled source has not answered yet.
    ///
    /// `items` is what the sources that *have* answered found — see the module docs. It is not a
    /// count and must never be rendered as one.
    Scanning {
        /// The still-scanning sources, joined for the headline.
        source: String,
        items: Vec<Diagnostic>,
        sources: Vec<SourceReport>,
    },
    /// Every enabled source answered. `items: []` is a real, reportable zero.
    Ready {
        /// The sources that answered, joined — "no problems" is only as good as who checked.
        source: String,
        items: Vec<Diagnostic>,
        sources: Vec<SourceReport>,
        /// How many items were dropped by the emit cap.
        ///
        /// Non-zero means `items` is a prefix. The header counter must report
        /// `items.len() + truncated`: saying `1000` when there are `1214` is the same quiet lie
        /// the panel exists to avoid, one layer down.
        truncated: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            path: path.into(),
            abs_path: format!("/repo/{path}"),
            line: 12,
            column: 5,
            end_line: 12,
            end_column: 9,
            severity,
            kind: DiagnosticKind::Semantic,
            message: message.into(),
            source: "rust-analyzer".into(),
            code: Some("E0308".into()),
            stale: false,
        }
    }

    #[test]
    fn a_diagnostic_survives_the_json_round_trip_under_its_wire_names() {
        let item = at("src/lib.rs", Severity::Error, "mismatched types");
        let json = serde_json::to_string(&item).expect("serialize");

        // Equality alone would pass with snake_case on both sides, and every multi-word field
        // would read `undefined` in the webview.
        for wire in [
            r#""absPath":"/repo/src/lib.rs""#,
            r#""endLine":12"#,
            r#""endColumn":9"#,
            r#""severity":"error""#,
            r#""kind":"semantic""#,
        ] {
            assert!(json.contains(wire), "{wire} missing from {json}");
        }
        assert_eq!(
            serde_json::from_str::<Diagnostic>(&json).expect("round trip"),
            item
        );
    }

    #[test]
    fn scanning_with_items_is_a_different_document_from_ready_with_the_same_items() {
        // The distinction the whole union exists for. If a partial answer serialized the same
        // as a complete one, the bar would print a count while an analyser was still indexing.
        let items = vec![at("src/a.rs", Severity::Error, "boom")];
        let partial = DiagnosticsSnapshot::Scanning {
            source: "rust-analyzer".into(),
            items: items.clone(),
            sources: Vec::new(),
        };
        let complete = DiagnosticsSnapshot::Ready {
            source: "gopls".into(),
            items,
            sources: Vec::new(),
            truncated: 0,
        };

        let partial = serde_json::to_string(&partial).expect("serialize");
        let complete = serde_json::to_string(&complete).expect("serialize");
        assert!(partial.contains(r#""kind":"scanning""#), "{partial}");
        assert!(complete.contains(r#""kind":"ready""#), "{complete}");
        assert_ne!(partial, complete);
        // And only the complete one can carry a truncation count, because only a complete
        // answer has a total to be a prefix of.
        assert!(!partial.contains("truncated"), "{partial}");
    }

    #[test]
    fn an_unavailable_snapshot_carries_no_items_field_at_all() {
        // Not `items: []`. A reader that pattern-matched on `kind` would be fine either way, but
        // one that reached for `.items?.length ?? 0` would read a confident zero.
        let json = serde_json::to_string(&DiagnosticsSnapshot::Unavailable {
            reason: "No language server is running for this project.".into(),
            sources: Vec::new(),
        })
        .expect("serialize");
        assert!(!json.contains("items"), "{json}");
    }

    #[test]
    fn every_source_id_has_a_label_that_is_not_its_variant_name() {
        // The labels are user-visible and appear in "Reported by …". Deriving them from the
        // variant would print `rustAnalyzer`, which is not how anyone writes it.
        assert_eq!(DiagnosticSourceId::rust_analyzer().label(), "rust-analyzer");
        assert_eq!(DiagnosticSourceId::gopls().label(), "gopls");
        assert_eq!(DiagnosticSourceId::tree_sitter().label(), "tree-sitter");
        assert_eq!(DiagnosticSourceId::claude().label(), "claude");
    }

    #[test]
    fn a_source_status_round_trips_with_its_reason_intact() {
        // The reason is the whole value of the `Unavailable` state — it is the sentence the user
        // reads and acts on. A tagged enum that dropped its payload would render an empty
        // explainer, which looks exactly like a panel that has nothing to say.
        let status = SourceStatus::Unavailable {
            reason: "gopls is not on PATH. Install it with `go install \
                     golang.org/x/tools/gopls@latest`."
                .into(),
        };
        let back: SourceStatus =
            serde_json::from_str(&serde_json::to_string(&status).expect("serialize"))
                .expect("round trip");
        assert_eq!(back, status);
    }

    #[test]
    fn severity_orders_worst_first() {
        // The panel sorts by this, and the derive follows declaration order. Pinned because
        // reordering the variants for tidiness would silently invert the panel's rows.
        let mut all = [
            Severity::Hint,
            Severity::Error,
            Severity::Info,
            Severity::Warning,
        ];
        all.sort();
        assert_eq!(
            all,
            [
                Severity::Error,
                Severity::Warning,
                Severity::Info,
                Severity::Hint
            ]
        );
    }
}

/// Where the declaration under the caret lives — or why cide cannot say.
///
/// # Three variants and never a rejected promise
///
/// The same rule `DiagnosticsSnapshot` is built on, one gesture over: "the analyser has not
/// finished indexing" and "there is no declaration here" are *different answers*, and a user who
/// is told the second when the first is true concludes the feature is broken. Both are worth a
/// sentence, and a rejected promise carries neither.
///
/// This is why `diagnostics_definition` returns `Self` rather than `Result<Option<_>, _>`: an
/// `Option` collapses "nothing found" and "nobody looked" into `None`, which is exactly the
/// distinction the whole diagnostics surface exists to preserve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DefinitionAnswer {
    /// A declaration, in cide's units: 1-based line, 1-based UTF-16 column.
    ///
    /// `path` is absolute, because the target is frequently a file no pane has open — and often
    /// outside the project entirely, in `~/.cargo/registry` or `$GOMODCACHE`, where nothing
    /// relative would resolve.
    Found {
        path: String,
        line: u32,
        column: u32,
        /// The target is a method declared inside an interface, so asking
        /// `textDocument/implementation` would name something concrete.
        ///
        /// Computed from the *target* file's outline rather than from the language or the
        /// caret — `cide_lang::interface_method_at` carries the whole argument, including why
        /// an interface *type name* deliberately answers `false`. The frontend uses it to
        /// redirect Go to definition exactly once, in the one case where the protocol's correct
        /// answer is not the useful one.
        interface_method: bool,
    },
    /// The server answered, and its answer was "no declaration here".
    ///
    /// Not an error and not a failure: a caret on a keyword, a comment, or a macro-generated name
    /// legitimately resolves to nothing.
    NotFound,
    /// Nobody could be asked. `reason` is shown to the user verbatim.
    Unavailable { reason: String },
}

/// What a **Ctrl+click** is about to do, decided before anything expensive runs.
///
/// # Why the gesture needs a third command at all
///
/// IDEA's Ctrl+click is two actions wearing one chord: on a *reference* it jumps to the
/// declaration, on a *declaration* it lists the usages. Deciding which is a question only the
/// language server can answer, so it is a round trip — and the Ctrl-hover underline has to make
/// the same decision, on pointer motion, without ever running the expensive half.
///
/// So the discriminator is its own answer. [`Self::Declaration`] says "the caret is already on the
/// declaration, so the click means Find usages" **without** having searched for any, which is what
/// makes the underline affordable: hovering a declaration costs one `textDocument/definition`, not
/// a whole-workspace reference search.
///
/// The hover and the click consume this same value, from the same cache entry. That is the whole
/// guarantee that the underline never promises something the click will not do — see
/// `ui/src/editor/codeIntel.ts`.
///
/// Four variants and never a rejected promise, for the reason [`DefinitionAnswer`] states one type
/// up: "still indexing" and "there is nothing here" are different answers and a rejection carries
/// neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ProbeAnswer {
    /// The caret is on a **reference**, and this is what it refers to. The click jumps here.
    ///
    /// Same units and same absoluteness as [`DefinitionAnswer::Found`], and deliberately carrying
    /// no end: `goToDefinition.ts` writes out why a definition's `range.end` may not be believed.
    Definition {
        path: String,
        line: u32,
        column: u32,
        /// Same flag, same meaning and same producer as
        /// [`DefinitionAnswer::Found::interface_method`], so Ctrl+click and Ctrl+B agree about
        /// what they are about to do. The hover reads this value too and ignores it: the
        /// underline promises "this jumps somewhere", which stays true either way.
        interface_method: bool,
    },
    /// The caret is on the **declaration itself**. The click shows usages.
    ///
    /// Carries nothing, on purpose: the position is the one the caller asked about, and echoing it
    /// back would be a second copy of a fact the caller already holds — one that could disagree.
    Declaration,
    /// The server answered, and its answer was "there is no symbol here".
    ///
    /// A keyword, a comment, a string body, punctuation. Not a failure: the hover stays silent and
    /// the click says one sentence.
    NotFound,
    /// Nobody could be asked. `reason` is shown to the user verbatim — and never by the hover.
    Unavailable { reason: String },
}

/// One place a symbol is used.
///
/// Three fields are copied from [`crate::SearchHit`] with their contracts intact, and each copy has
/// a reason:
///
/// * `rel` is named the way the search panel, the file picker and the symbol picker name a file,
///   so one file reads the same in all four surfaces.
/// * `text` plus **byte** `start`/`end` is `SearchHit`'s convention *specifically so
///   `ui/src/sidebar/SearchModel.ts::splitHighlight` can be reused verbatim*. The units trap is
///   real and silent: LSP hands out a UTF-16 column, the row wants a byte offset into a clipped
///   line, and the two agree only for ASCII. The conversion happens once, in Rust, at the point
///   the line is read.
/// * the line is clipped to a few hundred bytes for the same reason a search hit is — a minified
///   bundle has single lines of megabytes and none of them belongs on the IPC channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Usage {
    /// Absolute. A usage is regularly outside the project — `~/.cargo/registry`, `$GOMODCACHE` —
    /// and Go to definition has opened such files since M12.
    pub path: String,
    /// Root-relative, root-label-prefixed in a multi-root project. Falls back to the absolute path
    /// when the file is under no root, which is the common case for the two directories above.
    pub rel: String,
    /// 1-based.
    pub line: u32,
    /// 1-based, UTF-16 — the units a caret is placed with.
    pub column: u32,
    /// 1-based and exclusive, so the jump *selects* the identifier rather than poking at it.
    ///
    /// Believable here in a way a definition's end is not: a references reply's range is the
    /// occurrence itself, not an enclosing item, so selecting it is what makes "this is the usage
    /// I sent you to" visible.
    pub end_column: u32,
    /// The source line, without its terminator, clipped.
    ///
    /// Empty for a file that could not be read. The row still ships: a usage you cannot preview is
    /// still a usage, and dropping it would under-report the count with nothing to say why.
    pub text: String,
    /// Byte offset of the occurrence **within `text`**. See the type's docs for why bytes.
    pub start: u32,
    /// Exclusive end, same units.
    pub end: u32,
}

/// Every place a symbol is used — or why cide cannot say.
///
/// # `NotFound` and `Found { rows: [] }` are different, and the popup says different things
///
/// `textDocument/references` answers `null` for "there is no symbol at this position" and `[]` for
/// "this symbol is used nowhere". Collapsing them would tell a user who clicked a keyword that
/// their function is unused, which is the confident-empty-list failure the whole diagnostics
/// surface is built to avoid. `convert::locations` keeps them apart and this type carries the
/// distinction to the screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum UsagesAnswer {
    /// The usages, declaration excluded. Possibly empty — see the type's docs.
    Found {
        rows: Vec<Usage>,
        /// The server offered more than the cap. The popup says so rather than quietly lying
        /// about a count.
        truncated: bool,
    },
    /// There is no symbol under the caret at all.
    NotFound,
    /// Nobody could be asked, or the wait ran out. `reason` is shown verbatim.
    Unavailable { reason: String },
}
