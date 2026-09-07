//! The undo stack behind Ctrl+Shift+T. (M15)
//!
//! # Why this is not in `Workspace`
//!
//! A closed tab is not layout. Nothing draws it, no window has to agree about it, and it does
//! not survive the process — so putting it in the tree would mean every `×` bumping `rev` and
//! broadcasting the whole workspace to every window for a record with no reader. That is the
//! argument [`crate::positions_state`] already makes for view positions, and it applies here
//! twice over: the record is *larger* than a view position (it carries a pane tree) and the
//! event it would ride on is the one closing gesture that already re-renders every tab strip.
//!
//! The cost of keeping it out is that the frontend cannot see the depth, which is why
//! `tab.reopenClosed` ships with no `when` clause — `cide_core::commands` argues that at the
//! command.
//!
//! # Why it does not survive a restart, either
//!
//! It could: `persist::recent_path` is right there and the file would be small. It should not.
//! The gesture means "I closed that ten seconds ago by accident", and the first Ctrl+Shift+T of
//! a session reopening something deliberately closed last week is the opposite of that — the
//! user is offered a hole they did not dig. Worse for the `ClaudeFull` records: their session
//! ids name children of a process that has exited, so a restored record would come back as a
//! Resume splash rather than as the tab it promises.
//!
//! `recent.json` is what "come back to this tomorrow" looks like in this app, and it is a
//! different feature with a different list.
//!
//! # One vec for the whole app, filtered on the way out
//!
//! Records carry their [`ProjectId`] and [`pop`] takes one, so Ctrl+Shift+T in a window showing
//! project A can never resurrect a file from project B. A map keyed by project was the
//! alternative and buys nothing: the ordering *between* projects is the thing being remembered,
//! a map would need its own pruning when a project closes, and 16 records is not a scan worth
//! optimising.

use cide_ipc::{PaneTree, ProjectId, TabKind};
use parking_lot::Mutex;

/// How many closes are remembered, across all projects.
///
/// The same number as `persist::MAX_RECENT`, and for the same reason: it is this codebase's
/// existing answer to "how many things might I want back". The user asked for "another and
/// another until the end of the list", so this is a stack rather than a slot — but past about a
/// dozen the gesture has stopped being undo and become browsing, and Ctrl+P is the better tool
/// for that. A record is a path plus a small tree, so the cap is about the gesture and not
/// about memory.
const MAX_CLOSED: usize = 16;

/// One tab, as it was when it went away.
///
/// Everything needed to put it back and nothing that could go stale in a way that matters:
///
/// * `project` — reopening into a different project is meaningless.
/// * `kind` — the whole identity. A path, a diff's fetch key, a settings section, a title.
/// * `index` — where it sat in the strip. Clamped by `workspace::reinsert_tab`, so a record
///   made when the strip was longer still lands somewhere sensible.
/// * `tree` — the split shape, *with its original pane ids and session bindings*. That is what
///   makes a `ClaudeFull` tab come back attached to its own still-running conversation rather
///   than to a fresh one; see `workspace::reinsert_tab` for why the ids are not reminted.
///
/// **Scroll position and caret are deliberately absent.** They are already remembered, by path,
/// in `positions.json` — `panes/EditorPane.tsx` flushes the pending position on unmount and
/// fetches it beside the file text on every open, so a reopened file lands where it was left
/// with nothing in this record. The one hole is `persist::MAX_POSITIONS`: close a file, open 256
/// others, and its position is evicted. That is the existing contract for every other route
/// back into a file and there is no reason for this one to differ.
#[derive(Debug, Clone)]
pub struct ClosedTab {
    pub project: ProjectId,
    pub kind: TabKind,
    pub index: usize,
    pub tree: PaneTree,
}

#[derive(Default)]
pub struct ClosedTabs {
    inner: Mutex<Vec<ClosedTab>>,
}

