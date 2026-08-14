//! The merged view of every analyser's findings, and the one projection the UI reads.
//!
//! # Why the store lives here and not in `cide-lsp`
//!
//! It holds tree-sitter's syntax errors and Claude's inspections as well as a language server's
//! diagnostics, and `cide-lsp` must not know those exist. The type and the fold rules are plain
//! data and free functions over it — no I/O, no threads, no locks — which is what makes the
//! invariants below unit-testable without a Tauri state. `cide-app` owns the instance.
//!
//! # The rule that shapes everything: an empty list is not an answer
//!
//! `ui/src/sidebar/ProblemsPanel/model.ts` states it:
//!
//! > a problems panel has exactly one failure mode it cannot survive: showing a confident empty
//! > list when nothing has looked.
//!
//! Two places here follow from it directly, and both are easy to "simplify" into a bug:
//!
//! * [`DiagnosticStore::publish`] with an empty list **inserts an empty vec** rather than
//!   removing the key. An empty vec for `src/lib.rs` means "rust-analyzer looked and found
//!   nothing"; an absent key means "nothing has ever looked at lib.rs". `getDiagnostics` answers
//!   those two differently, and so does the panel.
//! * [`DiagnosticStore::snapshot`] reaches `Ready` only when every enabled, available source has
//!   answered. One source still scanning keeps the whole snapshot out of `Ready` — while still
//!   carrying the items the others found, because hiding twelve real findings is its own lie.
//!
//! # Where the settings filter applies, and why it is here
//!
//! Exactly once, in [`DiagnosticStore::snapshot`]. The two alternatives both lose:
//!
//! * **At the store** — filtering on the way *in* means turning "hints" back on shows nothing
//!   until the user edits a file, because rust-analyzer only republishes on change. The store is
//!   the truth; a filter is a view.
//! * **At render, in the frontend** — forbidden by `model.ts:13-16`, which derives
//!   `statusBarCounts` from the snapshot precisely so the bar and the panel "cannot drift into
//!   disagreeing about whether the workspace is clean". Two components each applying the filter
//!   is two chances to disagree, plus a second copy of the rule in TypeScript when the MCP tool
//!   is in Rust.
//!
//! The one caller that must **not** filter is `getDiagnostics`, which reads the store directly:
//! a user who hid hints made a statement about their own screen, not about what Claude should be
//! told. See [`DiagnosticStore::for_abs_path`].

use std::collections::BTreeMap;

use cide_ipc::{
    Diagnostic, DiagnosticSourceId, DiagnosticsSnapshot, InspectionSettings, SourceReport,
    SourceStatus,
};

/// How many diagnostics one snapshot may carry to the webview.
///
/// A `cargo check` on a badly broken crate emits tens of thousands, and a 252px sidebar cannot
/// show 2,000 rows. Past this the snapshot carries a prefix and a `truncated` count, and the
/// header counter reports the sum — a counter reading `1000` when there are `1214` is the same
/// quiet lie the panel exists to avoid, one layer down.
pub const EMIT_CAP: usize = 1_000;

/// One analyser's contribution.
#[derive(Debug, Clone)]
pub struct SourceState {
    pub status: SourceStatus,
    /// Keyed by **absolute** path.
    ///
    /// Absolute rather than workspace-relative because this map is also what answers
    /// `getDiagnostics{uri}`, and a `file://` URI resolves to an absolute path. The relative form
    /// the panel groups by travels on each [`Diagnostic`].
    ///
    /// `BTreeMap`, not `HashMap`: this feeds the panel, the status-bar count and a JSON payload
    /// serialized to a blocked agent, and a nondeterministic iteration order makes that payload
    /// differ byte-for-byte between runs for no reason — and the MCP test flaky with it.
    pub by_path: BTreeMap<String, Vec<Diagnostic>>,
}

impl SourceState {
    fn new() -> Self {
        Self {
            // A source that has been mentioned but has not reported is not "ready with nothing".
            status: SourceStatus::Scanning {
                detail: "starting".to_string(),
            },
            by_path: BTreeMap::new(),
        }
    }

    fn items(&self) -> impl Iterator<Item = &Diagnostic> {
        self.by_path.values().flatten()
    }
}

/// Every analyser's findings for one project.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticStore {
    sources: BTreeMap<DiagnosticSourceId, SourceState>,
}