impl ClosedTabs {
    /// Remember a tab that has just closed. Newest first.
    ///
    /// # What is refused, and why the refusal is here rather than at the pop
    ///
    /// A [`TabKind::Diff`] whose origin is `ClaudeMcp` is **not** remembered. Closing that tab
    /// cancels the agent's request (`cmd::project::tab_close` → `ide::resolve_or_cancel`), so
    /// the CLI has stopped waiting and the `request_id` in the spec names nothing — the pane
    /// would mount, call `claude_diff_content`, and get an error. Filtering at push time rather
    /// than at pop time is the difference between a stack the user can count through and one
    /// with an invisible hole in it: skipping at pop makes one press of Ctrl+Shift+T silently
    /// do the work of two.
    ///
    /// [`TabKind::ClaudeHome`] cannot reach here — `close_tab` refuses index 0 — and is refused
    /// anyway rather than relied upon, because a record for it would be one `reinsert_tab` away
    /// from a project with two consoles.
    pub fn push(&self, tab: ClosedTab) {
        if !remembered(&tab.kind) {
            return;
        }
        let mut list = self.inner.lock();
        list.insert(0, tab);
        list.truncate(MAX_CLOSED);
    }

    /// Take the newest record belonging to `project`, or `None`.
    ///
    /// The caller may pop repeatedly — a record that turns out to be unusable (its file has
    /// been deleted, or the tab is somehow open already) is *consumed* and the next one asked
    /// for, so that the key never appears to do nothing while records remain. See
    /// `cmd::project::tab_reopen_closed`.
    pub fn pop(&self, project: ProjectId) -> Option<ClosedTab> {
        let mut list = self.inner.lock();
        let at = list.iter().position(|t| t.project == project)?;
        Some(list.remove(at))
    }

    /// The record for a specific **file** path, without consuming it.
    ///
    /// Back is not Ctrl+Shift+T and must never call [`pop`](Self::pop): a walk of the navigation
    /// history has a *path* in hand and wants that file back, whereas `pop` hands over whatever
    /// closed most recently. A Back into `main.rs` that resurrected an unrelated `Cargo.toml` —
    /// and threw away its record on the way — is a worse outcome than not restoring the split.
    ///
    /// Peek rather than take, because the caller cannot know whether it will *spend* the record
    /// until it has consulted the workspace: a tab that is already open is shown rather than
    /// reinserted, and in that case the record must stay for a later Ctrl+Shift+T. See
    /// `cmd::file::tab_reopen_file`.
    pub fn peek_file(&self, project: ProjectId, path: &std::path::Path) -> Option<ClosedTab> {
        let list = self.inner.lock();
        list.iter()
            .find(|t| is_file_record(t, project, path))
            .cloned()
    }

    /// Take the record for a specific file path, if there is one.
    ///
    /// Removes **only** the matching record and leaves the rest of the stack in order — the
    /// difference from [`pop`](Self::pop), and the reason this is not implemented in terms of
    /// it. Called after the caller has decided the record is actually being honoured, so the
    /// stack only ever loses a record whose tab the user can now see on screen.
    pub fn take_file(&self, project: ProjectId, path: &std::path::Path) -> Option<ClosedTab> {
        let mut list = self.inner.lock();
        let at = list.iter().position(|t| is_file_record(t, project, path))?;
        Some(list.remove(at))
    }

    /// Forget everything belonging to a project that is closing.
    ///
    /// Without this the stack outlives its project and every record in it names a `ProjectId`
    /// that `reinsert_tab` answers `NoSuchProject` for — and, if the same directory is opened
    /// again, `open_project` mints a *new* id, so they are unreachable rather than merely
    /// stale. Called from `project_close`, beside the detached windows it already prunes.
    pub fn forget_project(&self, project: ProjectId) {
        self.inner.lock().retain(|t| t.project != project);
    }

    /// How many records this project has. Tests and diagnostics only.
    pub fn depth(&self, project: ProjectId) -> usize {
        self.inner
            .lock()
            .iter()
            .filter(|t| t.project == project)
            .count()
    }
}

/// Whether this record is *that project's* record for *that file*.
///
/// Both halves matter and the project one is the half that would be silently wrong: two projects
/// open on two checkouts of the same repository hold the same absolute paths only when one is a
/// worktree of the other, but a scratch file or a `/tmp` path is genuinely shared — and a Back in
/// project A reinserting a tab whose record names project B would put the tab in the wrong strip
/// (or, once `reinsert_tab` refused the id, in none at all).
fn is_file_record(record: &ClosedTab, project: ProjectId, path: &std::path::Path) -> bool {
    record.project == project
        && matches!(&record.kind, TabKind::File { path: p, .. } if p.as_path() == path)
}

/// Whether a tab of this kind is worth remembering. See [`ClosedTabs::push`].
///
/// The console is the one refusal that is *this stack's*: a project has exactly one, so a
/// record for it would be one `reinsert_tab` away from a project with two. Every other answer
/// is `cide_core::workspace::tab_outlives_close`, which is shared with the project-level
/// reopen (`workspace::reopen_project`) so the two features cannot disagree about which tabs
/// can come back — a `ClaudeMcp` diff whose request was cancelled at the close, and a merge
/// resolver over a conflict that may no longer exist, are refused there with the reasons.
fn remembered(kind: &TabKind) -> bool {
    !matches!(kind, TabKind::ClaudeHome) && cide_core::workspace::tab_outlives_close(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{DiffOrigin, DiffSpec, Pane, PaneId, PaneKind, PaneRole, RepoId};
    use std::path::PathBuf;

    /// A one-pane tree, built through `cide_core::layout` rather than by hand: `indexmap` is
    /// deliberately not a dependency of this crate, and a literal `PaneTree` here would be a
    /// second construction of a shape the domain already owns.
    fn tree() -> PaneTree {
        cide_core::layout::new_tree(Pane {
            id: PaneId::new(),
            kind: PaneKind::Editor,
            role: PaneRole::Auxiliary,
            session: None,
            conversation: None,
            conversation_since: None,
            continues: None,
            title: "x".into(),
        })
    }

    fn record(project: ProjectId, name: &str) -> ClosedTab {
        ClosedTab {
            project,
            kind: TabKind::File {
                path: PathBuf::from(format!("/tmp/{name}")),
                dirty: false,
            },
            index: 1,
            tree: tree(),
        }
    }

    fn diff(project: ProjectId, origin: DiffOrigin) -> ClosedTab {
        ClosedTab {
            project,
            kind: TabKind::Diff {
                spec: DiffSpec {
                    title: "d".into(),
                    old_path: PathBuf::from("a"),
                    new_path: PathBuf::from("b"),
                    origin,
                },
                preview: false,
            },
            index: 1,
            tree: tree(),
        }
    }

    #[test]
    fn popping_walks_back_through_the_closes_one_at_a_time() {
        let stack = ClosedTabs::default();
        let project = ProjectId::new();
        for name in ["a", "b", "c"] {
            stack.push(record(project, name));
        }

        // "and another and another until the end of the list", newest first.
        let mut seen = Vec::new();
        while let Some(tab) = stack.pop(project) {
            let TabKind::File { path, .. } = tab.kind else {
                panic!("a file record");
            };
            seen.push(path);
        }
        assert_eq!(
            seen,
            ["/tmp/c", "/tmp/b", "/tmp/a"].map(PathBuf::from).to_vec()
        );
        assert!(stack.pop(project).is_none(), "and then it is empty");
    }

    /// Ctrl+Shift+T in a window showing project A must not resurrect a file from project B.
    ///
    /// One global vec makes this a filter rather than an invariant, so the ordering here is
    /// chosen to *catch a missing filter* rather than to look representative: **b's record is
    /// the newest**, so an unscoped `pop` — `list.remove(0)`, the obvious implementation —
    /// hands back b1 and this fails. Written the other way round first, with a's record newest,
    /// it passed against exactly that bug.
    #[test]
    fn a_project_only_ever_pops_its_own_records() {
        let stack = ClosedTabs::default();
        let (a, b) = (ProjectId::new(), ProjectId::new());
        stack.push(record(a, "a1"));
        stack.push(record(a, "a2"));
        stack.push(record(b, "b1"));

        let popped = stack.pop(a).expect("a record for a");
        assert_eq!(popped.project, a);
        let TabKind::File { path, .. } = popped.kind else {
            panic!("a file record");
        };
        assert_eq!(
            path,
            PathBuf::from("/tmp/a2"),
            "newest of *a*'s, not of all"
        );
        assert_eq!(stack.depth(b), 1, "and b's record is untouched");
    }

    /// Back names a *path*, so it must take that path's record and leave the stack alone.
    ///
    /// Ordered to catch the two implementations that would pass a friendlier arrangement. The
    /// wanted record is **not** the newest, so a `remove(0)` — or a `pop`-shaped scan that stops
    /// at the first record for the project — hands back `b` and fails here. And a second record
    /// for another *project* sits on the same path, so a match that forgot `project` takes the
    /// wrong one.
    #[test]
    fn back_takes_the_record_for_its_own_file_and_leaves_the_rest() {
        let stack = ClosedTabs::default();
        let (a, other) = (ProjectId::new(), ProjectId::new());
        stack.push(record(other, "wanted")); // same path, wrong project
        stack.push(record(a, "wanted"));
        stack.push(record(a, "b")); // newest, and not what Back asked for

        let path = PathBuf::from("/tmp/wanted");
        let peeked = stack.peek_file(a, &path).expect("a record for that file");
        assert_eq!(
            peeked.project, a,
            "and it is this project's, not the other's"
        );
        assert_eq!(stack.depth(a), 2, "a peek consumes nothing");

        let taken = stack.take_file(a, &path).expect("a record for that file");
        assert_eq!(taken.project, a);
        assert_eq!(stack.depth(a), 1, "only the matching record went");
        assert_eq!(
            stack.depth(other),
            1,
            "and the other project's is untouched"
        );

        // The newest is still there, still `b`: Ctrl+Shift+T after a Back gets what it always
        // would have. This is the whole point of not popping.
        let TabKind::File { path: newest, .. } = stack.pop(a).expect("a record").kind else {
            panic!("a file record");
        };
        assert_eq!(newest, PathBuf::from("/tmp/b"));
        assert!(
            stack.take_file(a, &path).is_none(),
            "and a second Back finds nothing left to spend"
        );
    }

    #[test]
    fn a_closing_project_takes_its_records_with_it() {
        let stack = ClosedTabs::default();
        let (a, b) = (ProjectId::new(), ProjectId::new());
        stack.push(record(a, "a1"));
        stack.push(record(b, "b1"));

        stack.forget_project(a);
        assert_eq!(stack.depth(a), 0);
        assert_eq!(stack.depth(b), 1, "and only that project's");
    }

    /// The one refusal that matters, and it is refused at *push*.
    ///
    /// A `ClaudeMcp` diff's tab close has already cancelled the agent's request, so the
    /// `request_id` names nothing and the pane would mount into an error. A record that can
    /// never be usefully popped is a hole the user counts through — one press doing the work of
    /// two — which is why this is not a filter at the pop.
    #[test]
    fn a_diff_claude_is_blocked_on_is_never_remembered() {
        let stack = ClosedTabs::default();
        let project = ProjectId::new();
        stack.push(diff(
            project,
            DiffOrigin::ClaudeMcp {
                request_id: "r1".into(),
            },
        ));
        assert_eq!(stack.depth(project), 0);

        // A git diff is a plain tab and comes back like any other.
        stack.push(diff(
            project,
            DiffOrigin::Git {
                repo: RepoId::new(),
                path: "src/lib.rs".into(),
                side: cide_ipc::git::DiffSide::Unstaged,
            },
        ));
        assert_eq!(stack.depth(project), 1);
    }

    #[test]
    fn the_stack_forgets_the_oldest_rather_than_growing_without_end() {
        let stack = ClosedTabs::default();
        let project = ProjectId::new();
        for n in 0..MAX_CLOSED + 4 {
            stack.push(record(project, &format!("f{n}")));
        }
        assert_eq!(stack.depth(project), MAX_CLOSED);

        let newest = stack.pop(project).expect("a record");
        let TabKind::File { path, .. } = newest.kind else {
            panic!("a file record");
        };
        assert_eq!(
            path,
            PathBuf::from(format!("/tmp/f{}", MAX_CLOSED + 3)),
            "the cap drops the oldest, not the newest"
        );
    }
}