impl DiagnosticStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what an analyser is doing. Creates the source if this is the first word from it.
    pub fn set_status(&mut self, source: DiagnosticSourceId, status: SourceStatus) {
        self.sources
            .entry(source)
            .or_insert_with(SourceState::new)
            .status = status;
    }

    /// Replace one source's list for one absolute path.
    ///
    /// `items` may be empty, and an empty vec is **inserted rather than removing the key** — see
    /// the module docs. This is `textDocument/publishDiagnostics`' own semantics: a server that
    /// has fixed the last error in a file republishes it with `[]`, and dropping the key there
    /// would make the file indistinguishable from one nothing has opened.
    pub fn publish(
        &mut self,
        source: DiagnosticSourceId,
        abs_path: String,
        items: Vec<Diagnostic>,
    ) {
        self.sources
            .entry(source)
            .or_insert_with(SourceState::new)
            .by_path
            .insert(abs_path, items);
    }

    /// Forget everything one source ever said — it exited, or was turned off.
    ///
    /// Note that turning a source *off in settings* does **not** call this: the projection omits
    /// it, and turning it back on restores what was already found without re-spending. That
    /// matters most for Claude, where re-finding costs tokens.
    pub fn clear_source(&mut self, source: DiagnosticSourceId) {
        self.sources.remove(&source);
    }

    /// Every diagnostic for one absolute path, from every source, **unfiltered**.
    ///
    /// Unfiltered deliberately, and this is the rule rather than an oversight:
    ///
    /// > Per-source toggles gate the *process* — nothing runs, so there is nothing to report, and
    /// > Claude correctly sees nothing. Per-severity toggles and the highlighting level are
    /// > display-only and never reach the MCP tool.
    ///
    /// A user who hid hints made a statement about their own screen, not about what an agent
    /// should be told.
    pub fn for_abs_path(&self, abs_path: &str) -> Vec<&Diagnostic> {
        self.sources
            .values()
            .filter_map(|state| state.by_path.get(abs_path))
            .flatten()
            .collect()
    }

    /// Has *anything* ever reported on this path?
    ///
    /// The distinction `getDiagnostics{uri}` needs: `[{uri, diagnostics: []}]` means "we looked
    /// and it is clean", `[]` means "nothing has looked at that file". Same rule as the panel's,
    /// aimed at an agent rather than a person.
    pub fn has_looked_at(&self, abs_path: &str) -> bool {
        self.sources
            .values()
            .any(|state| state.by_path.contains_key(abs_path))
    }

    /// Every path anything has reported on, unfiltered.
    pub fn known_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self
            .sources
            .values()
            .flat_map(|state| state.by_path.keys().map(String::as_str))
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }

    /// The one projection the UI reads.
    ///
    /// See the module docs for why the filter is applied here and nowhere else.
    pub fn snapshot(&self, settings: &InspectionSettings, cap: usize) -> DiagnosticsSnapshot {
        let enabled: Vec<(DiagnosticSourceId, &SourceState)> = self
            .sources
            .iter()
            .filter(|(id, _)| settings.shows_source(id.label()))
            .map(|(id, state)| (*id, state))
            .collect();

        if enabled.is_empty() {
            return DiagnosticsSnapshot::Unavailable {
                reason: no_enabled_source_reason(&self.sources, settings),
                sources: self.reports(settings, 0),
            };
        }

        // Items first, because both the `Scanning` and `Ready` arms carry them and the reports
        // need per-source counts of the *filtered* set.
        let mut items: Vec<Diagnostic> = enabled
            .iter()
            .flat_map(|(_, state)| state.items())
            .filter(|d| settings.shows_severity(d.severity) && settings.shows_source(&d.source))
            .cloned()
            .collect();
        // A total order, so the prefix a cap keeps is the *worst* problems rather than whichever
        // path sorted first. `sort_by` on (severity, path, line, column) mirrors the panel's own
        // comparator — the frontend re-sorts within a file, and this decides what survives the cap.
        items.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.column.cmp(&b.column))
                .then_with(|| a.message.cmp(&b.message))
        });

        let truncated = items.len().saturating_sub(cap) as u32;
        items.truncate(cap);
        let reports = self.reports(settings, cap);

        // A source that is `Unavailable` is not waiting for anything — it is *not running*, which
        // is an answer. Only a `Scanning` one keeps the snapshot out of `Ready`.
        let scanning: Vec<&str> = enabled
            .iter()
            .filter(|(_, s)| matches!(s.status, SourceStatus::Scanning { .. }))
            .map(|(id, _)| id.label())
            .collect();
        if !scanning.is_empty() {
            return DiagnosticsSnapshot::Scanning {
                source: join(&scanning),
                items,
                sources: reports,
            };
        }

        let ready: Vec<(DiagnosticSourceId, &SourceState)> = enabled
            .iter()
            .filter(|(_, s)| matches!(s.status, SourceStatus::Ready))
            .copied()
            .collect();
        if ready.is_empty() {
            // Everything enabled is unavailable. The reasons are the actionable part.
            return DiagnosticsSnapshot::Unavailable {
                reason: join_reasons(&enabled),
                sources: reports,
            };
        }

        DiagnosticsSnapshot::Ready {
            source: ready_label(&ready),
            items,
            sources: reports,
            truncated,
        }
    }

    fn reports(&self, settings: &InspectionSettings, cap: usize) -> Vec<SourceReport> {
        let _ = cap;
        self.sources
            .iter()
            .map(|(id, state)| SourceReport {
                id: *id,
                label: id.label().to_string(),
                status: if settings.shows_source(id.label()) {
                    state.status.clone()
                } else {
                    // A source the user switched off is not "unavailable" in the sense of broken.
                    // Saying so in its own row is what stops the panel's empty list from reading
                    // as a clean bill of health.
                    SourceStatus::Unavailable {
                        reason: "Turned off in Settings ▸ Inspections.".to_string(),
                    }
                },
                items: state
                    .items()
                    .filter(|d| settings.shows_severity(d.severity))
                    .count() as u32,
            })
            .collect()
    }
}

/// `rust-analyzer` / `rust-analyzer and gopls` / `rust-analyzer, gopls and tree-sitter`.
fn join(labels: &[&str]) -> String {
    match labels {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The label for a `Ready` snapshot — with one qualifier that is not decoration.
///
/// tree-sitter only ever sees files the user has **open**, where a language server sees the whole
/// workspace. So "tree-sitter found nothing" is a claim about a handful of buffers, and rendering
/// it as `No problems found` would be a statement about forty thousand unopened files that
/// nothing has read. When it is the only source that answered, the label says so.
fn ready_label(ready: &[(DiagnosticSourceId, &SourceState)]) -> String {
    let labels: Vec<&str> = ready.iter().map(|(id, _)| id.label()).collect();
    if labels == ["tree-sitter"] {
        return "tree-sitter (open files only)".to_string();
    }
    join(&labels)
}

fn join_reasons(sources: &[(DiagnosticSourceId, &SourceState)]) -> String {
    let reasons: Vec<String> = sources
        .iter()
        .filter_map(|(_, s)| match &s.status {
            SourceStatus::Unavailable { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    if reasons.is_empty() {
        return "No diagnostics source is running.".to_string();
    }
    reasons.join(" ")
}

/// Why nothing is enabled — which is a different sentence depending on whether anything *could*
/// have been.
fn no_enabled_source_reason(
    sources: &BTreeMap<DiagnosticSourceId, SourceState>,
    settings: &InspectionSettings,
) -> String {
    if sources.is_empty() {
        return "No language server is running for this project. Rust needs rust-analyzer and \
                Go needs gopls on PATH."
            .to_string();
    }
    let off: Vec<&str> = sources
        .keys()
        .map(|id| id.label())
        .filter(|label| !settings.shows_source(label))
        .collect();
    format!(
        "Every diagnostics source is turned off ({}). Turn one back on in \
         Settings ▸ Inspections.",
        join(&off)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{DiagnosticKind, Severity};

    /// How many items a snapshot carries, whatever arm it is.
    fn items_len(snapshot: &DiagnosticsSnapshot) -> usize {
        match snapshot {
            DiagnosticsSnapshot::Unavailable { .. } => 0,
            DiagnosticsSnapshot::Scanning { items, .. }
            | DiagnosticsSnapshot::Ready { items, .. } => items.len(),
        }
    }

    fn item(path: &str, severity: Severity, source: &str, message: &str) -> Diagnostic {
        Diagnostic {
            path: path.to_string(),
            abs_path: format!("/repo/{path}"),
            line: 1,
            column: 1,
            end_line: 1,
            end_column: 2,
            severity,
            kind: DiagnosticKind::Semantic,
            message: message.to_string(),
            source: source.to_string(),
            code: None,
        }
    }

    fn ready_store() -> DiagnosticStore {
        let mut store = DiagnosticStore::new();
        store.set_status(DiagnosticSourceId::RustAnalyzer, SourceStatus::Ready);
        store.publish(
            DiagnosticSourceId::RustAnalyzer,
            "/repo/src/a.rs".into(),
            vec![
                item("src/a.rs", Severity::Error, "rust-analyzer", "boom"),
                item("src/a.rs", Severity::Hint, "rust-analyzer", "consider"),
            ],
        );
        store
    }

    #[test]
    fn an_empty_publish_is_not_the_same_as_never_having_published() {
        // The distinction `getDiagnostics{uri}` answers differently, and the one a "tidy up empty
        // entries" pass would delete.
        let mut store = DiagnosticStore::new();
        store.set_status(DiagnosticSourceId::RustAnalyzer, SourceStatus::Ready);
        store.publish(
            DiagnosticSourceId::RustAnalyzer,
            "/repo/clean.rs".into(),
            vec![],
        );

        assert!(store.has_looked_at("/repo/clean.rs"));
        assert!(!store.has_looked_at("/repo/unopened.rs"));
        assert!(store.for_abs_path("/repo/clean.rs").is_empty());
    }

    #[test]
    fn a_republish_replaces_rather_than_appends() {
        let mut store = ready_store();
        store.publish(
            DiagnosticSourceId::RustAnalyzer,
            "/repo/src/a.rs".into(),
            vec![item(
                "src/a.rs",
                Severity::Warning,
                "rust-analyzer",
                "only me",
            )],
        );
        assert_eq!(store.for_abs_path("/repo/src/a.rs").len(), 1);
    }

    #[test]
    fn one_scanning_source_keeps_the_snapshot_out_of_ready_but_not_out_of_items() {
        // The whole reason the `Scanning` arm carries items. rust-analyzer indexing while gopls
        // has answered must not render as either "no problems" or "nothing to show".
        let mut store = ready_store();
        store.set_status(
            DiagnosticSourceId::Gopls,
            SourceStatus::Scanning {
                detail: "Loading packages".into(),
            },
        );

        let snapshot = store.snapshot(&InspectionSettings::default(), EMIT_CAP);
        let DiagnosticsSnapshot::Scanning { source, items, .. } = &snapshot else {
            panic!("{snapshot:?}");
        };
        assert_eq!(source, "gopls");
        assert_eq!(items.len(), 2, "the answered source's findings were hidden");
    }

    #[test]
    fn an_unavailable_source_does_not_hold_the_snapshot_at_scanning_for_ever() {
        // A source that is not installed has *answered* — it is not going to report. Treating it
        // as pending would leave the panel saying "waiting for gopls" on a machine that has none,
        // for the life of the session.
        let mut store = ready_store();
        store.set_status(
            DiagnosticSourceId::Gopls,
            SourceStatus::Unavailable {
                reason: "gopls is not on PATH.".into(),
            },
        );
        let snapshot = store.snapshot(&InspectionSettings::default(), EMIT_CAP);
        assert!(
            matches!(snapshot, DiagnosticsSnapshot::Ready { .. }),
            "{snapshot:?}"
        );
    }

    #[test]
    fn everything_unavailable_is_unavailable_and_names_the_reasons() {
        let mut store = DiagnosticStore::new();
        store.set_status(
            DiagnosticSourceId::RustAnalyzer,
            SourceStatus::Unavailable {
                reason: "rust-analyzer is not on PATH.".into(),
            },
        );
        let snapshot = store.snapshot(&InspectionSettings::default(), EMIT_CAP);
        let DiagnosticsSnapshot::Unavailable { reason, .. } = &snapshot else {
            panic!("{snapshot:?}");
        };
        assert!(reason.contains("not on PATH"), "{reason}");
    }

    #[test]
    fn every_source_off_is_unavailable_and_says_which() {
        let mut settings = InspectionSettings::default();
        settings.sources.insert("rust-analyzer".into(), false);
        let snapshot = ready_store().snapshot(&settings, EMIT_CAP);
        let DiagnosticsSnapshot::Unavailable { reason, sources } = &snapshot else {
            panic!("{snapshot:?}");
        };
        assert!(reason.contains("turned off"), "{reason}");
        assert!(reason.contains("rust-analyzer"), "{reason}");
        // The row still exists, so the panel can show *why* it is silent rather than omitting it.
        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn filtering_a_severity_removes_it_from_the_snapshot_the_bar_and_the_panel_share() {
        // One snapshot, one filter, so the two surfaces cannot disagree. This is the invariant
        // that would break if the filter moved to the frontend.
        let mut settings = InspectionSettings::default();
        settings.severities.hint = false;
        let snapshot = ready_store().snapshot(&settings, EMIT_CAP);
        let DiagnosticsSnapshot::Ready { items, sources, .. } = &snapshot else {
            panic!("{snapshot:?}");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].severity, Severity::Error);
        assert_eq!(
            sources[0].items, 1,
            "the per-source count ignored the filter"
        );
    }

    #[test]
    fn the_filter_never_reaches_the_mcp_tools_view_of_the_store() {
        // A display preference is not a statement about what Claude should be told.
        let mut settings = InspectionSettings::default();
        settings.severities.hint = false;
        let store = ready_store();
        assert_eq!(items_len(&store.snapshot(&settings, EMIT_CAP)), 1);
        assert_eq!(store.for_abs_path("/repo/src/a.rs").len(), 2);
    }

    #[test]
    fn tree_sitter_alone_and_clean_does_not_claim_the_workspace_is_clean() {
        // tree-sitter only sees open buffers. "No problems found" over a workspace of 40,000
        // unopened files would be a claim nothing has checked.
        let mut store = DiagnosticStore::new();
        store.set_status(DiagnosticSourceId::TreeSitter, SourceStatus::Ready);
        store.publish(
            DiagnosticSourceId::TreeSitter,
            "/repo/open.rs".into(),
            vec![],
        );

        let snapshot = store.snapshot(&InspectionSettings::default(), EMIT_CAP);
        let DiagnosticsSnapshot::Ready { source, .. } = &snapshot else {
            panic!("{snapshot:?}");
        };
        assert_eq!(source, "tree-sitter (open files only)");
    }

    #[test]
    fn a_language_server_beside_tree_sitter_drops_the_qualifier() {
        let mut store = ready_store();
        store.set_status(DiagnosticSourceId::TreeSitter, SourceStatus::Ready);
        let snapshot = store.snapshot(&InspectionSettings::default(), EMIT_CAP);
        let DiagnosticsSnapshot::Ready { source, .. } = &snapshot else {
            panic!("{snapshot:?}");
        };
        assert_eq!(source, "rust-analyzer and tree-sitter");
    }

    #[test]
    fn the_emit_cap_keeps_the_worst_problems_and_reports_what_it_dropped() {
        let mut store = DiagnosticStore::new();
        store.set_status(DiagnosticSourceId::RustAnalyzer, SourceStatus::Ready);
        let mut items: Vec<Diagnostic> = (0..50)
            .map(|i| {
                item(
                    &format!("src/h{i}.rs"),
                    Severity::Hint,
                    "rust-analyzer",
                    "h",
                )
            })
            .collect();
        items.push(item(
            "src/z.rs",
            Severity::Error,
            "rust-analyzer",
            "the one that matters",
        ));
        store.publish(DiagnosticSourceId::RustAnalyzer, "/repo/all".into(), items);

        let snapshot = store.snapshot(&InspectionSettings::default(), 3);
        let DiagnosticsSnapshot::Ready {
            items, truncated, ..
        } = &snapshot
        else {
            panic!("{snapshot:?}");
        };
        assert_eq!(items.len(), 3);
        assert_eq!(*truncated, 48);
        // Worst first, so a cap of three does not throw away the only error.
        assert_eq!(items[0].severity, Severity::Error);
    }

    #[test]
    fn a_ready_source_with_nothing_is_a_reportable_zero() {
        let mut store = DiagnosticStore::new();
        store.set_status(DiagnosticSourceId::RustAnalyzer, SourceStatus::Ready);
        store.publish(
            DiagnosticSourceId::RustAnalyzer,
            "/repo/a.rs".into(),
            vec![],
        );
        let snapshot = store.snapshot(&InspectionSettings::default(), EMIT_CAP);
        let DiagnosticsSnapshot::Ready { items, source, .. } = &snapshot else {
            panic!("{snapshot:?}");
        };
        assert!(items.is_empty());
        assert_eq!(source, "rust-analyzer");
    }

    #[test]
    fn a_source_that_has_been_mentioned_but_never_answered_is_scanning() {
        // `set_status` is not the only way a source enters the store — `publish` creates one too,
        // and its default must not be `Ready`. A `Ready` default would make the very first
        // publish of a partial result read as a complete answer.
        let mut store = DiagnosticStore::new();
        store.publish(DiagnosticSourceId::Gopls, "/repo/a.go".into(), vec![]);
        let snapshot = store.snapshot(&InspectionSettings::default(), EMIT_CAP);
        assert!(
            matches!(snapshot, DiagnosticsSnapshot::Scanning { .. }),
            "{snapshot:?}"
        );
    }

    #[test]
    fn clearing_a_source_forgets_it_entirely() {
        let mut store = ready_store();
        store.clear_source(DiagnosticSourceId::RustAnalyzer);
        assert!(!store.has_looked_at("/repo/src/a.rs"));
        assert!(store.known_paths().is_empty());
    }

    #[test]
    fn joining_labels_reads_like_a_sentence() {
        assert_eq!(join(&["a"]), "a");
        assert_eq!(join(&["a", "b"]), "a and b");
        assert_eq!(join(&["a", "b", "c"]), "a, b and c");
    }
}
