//! File-tab and document commands. (M9)
//!
//! Two pairs, and they are unrelated to each other. `tab_open_file` / `tab_set_dirty` are
//! workspace mutations in the same shape as everything in `cmd::project`. `file_read` /
//! `file_write` touch the disk instead, and are the only commands in the app that do
//! arbitrary blocking IO on a path the user chose — so they are the only ones that go
//! through `spawn_blocking`. A `cargo build` saturating the page cache, or a project on a
//! stalled NFS mount, would otherwise freeze the event loop and with it every terminal in
//! the window.

use std::path::{Path, PathBuf};

use cide_core::document;
use cide_core::workspace;
use cide_core::workspace::PreviewSlot;
use cide_core::{CoreError, Result};
use cide_ipc::git::DiffSide;
use cide_ipc::history::RevSide;
use cide_ipc::{
    ClaudeSendTarget, DiffOrigin, DiffSpec, FileDoc, Pane, PaneId, PaneKind, PaneRole, ProjectId,
    ReopenedFile, RepoId, TabId, TabKind,
};
use tauri::{Manager, State};

use crate::cmd::project::Mutated;
use crate::workspace_state::WorkspaceState;

/// Open a file tab, or activate the one already showing this path.
///
/// Re-opening rather than duplicating is not a nicety: two tabs over one path are two
/// buffers over one file, and whichever saves second silently discards the other's edits.
/// The domain has no opinion about paths, so the search is here.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_file(
    state: State<'_, WorkspaceState>,
    app: tauri::AppHandle,
    project: ProjectId,
    path: String,
) -> Result<TabId> {
    let tab = open_file_tab(&state, project, PathBuf::from(path))?;
    raise_if_detached(&app, &state, tab);
    Ok(tab)
}

/// Bring a torn-out tab's window forward, when the tab has one.
///
/// The other half of `activate_tab`'s silent success for a detached tab: the shell is
/// deliberately not drawing it, so opening a file that is already open in a torn-out window
/// would otherwise be the one open gesture with nothing visible to show for it. The domain
/// cannot do this — raising is a desktop act — so every command that lands in an existing
/// tab passes through here after the mutation commits.
fn raise_if_detached(app: &tauri::AppHandle, state: &WorkspaceState, tab: TabId) {
    if let Some(label) = workspace::detached_tab_window(&state.snapshot(), tab) {
        crate::windows::raise(app, &label);
    }
}

/// The body of [`tab_open_file`], as a function over the state.
///
/// Split out so [`terminal_open_path`] can perform its refusals and then land in *this* tab
/// list rather than growing a second one. Two functions that both open file tabs is how two
/// tabs over one path come back.
///
/// `pub(crate)` for the same reason, one caller further out: `crate::edit_wait` opens the tab a
/// blocked `cide --wait` is waiting on, and it needs the deduplication above — a second tab over
/// the path would be a second buffer over the file the CLI is about to read back.
pub(crate) fn open_file_tab(
    state: &WorkspaceState,
    project: ProjectId,
    path: PathBuf,
) -> Result<TabId> {
    state.update(|ws| {
        let existing = workspace::project(ws, project)?
            .tabs
            .iter()
            .find(|t| matches!(&t.kind, TabKind::File { path: p, .. } if *p == path))
            .map(|t| t.id);
        if let Some(id) = existing {
            workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }

        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        workspace::open_tab(
            ws,
            project,
            TabKind::File { path, dirty: false },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                // Auxiliary, like every pane outside the pinned console: splitting a file
                // tab and closing one of the halves must not be refused, and closing the
                // last one closes the tab.
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                conversation_since: None,
                continues: None,
                harness: None,
                title,
                docker: None,
                origin: None,
            },
        )
    })
}

/// Reopen a file the navigation history remembers — the Back/Forward half of `tab_open_file`.
///
/// # Why Back does not simply call `tab_open_file`
///
/// It did, and the result was a *fresh single-pane editor tab* every time. That is wrong twice
/// over, and the second one is the expensive one:
///
/// * A file tab's `PaneTree` is not always one editor. `cmd::pane::default_intent` gives a File
///   tab `SplitIntent::NewClaude`, so splitting one produces a `PaneKind::Claude` pane bound to a
///   live conversation — and `closing_record` clones that whole tree, pane ids included, exactly
///   so `reinsert_tab` can hand the parked terminal back to `paneHosts`. A plain open discards
///   all of it.
/// * **It silently poisoned Ctrl+Shift+T.** Back opened a fresh tab for X and activated it while
///   X's record was still on the closed-tab stack; the next Ctrl+Shift+T popped that record,
///   found X open *and active*, answered `Reopen::Skip`, and **consumed the record anyway** —
///   so one press did the work of two, landed the user on an unrelated tab, and destroyed the
///   split and the pane id naming a still-running `claude`. That is precisely the
///   one-press-two-records hole `ClosedTabs::push` filters at push time to avoid.
///
/// # What it does with the stack: peek, then take only what it spends
///
/// Never [`crate::closed_tabs::ClosedTabs::pop`] — that hands over the newest record for the
/// project, and Back has a *path* in mind. The rule, and the cost of the alternative in each
/// case:
///
/// * `Reinsert` — the tab is genuinely coming back, so the record is **taken**. The stack loses
///   exactly the record that has just been honoured, visibly, on screen. Leaving it would be the
///   poisoning above with the tabs swapped: Ctrl+Shift+T would later find the tab open and burn
///   the record for nothing.
/// * `Show` / `Skip` — the tab is open already, so this activates it and **leaves the record**.
///   Back's job is done by the activation; the record is still the right answer for a
///   Ctrl+Shift+T after the user closes that tab again. Consuming it here would be the classic
///   invisible spend — nothing on screen changes to explain where it went.
///
/// # And the stat, which is not belt-and-braces
///
/// `tab_open_file` does not stat, by its own documentation, so a Back into a deleted file or a
/// discarded scratch used to mint a permanent tab reading *"This file could not be opened / No
/// such file or directory (os error 2)"* — a sentence that does not even contain the path. The
/// same `is_file` test `reopen_plan` already runs answers it here, and the caller turns it into
/// a notice naming the file.
///
/// # What this does *not* re-check, and why that is now safe
///
/// Containment. A Back entry can legitimately name a path outside every root — a dependency
/// source under *External Libraries*, a scratch under `scratches_root()`, an out-of-project
/// terminal open the user approved by name — and refusing those would break the feature for the
/// files it most exists to serve. What made that dangerous was not the missing check here but
/// `App.tsx` recording a terminal jump **before** `openFromTerminal` had been refused: a path the
/// user *declined* in the out-of-project dialog sat on the Back stack, and Back walked into it
/// through a command that enforces nothing. That is fixed at the source — the entry is now
/// written from the `.then()`, so only an open that actually succeeded is a place the user has
/// been, which is `navHistory.ts`'s own stated rule. `openable`'s guards therefore still stand
/// in front of every untrusted path; this command is reachable only from places one has already
/// cleared.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_reopen_file(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: String,
) -> Result<ReopenedFile> {
    let path = PathBuf::from(path);
    let stack = app.state::<crate::closed_tabs::ClosedTabs>();

    // Peeked, never popped, and *before* the plan — the plan needs the record to decide. The stat
    // is folded into `reopen_file_step` beside it so the whole decision is one call.
    let record = stack.peek_file(project, &path);
    let plan = record
        .as_ref()
        .map(|record| state.with(|ws| crate::cmd::project::reopen_plan(ws, record)));

    match reopen_file_step(path.is_file(), plan) {
        ReopenFile::Gone => Ok(ReopenedFile::Gone {
            path: path.display().to_string(),
        }),
        ReopenFile::Restore => {
            // Taken only now, once the step has committed to spending it. Between the peek and
            // here the workspace lock has been released, so `take_file` can find nothing if
            // another window reopened the same tab in the meantime — in which case this falls
            // back to an ordinary open rather than reinserting a record somebody else owns.
            let Some(record) = stack.take_file(project, &path) else {
                return Ok(ReopenedFile::Opened {
                    tab: open_file_tab(&state, project, path)?,
                });
            };
            let tab = state.update(|ws| {
                workspace::reinsert_tab(ws, project, record.index, record.kind, record.tree)
            })?;
            Ok(ReopenedFile::Restored { tab })
        }
        // `open_file_tab` rather than a hand-rolled insert or a bare `activate_tab`, because it
        // already matches an open File tab on its path: it activates the one that is there and
        // opens one when there is not. `reinsert_tab` performs no such dedupe, and a second
        // opener with its own idea of "already open" is how two tabs over one file come back —
        // and with them the save that silently discards the other buffer.
        ReopenFile::Open => {
            let tab = open_file_tab(&state, project, path)?;
            // A Back into a file living in a torn-out window has to show that window, for
            // the reason `tab_open_file` gives: the shell will not draw the tab.
            raise_if_detached(&app, &state, tab);
            Ok(ReopenedFile::Opened { tab })
        }
    }
}

/// What a Back or Forward into a path should do. See [`reopen_file_step`].
#[derive(Debug, PartialEq, Eq)]
enum ReopenFile {
    /// Nothing is at that path. Open nothing, spend nothing, say so.
    Gone,
    /// Put the remembered tab back where it was, and spend the record that describes it.
    Restore,
    /// An ordinary open — or an activation of the tab that is already showing this file. Any
    /// record for it stays on the stack.
    Open,
}

/// The reopen rule, as a function of the disk and the closed-tab record alone.
///
/// A free function rather than three arms inside the command above, for the reason
/// `cmd::project::reopen_plan` gives about itself: a rule reachable only through a
/// `State<WorkspaceState>` is a rule that gets tested at the level of "does the app start", and
/// this is the rule where **a record gets spent**. Spending one invisibly is not a crash; it is a
/// Ctrl+Shift+T months later that opens the wrong tab, which is precisely the class of bug no
/// integration test notices.
///
/// `plan` is `reopen_plan`'s answer for the record, or `None` when the stack has no record for
/// this path. The three rules, and what the other choice would cost in each:
///
/// * **The stat wins over everything.** A file that is gone is `Gone` *even when a record exists*,
///   and the record is left alone. Consulting the stack first and discovering the deletion
///   afterwards would burn a record on a tab that never appeared — the invisible spend again, with
///   nothing on screen to explain it. (`reopen_plan` also stats, and answers `Skip`; that is the
///   right answer for Ctrl+Shift+T, which moves on to the next record, and the wrong one here,
///   where the user named *this* file and deserves to be told about it.)
/// * **`Reinsert` is the only case that spends the record.** The tab is genuinely coming back with
///   its pane tree, so the stack loses exactly the record the user can now see honoured. Leaving
///   it would poison the next Ctrl+Shift+T: that press would pop it, find the tab open, and
///   consume it for nothing.
/// * **`Show`, `Skip` and no record at all are one answer.** The tab is open already (or has never
///   been closed), so the press is an activation and the record — if there is one — is still the
///   right answer for a Ctrl+Shift+T after the user closes that tab again. Consuming it here is
///   the invisible spend a third time.
fn reopen_file_step(exists: bool, plan: Option<crate::cmd::project::Reopen>) -> ReopenFile {
    use crate::cmd::project::Reopen;
    if !exists {
        return ReopenFile::Gone;
    }
    match plan {
        Some(Reopen::Reinsert) => ReopenFile::Restore,
        Some(Reopen::Show(_)) | Some(Reopen::Skip) | None => ReopenFile::Open,
    }
}

/// Why a path named by terminal output was not opened.
///
/// Tagged `{kind, message, path, real}` on the wire like [`ClaudeSendError`], and typed rather
/// than a bare string for the same reason: the frontend must be able to tell a refusal from a
/// failure without matching on prose. Every variant is a sentence a user reads in the notice
/// stack — except [`Self::Outside`], which is a *question*, and the only one the frontend turns
/// into a dialog instead.
///
/// A silent `Ok` for a refused path was the obvious alternative and is the worst of them: a
/// ctrl+click that resolves to nothing is indistinguishable from a link wired to nothing, which
/// is the defect this project has now found twelve times.
#[derive(Debug, thiserror::Error)]
pub enum TerminalOpenError {
    /// The path is not inside any of this project's roots.
    ///
    /// **This is the one refusal the user may overrule**, and `real` is what makes that safe:
    /// it is the canonical path the click would actually open, which is what the confirmation
    /// names and what comes back as `approvedTarget`. It is `None` when there is nothing to
    /// approve because the path was *malformed* rather than merely outside — not absolute, or
    /// carrying a `..` component — and a `None` here is the frontend's signal that no dialog
    /// may be offered. See [`outside_ask`'s mirror in `ui/src/terminal/outsideOpen.ts`].
    #[error("{path} is outside this project's roots.")]
    Outside { path: String, real: Option<String> },

    /// Nothing is there. Ordinary: output outlives the files it names.
    #[error("{0} no longer exists")]
    Missing(String),

    /// A directory, a device node, a socket or a FIFO.
    #[error("{0} is not a regular file, so there is nothing to open in an editor")]
    NotAFile(String),

    /// Past the editor's own limit, refused *before* a tab exists rather than after.
    #[error("{path} is {size} MiB; the editor opens files up to {limit} MiB")]
    TooLarge { path: String, size: u64, limit: u64 },

    /// The workspace refused the tab — a project that closed under the click, most likely.
    #[error("{0}")]
    Failed(String),
}

impl TerminalOpenError {
    /// The path this refusal is about, as it was asked for.
    ///
    /// Serialised alongside the message so the frontend's rules can be written over data rather
    /// than over prose. The caller already knows the string it sent; carrying it back anyway is
    /// what lets `outsideOpen.ts` be a pure function of the refusal alone, which is what makes
    /// it testable under node.
    fn path(&self) -> &str {
        match self {
            Self::Outside { path, .. } => path,
            Self::Missing(p) | Self::NotAFile(p) | Self::Failed(p) => p,
            Self::TooLarge { path, .. } => path,
        }
    }
}

impl serde::Serialize for TerminalOpenError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let kind = match self {
            Self::Outside { .. } => "outside",
            Self::Missing(_) => "missing",
            Self::NotAFile(_) => "notAFile",
            Self::TooLarge { .. } => "tooLarge",
            Self::Failed(_) => "failed",
        };
        let real = match self {
            Self::Outside { real, .. } => real.as_deref(),
            _ => None,
        };
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("TerminalOpenError", 4)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &self.to_string())?;
        st.serialize_field("path", self.path())?;
        st.serialize_field("real", &real)?;
        st.end()
    }
}

/// What [`openable`] decided about a path it is willing to open.
#[derive(Debug, PartialEq, Eq)]
struct Openable {
    /// The canonical path. **Not** what the tab opens on — see [`terminal_open_path`] — but it
    /// is what an out-of-project approval is bound to, so it has to travel out of here.
    real: PathBuf,
    /// This path is outside every root and the user approved it by name.
    outside: bool,
}

/// Every check a path parsed out of a pane's bytes has to pass before it may open a tab.
///
/// A free function over the roots so it can be driven against real directories in a test
/// without a `WorkspaceState`, an `AppHandle` or a window — which is the only way the refusals
/// below get exercised at all, and they are the half of this feature that must not rot.
///
/// # Why this exists rather than reusing `tab_open_file`
///
/// `tab_open_file` enforces **nothing**: it takes a `String`, makes a `PathBuf`, searches the
/// tab list and opens a tab. That has been safe only because every caller so far — the file
/// tree, the picker, the git panel — hands back a path the backend itself produced. It stops
/// being safe the instant a path parsed out of terminal output can reach it, and terminal
/// output is attacker-influenced by definition: it is a repository's build log, a file some
/// tool printed, a tool result. Note the asymmetry it would otherwise inherit — `fs_read_file`
/// calls `check_within`, and the editor's `file_read` does not. So the check goes where the
/// untrusted path *enters*, and `file_read` is left alone.
///
/// Unguarded, `⏺ Read(/home/you/.claude/.credentials.json)` printed by any program in any pane
/// would be a ~1 KiB valid-UTF-8 text file: it would open, and its contents would be in the
/// webview. That is the constraint this function exists around.
///
/// # Four guards on one error path, and only one of them is about the project boundary
///
/// They were written together and they answer completely different questions. Writing them
/// down separately is the point of this section, because M13 relaxed exactly one of them and a
/// reader who thinks of them as one rule will relax the wrong one next time.
///
/// | guard | concern | what removing it costs |
/// | --- | --- | --- |
/// | shape: absolute, no `..` | **integrity** — a path that means something different depending on who resolves it | a relative path canonicalised against cide's *own* cwd, which is not the project |
/// | containment (textual, then canonical) | **confidentiality** — any readable text file on the machine | `~/.claude/.credentials.json` in a buffer, and from there in `claude_send_lines` |
/// | `is_file` | **liveness** — FIFOs, device nodes, sockets, directories | a blocking-pool worker parked in `read_to_end` for ever, or `/dev/zero` read until the process is OOM-killed |
/// | size limit | **junk** — a 2 GB log becomes a tab and then an error | annoying, not dangerous |
///
/// Only the second is a *project* boundary, and it is the only one `approved` can overrule.
/// The other three stay hard refusals for an approved out-of-project path exactly as they are
/// for an in-project one — which is what the "…even when approved" tests below pin, because
/// until M13 containment fired first for everything under `/dev`, `/proc` and `/tmp` and those
/// three guards were very nearly decorative.
///
/// # Why containment is now checked *last* rather than first
///
/// It used to be first, so nothing outside the project was ever `stat`ed. The order is now
/// shape → `canonicalize` → `metadata` → containment, and the trade is deliberate:
///
/// * **A refusal that cannot be overruled must never open a dialog.** A ctrl+click on
///   `/dev/zero`, on a directory, or on a path that no longer exists is answered with one
///   sentence and no question — because asking "may cide open this?" about a thing it could
///   not open either way trains the user to approve without reading, which is the only way
///   this dialog can fail.
/// * **The dialog must name what would actually be opened**, which means the canonical path,
///   which means canonicalising before refusing.
///
/// What it costs is two `stat`-class syscalls on an out-of-project path before the user has
/// approved it, which tells the webview whether that path exists. That is not a new capability:
/// `file_read` beside this function has no containment at all, so a *compromised webview*
/// already reads any file, and the threat this function actually defends against is a different
/// one — attacker-chosen bytes on screen plus one unsuspecting ctrl+click. Against that threat
/// the defence has to be visible to the user at the moment of the click, and that is the
/// confirmation, not the ordering of two syscalls.
///
/// # What `approved` is, and why it is a path and not a bool
///
/// It is the canonical path the confirmation named — the one the user read before clicking
/// *Open*. Rust re-canonicalises and compares, so the approval is an approval of **a file**
/// rather than of a string. A bool would approve whatever that string resolves to *now*: swap
/// the symlink between the dialog and the click and the user's answer applies to a file they
/// were never shown. Mismatch is refused as `Outside` again, carrying the new target, so the
/// user is asked about what is actually there.
///
/// What none of this grants is any new *capability*. On success the only thing that happens is
/// the workspace mutation `tab_open_file` already performs; a terminal-derived path never
/// reaches `fs_show_in_manager`, `tauri_plugin_opener` or anything else that hands a path to
/// the desktop.
fn openable(
    roots: &[PathBuf],
    path: &Path,
    approved: Option<&Path>,
) -> std::result::Result<Openable, TerminalOpenError> {
    let shown = path.display().to_string();

    // Shape, against no roots at all — so it holds whether or not anything was approved. This
    // is `check_within`'s first two clauses, lifted out of it precisely because the third
    // (containment) is now overrulable and these two never are. `real: None` is what tells the
    // frontend there is nothing here to offer a dialog about.
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(TerminalOpenError::Outside {
            path: shown,
            real: None,
        });
    }

    let real = std::fs::canonicalize(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => TerminalOpenError::Missing(shown.clone()),
        _ => TerminalOpenError::Failed(format!("{shown}: {e}")),
    })?;

    // Liveness and junk, on the canonical path, before containment and before any approval is
    // consulted. `canonicalize` already proved it exists, so a failure here is a race or a
    // permission problem rather than the ordinary "output outlives its files".
    let meta = std::fs::metadata(&real).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => TerminalOpenError::Missing(shown.clone()),
        _ => TerminalOpenError::Failed(format!("{shown}: {e}")),
    })?;
    if !meta.is_file() {
        return Err(TerminalOpenError::NotAFile(shown));
    }
    if meta.len() > document::MAX_FILE_BYTES {
        return Err(TerminalOpenError::TooLarge {
            path: shown,
            size: meta.len() / (1024 * 1024),
            limit: document::MAX_FILE_BYTES / (1024 * 1024),
        });
    }

    // Containment, both locks. Textual on the path as given, because that is the string the
    // user read; canonical on what it resolves to, because the textual check cannot see a
    // symlink inside the project pointing out of it. The roots are canonicalised too: a machine
    // whose `$HOME` is a symlink would otherwise fail every one of its own files.
    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.clone()))
        .collect();
    let contained = cide_fs::ops::check_within(roots, path).is_ok()
        && cide_fs::ops::check_within(&canonical_roots, &real).is_ok();
    if contained {
        return Ok(Openable {
            real,
            outside: false,
        });
    }

    match approved {
        // The user was shown this exact target and said yes.
        Some(target) if target == real.as_path() => Ok(Openable {
            real,
            outside: true,
        }),
        // Either nobody has been asked yet, or the answer was about a different file than the
        // one this path resolves to now. Both end in the same place: ask about what is there.
        _ => Err(TerminalOpenError::Outside {
            path: shown,
            real: Some(real.display().to_string()),
        }),
    }
}

/// Open a file a terminal pane named — the one command whose path argument is untrusted.
///
/// The refusals are [`openable`]; this is the plumbing around them. The tab is opened on the
/// path **as given**, not on the canonical one, so the tab a user gets is the file they pointed
/// at rather than whatever a symlink resolved to — and so the `requestReveal` the frontend
/// parked under that same string is spent by the editor this opens.
///
/// `approved_target` is the out-of-project answer, and it is a *parameter rather than a second
/// command* on purpose. Two entry points into one tab list is how the trust story forks: the
/// day someone adds `terminal_open_path_approved` is the day one of the two stops canonicalising
/// and nobody notices, because the tests are attached to the other one. Absent — which is every
/// first click, and every click on an in-project path for ever — nothing about this command's
/// behaviour has changed.
///
/// `spawn_blocking` because [`openable`] is a `canonicalize` and two `stat`s, and a project on a
/// stalled network mount would otherwise take the event loop and with it every terminal in the
/// window. The same reasoning as `file_read` beside it.
#[tauri::command(rename_all = "camelCase")]
pub async fn terminal_open_path(
    state: State<'_, WorkspaceState>,
    app: tauri::AppHandle,
    project: ProjectId,
    path: PathBuf,
    approved_target: Option<PathBuf>,
) -> std::result::Result<TabId, TerminalOpenError> {
    let roots: Vec<PathBuf> = state
        .with(|ws| {
            workspace::project(ws, project)
                .map(|p| p.roots.iter().map(|r| r.path.clone()).collect())
        })
        .map_err(|e| TerminalOpenError::Failed(e.to_string()))?;

    let checked = path.clone();
    let job = tauri::async_runtime::spawn_blocking(move || {
        openable(&roots, &checked, approved_target.as_deref())
    });
    let opened = match job.await {
        Ok(result) => result?,
        Err(e) => {
            return Err(TerminalOpenError::Failed(format!(
                "file worker failed: {e}"
            )));
        }
    };

    if opened.outside {
        // Worth a line in the log and only a line: the user answered a dialog naming this exact
        // path, so it is not a surprise to them — but it is the one gesture in the app that
        // reads a file the project does not contain, and a log with no record of it would make
        // an after-the-fact "how did that get open?" unanswerable.
        tracing::info!(
            path = %path.display(),
            real = %opened.real.display(),
            "opening a file outside the project, approved by the user"
        );
    }

    let tab = open_file_tab(&state, project, path)
        .map_err(|e| TerminalOpenError::Failed(e.to_string()))?;
    // A ctrl+click on a path whose file lives in a torn-out window has to show that window —
    // the shell will not draw the tab, so without this the click reads as wired to nothing.
    raise_if_detached(&app, &state, tab);
    Ok(tab)
}

/// The tab a git diff opens as, given its fetch key.
///
/// Split out of the command so it can be tested without a `WorkspaceState`, and because it
/// is the one place that decides what a diff tab is *called*: the basename plus a marker, so
/// a `main.rs` diff and a `main.rs` editor are two distinguishable rows in the tab strip
/// rather than two identical ones.
///
/// `old_path` is git's pre-image path — set for a rename, absent otherwise — and the pair is
/// display only. Both sides stay **repo-relative**, matching the fetch key; see `DiffSpec`.
fn git_diff_spec(repo: RepoId, path: &str, side: DiffSide, old_path: Option<String>) -> DiffSpec {
    let name = path.rsplit('/').next().unwrap_or(path);
    DiffSpec {
        title: format!("{name} — diff"),
        old_path: PathBuf::from(old_path.unwrap_or_else(|| path.to_owned())),
        new_path: PathBuf::from(path),
        origin: DiffOrigin::Git {
            repo,
            path: path.to_owned(),
            side,
        },
    }
}

/// Whether an open tab is already showing this file's git diff.
///
/// Keyed on the repository and the path and **not** on the side. The pane switches sides in
/// place — the same file's staged and unstaged diffs are two views of one thing, and the
/// selection is cleared when it switches — so keying on the side as well would answer a
/// second double-click with a second tab over the same file, which is exactly the
/// duplicate-tab problem `tab_open_file` exists to avoid.
fn shows_git_diff(kind: &TabKind, repo: RepoId, path: &str) -> bool {
    matches!(
        kind,
        TabKind::Diff { spec, .. } if matches!(
            &spec.origin,
            DiffOrigin::Git { repo: r, path: p, .. } if *r == repo && p == path
        )
    )
}

/// Open a diff tab for one file in one repository, or activate the one already showing it.
///
/// # Why the diff text is not an argument
///
/// The caller — the git panel — is holding a `FileDiff` when it calls this, and handing that
/// over would save the pane a round trip. It is deliberately not accepted. `DiffSpec` is
/// persisted inside `Workspace`, which `cide-core::persist` debounces to `workspace.json`,
/// so a diff passed in here would be a diff written to disk; and a saved diff is a *stale*
/// diff the moment anything writes to the file, which on this code path is constantly (an
/// agent is editing, a build is running, a bash pane is committing). The pane calls
/// `git_diff_file` with the key in [`DiffOrigin::Git`] instead, exactly as a Claude diff
/// calls `claude_diff_content` with its `request_id`.
///
/// It is also what makes the tab survive a restart: a key still resolves tomorrow.
///
/// Nothing here touches the disk — it is a workspace mutation, like `tab_open_file` — so it
/// is deliberately *not* `async`. The blocking git read happens in `git_diff_file`, which
/// already goes through `spawn_blocking`.
///
/// # This is the *double-click* half
///
/// The tab it produces is `preview: false` — kept. Its partner is [`tab_retarget_diff`],
/// which the panel calls for a single click on a changelist row while a diff is already up,
/// and which re-points one scratch tab instead of adding another. Double-click has meant
/// "open properly" since the click rules landed; this is what "properly" now buys you.
///
/// Finding the file already open **promotes** it, which is the same rule read backwards: a
/// double-click on the file currently sitting in the preview slot means the user wants to
/// keep it, so the next single click must leave it alone and start a new scratch tab.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_diff(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    state.update(|ws| open_git_diff(ws, project, repo, &path, side, old_path))
}

/// The pane a diff tab opens with. One shape for both gestures, so a preview tab and a kept
/// one differ in exactly the flag and nothing a renderer could accidentally key off.
pub(super) fn diff_pane(title: String) -> Pane {
    Pane {
        id: PaneId::new(),
        kind: PaneKind::Diff,
        // Auxiliary like every pane a tab is opened with: a diff holds no conversation, so
        // there is nothing about it that must not be closed.
        role: PaneRole::Auxiliary,
        // No process, ever. A diff is a document.
        session: None,
        conversation: None,
        conversation_since: None,
        continues: None,
        harness: None,
        title,
        docker: None,
        origin: None,
    }
}

/// [`tab_open_diff`] without Tauri.
///
/// Split out for the tests below, which is not a formality here: what this and
/// [`retarget_git_diff`] do to a workspace *is* the fix for the thirty-tab report, and a
/// policy that can only be exercised through a `State<WorkspaceState>` is a policy that gets
/// tested at the level of `shows_git_diff` and nowhere else — which is exactly how a tab
/// lookup that was individually correct produced thirty tabs in a row.
fn open_git_diff(
    ws: &mut cide_ipc::Workspace,
    project: ProjectId,
    repo: RepoId,
    path: &str,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    let existing = workspace::project(ws, project)?
        .tabs
        .iter()
        .find(|t| shows_git_diff(&t.kind, repo, path))
        .map(|t| t.id);
    if let Some(id) = existing {
        workspace::promote_diff(ws, project, id)?;
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let spec = git_diff_spec(repo, path, side, old_path);
    let title = spec.title.clone();
    workspace::open_tab(
        ws,
        project,
        TabKind::Diff {
            spec,
            preview: false,
        },
        diff_pane(title),
    )
}

/// Point the preview diff tab at this file — the *single-click* half.
///
/// > *"in git files tree when i do one click on element - we should select it, but not open
/// > the diff. Only when diff is already opened one click should change current diff to
/// > selected file."*
///
/// The click rule shipped and routed to [`tab_open_diff`], which reuses a tab only when the
/// repo *and* the path match — so every other file got a new tab and clicking down a 30-file
/// changelist produced 30 tabs, the exact opposite of what was asked for. "Change the current
/// diff" is retargeting, a different operation from opening, and this is it.
///
/// Three outcomes, in this order, and the order is the whole design:
///
/// 1. **A tab already shows this file** — activate it, retarget nothing. Whether it is the
///    preview tab or a kept one, a second copy of a diff the user can already see is never
///    the answer, and stealing the scratch slot to duplicate an open tab would cost them the
///    file that was in it for no gain.
/// 2. **A preview tab exists** — retarget it. One tab, however far down the list they click.
/// 3. **Neither** — open one, marked `preview: true`. Reached when every diff tab on screen
///    was opened by double-click, i.e. deliberately kept; the honest answer there is a new
///    scratch tab rather than eating one the user asked for. It costs exactly one extra tab,
///    once, and every click after it lands in that tab. This is VS Code's rule for a
///    single-click in the explorer when the only open editors are permanent, and it is the
///    reason the flag exists at all — see [`cide_ipc::TabKind::Diff`].
///
/// Not `async`, for the same reason as [`tab_open_diff`]: no disk is touched here. The pane
/// notices its tab's `spec` changed and refetches through `git_diff_file`, which is where the
/// blocking read lives and is already on `spawn_blocking`.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_retarget_diff(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    state.update(|ws| retarget_git_diff(ws, project, repo, &path, side, old_path))
}

/// [`tab_retarget_diff`] without Tauri. See [`open_git_diff`] for why it is split out.
fn retarget_git_diff(
    ws: &mut cide_ipc::Workspace,
    project: ProjectId,
    repo: RepoId,
    path: &str,
    side: DiffSide,
    old_path: Option<String>,
) -> Result<TabId> {
    let showing = workspace::project(ws, project)?
        .tabs
        .iter()
        .find(|t| shows_git_diff(&t.kind, repo, path))
        .map(|t| t.id);
    if let Some(id) = showing {
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let spec = git_diff_spec(repo, path, side, old_path);
    if let Some(id) = workspace::preview_diff_tab(ws, project, PreviewSlot::Working)? {
        workspace::retarget_diff(ws, project, id, spec)?;
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let title = spec.title.clone();
    workspace::open_tab(
        ws,
        project,
        TabKind::Diff {
            spec,
            preview: true,
        },
        diff_pane(title),
    )
}

// --- the revision diff (M18) ------------------------------------------------------------
//
// The same pair of gestures over a different pair of trees. Everything below mirrors the four
// functions above line for line, and where it deliberately does not — `shows_revision_diff`'s
// key — the difference is written down at the site.

/// How one side of a revision pair is spelled in a tab title.
///
/// Seven characters of the oid, which is what `git log --oneline` shows and what every other
/// short oid in this application is cut to. The two non-commit sides get a word rather than a
/// sentinel: a title reading `main.rs @ 0000000` would name a commit that does not exist, and
/// the user cannot tell an invented oid from a real one by looking.
fn rev_label(side: &RevSide) -> String {
    match side {
        // `chars().take(7)` and not `[..7]`: an oid is hex so the two agree today, but slicing a
        // `String` by byte index panics on a non-ASCII boundary, and this value arrives from the
        // frontend. A short input is returned whole rather than padded.
        RevSide::Commit { oid } => oid.chars().take(7).collect(),
        RevSide::FirstParent => "parent".to_owned(),
        RevSide::WorkingTree => "working tree".to_owned(),
    }
}

/// The tab a revision diff opens as, given its fetch key.
///
/// Split out of the command for the reason [`git_diff_spec`] is — it can then be tested without
/// a `WorkspaceState`, and it is the one place that decides what the tab is *called*.
///
/// The title names the **new** side: `main.rs @ a1b2c3d`. That is the revision the user asked to
/// look at, and it is what makes two history entries for one file distinguishable in the strip,
/// which a title naming only the file would not be. The old side is deliberately not in it — a
/// strip is 160px per tab and `main.rs @ a1b2c3d ← 9f8e7d6` is a string nobody can read at that
/// width; the pane's header carries the pair in full.
///
/// `old_path` is git's pre-image path — set for a rename, absent otherwise — and the pair is
/// display only, exactly as in [`git_diff_spec`]. Both sides stay **repo-relative**, matching the
/// fetch key.
fn revision_diff_spec(
    repo: RepoId,
    path: &str,
    new: RevSide,
    old: RevSide,
    old_path: Option<String>,
) -> DiffSpec {
    let name = path.rsplit('/').next().unwrap_or(path);
    DiffSpec {
        title: format!("{name} @ {}", rev_label(&new)),
        old_path: PathBuf::from(old_path.unwrap_or_else(|| path.to_owned())),
        new_path: PathBuf::from(path),
        origin: DiffOrigin::GitRevision {
            repo,
            path: path.to_owned(),
            new,
            old,
        },
    }
}

/// Whether an open tab is already showing this exact comparison.
///
/// Keyed on **all four** of `(repo, path, new, old)`, and the asymmetry with
/// [`shows_git_diff`] — which deliberately leaves the side out — is the whole point rather than
/// an oversight.
///
/// A `Git` tab *switches sides in place*: the pane draws three buttons, and clicking one
/// re-reads the same file against a different pair of trees and clears the selection. The side
/// is therefore a **mode of one tab**, and keying on it would answer a second double-click with
/// a second tab over the same file — the duplicate-tab problem `tab_open_file` exists to avoid.
///
/// A `GitRevision` tab has no such control, because there is no such gesture: the pair **is**
/// the tab's identity. `main.rs` at `a1b2c3d` against its parent and `main.rs` at `a1b2c3d`
/// against `9f8e7d6` are two different documents that happen to share a file name, and a user
/// who opens both wants both — collapsing them onto one tab would silently discard the
/// comparison they asked for and replace it with one they did not.
fn shows_revision_diff(
    kind: &TabKind,
    repo: RepoId,
    path: &str,
    new: &RevSide,
    old: &RevSide,
) -> bool {
    matches!(
        kind,
        TabKind::Diff { spec, .. } if matches!(
            &spec.origin,
            DiffOrigin::GitRevision { repo: r, path: p, new: n, old: o }
                if *r == repo && p == path && n == new && o == old
        )
    )
}

/// Open a read-only diff of one file between two revisions, or activate the one already
/// showing it — the *double-click* half. (M18)
///
/// [`tab_open_diff`]'s twin, and every argument in that function's header applies here
/// unchanged: no diff text is accepted, because [`DiffSpec`] is persisted to `workspace.json`
/// and a saved diff is a lie; the pane re-reads through `git_diff_revision` with the key; and
/// nothing here touches the disk, so it is not `async`.
///
/// One thing is *better* here than for a working-tree diff, and it is worth stating because it
/// is the reason a revision tab can be restored with confidence: the read is **reproducible**.
/// Two commits diff to the same bytes for ever, so a tab saved at 10:00 and restored at 14:00
/// shows exactly what it showed. The one exception is a [`RevSide::WorkingTree`] side, which is
/// why that is a variant a caller can match on rather than a magic oid.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_revision_diff(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    new: RevSide,
    old: RevSide,
    old_path: Option<String>,
) -> Result<TabId> {
    state.update(|ws| {
        open_revision_diff(
            ws,
            project,
            repo,
            &path,
            new.clone(),
            old.clone(),
            old_path.clone(),
        )
    })
}

/// [`tab_open_revision_diff`] without Tauri. See [`open_git_diff`] for why it is split out.
#[allow(clippy::too_many_arguments)]
fn open_revision_diff(
    ws: &mut cide_ipc::Workspace,
    project: ProjectId,
    repo: RepoId,
    path: &str,
    new: RevSide,
    old: RevSide,
    old_path: Option<String>,
) -> Result<TabId> {
    let existing = workspace::project(ws, project)?
        .tabs
        .iter()
        .find(|t| shows_revision_diff(&t.kind, repo, path, &new, &old))
        .map(|t| t.id);
    if let Some(id) = existing {
        workspace::promote_diff(ws, project, id)?;
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let spec = revision_diff_spec(repo, path, new, old, old_path);
    let title = spec.title.clone();
    workspace::open_tab(
        ws,
        project,
        TabKind::Diff {
            spec,
            preview: false,
        },
        diff_pane(title),
    )
}

/// Point the tool window's preview diff tab at this comparison — the *single-click* half. (M18)
///
/// [`tab_retarget_diff`]'s twin, with the same three outcomes in the same order: a tab already
/// showing this comparison wins, then the preview tab, then a new preview tab. Walking a
/// commit's forty changed files with single clicks therefore costs one tab, not forty.
///
/// **A different scratch slot from the git panel's**, which is the part that is not merely a
/// copy. Both lists are on screen at once — the changes tree in the sidebar and the log's file
/// list in the tool window — so a shared slot would make a click in one throw away the other's
/// tab. See [`cide_core::workspace::PreviewSlot`], and the fourth refusal in
/// `cide_core::workspace::retarget_diff` which makes the two slots disjoint by construction.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_retarget_revision_diff(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    new: RevSide,
    old: RevSide,
    old_path: Option<String>,
) -> Result<TabId> {
    state.update(|ws| {
        retarget_revision_diff(
            ws,
            project,
            repo,
            &path,
            new.clone(),
            old.clone(),
            old_path.clone(),
        )
    })
}

/// [`tab_retarget_revision_diff`] without Tauri. See [`open_git_diff`] for why it is split out.
#[allow(clippy::too_many_arguments)]
fn retarget_revision_diff(
    ws: &mut cide_ipc::Workspace,
    project: ProjectId,
    repo: RepoId,
    path: &str,
    new: RevSide,
    old: RevSide,
    old_path: Option<String>,
) -> Result<TabId> {
    let showing = workspace::project(ws, project)?
        .tabs
        .iter()
        .find(|t| shows_revision_diff(&t.kind, repo, path, &new, &old))
        .map(|t| t.id);
    if let Some(id) = showing {
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let spec = revision_diff_spec(repo, path, new, old, old_path);
    if let Some(id) = workspace::preview_diff_tab(ws, project, PreviewSlot::Revision)? {
        workspace::retarget_diff(ws, project, id, spec)?;
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let title = spec.title.clone();
    workspace::open_tab(
        ws,
        project,
        TabKind::Diff {
            spec,
            preview: true,
        },
        diff_pane(title),
    )
}

// --- the revision tab (M18) --------------------------------------------------------------
//
// A *file* at one commit, read-only — not a comparison. The pair above answers "what changed
// here"; this answers "what did this file look like then", which is the question the blame
// gutter's second button actually asks and the one `cide_git::blame::blame_with` documents its
// `newest` argument as being about.
//
// Everything below mirrors `tab_open_diff` line for line, minus the preview slot: there is no
// single-click gesture that mints one of these, so there is nothing to retarget and no scratch
// tab to promote. Where the key deliberately differs from both of the pairs above —
// `shows_revision` — the difference is written down at the site.

/// How a revision tab is labelled: `log.rs @ a1b2c3d`.
///
/// The same seven characters and the same ` @ ` as [`revision_diff_spec`], deliberately: the two
/// kinds of tab sit in one strip, side by side, and a user cannot be asked to learn two
/// spellings of "this file at this commit". `chars().take(7)` and not `[..7]` for the reason
/// [`rev_label`] gives — an oid is hex so the two agree today, but slicing a `String` by byte
/// index panics on a non-ASCII boundary and this value arrives from the frontend. A short input
/// is returned whole rather than padded.
///
/// **Stored on the tab rather than derived at render time**, which is [`DiffSpec`]'s rule and is
/// worth repeating because the cost is invisible: a derived title changes the day the derivation
/// is improved and silently relabels every saved tab in every `workspace.json` on disk.
fn revision_title(path: &str, rev: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    format!("{name} @ {}", rev.chars().take(7).collect::<String>())
}

/// The pane a revision tab opens with.
///
/// [`PaneKind::Editor`] and not [`PaneKind::Diff`], because what it renders *is* an editor —
/// `RevisionPane` mounts `EditorSurface` with `readOnly`, so the buffer keeps Ctrl+F, folding,
/// syntax highlighting and the find bar. The kind is read in three places and all three want
/// that answer: `PaneBody` dispatches on it, `keys/context.ts` derives the `editorFocused`
/// context flag from it (which is what puts the editor's own commands in the palette), and
/// `windows/detachedPane.ts` reads it to know this pane runs no child and must never be handed
/// to `TerminalPane`.
///
/// It costs nothing on the other side: `close_pane`'s unsaved-changes guard consults
/// `unsaved_in_tab`, which only ever answers for a [`TabKind::File`], so an editor pane over a
/// tab that cannot be dirty is refused nothing. `cmd::settings` already opens its tab with an
/// editor pane over a kind with no path at all, for the same reason.
fn revision_pane(title: String) -> Pane {
    Pane {
        id: PaneId::new(),
        kind: PaneKind::Editor,
        // Auxiliary like every pane a tab is opened with: nothing here holds a conversation, so
        // there is nothing about it that must not be closed.
        role: PaneRole::Auxiliary,
        // No process, ever. A commit's bytes are a document.
        session: None,
        conversation: None,
        conversation_since: None,
        continues: None,
        harness: None,
        title,
        docker: None,
        origin: None,
    }
}

/// Whether an open tab is already showing this file at this revision.
///
/// Keyed on `(repo, path, rev)` and **deliberately not on `from`**, which is the one interesting
/// decision in this whole block.
///
/// `from` is the *route* the user took to get here — the trail of blame-the-parent hops — and two
/// walks that arrive at `a1b2c3d` by different routes are looking at the same bytes of the same
/// file. Keying on the route would answer the second walk with a second tab titled exactly like
/// the first, over identical content, which is the duplicate-tab problem `tab_open_file` exists
/// to avoid, with the added insult that the two are indistinguishable in the strip.
///
/// **The chain of the tab that already exists wins**, and that follows from the same reasoning
/// rather than being a separate rule. The existing tab's crumb strip is a route the user has
/// already walked and can see; replacing it with the newer arrival's route would rewrite a trail
/// under them — the crumbs they used to get somewhere would silently become crumbs they never
/// walked, and Back would lead out through a history they do not remember. The first route there
/// is the one they can retrace, so it is the one that is kept.
///
/// This is the mirror image of [`shows_revision_diff`]'s asymmetry with [`shows_git_diff`], and
/// for the mirror-image reason: there, the pair of revisions *is* the document, so it belongs in
/// the key; here, the route is not part of the document at all.
fn shows_revision(kind: &TabKind, repo: RepoId, path: &str, rev: &str) -> bool {
    matches!(
        kind,
        TabKind::Revision {
            repo: r,
            path: p,
            rev: v,
            ..
        } if *r == repo && p == path && v == rev
    )
}

/// Open a read-only tab showing one file as one commit left it, or activate the one already
/// showing it. (M18)
///
/// [`tab_open_diff`]'s cousin, and its header's arguments hold here unchanged: **no text is
/// accepted**, because `TabKind::Revision` is persisted inside `Workspace` and
/// `cide-core::persist` debounces that to `workspace.json` — a blob passed in here would be a
/// blob written to disk. The pane calls `git_file_at_revision` with `(repo, path, rev)` when it
/// mounts, exactly as a git diff calls `git_diff_file` with its key. And nothing here touches
/// the disk, so this is deliberately *not* `async`; the blocking object-database read happens in
/// `git_file_at_revision`, which is already on `spawn_blocking`.
///
/// One thing is *better* here than for a working-tree diff and it is why a revision tab can be
/// restored with confidence: the read is **reproducible**. A commit is immutable, so a tab saved
/// at 10:00 and restored at 14:00 shows the same bytes — which is also why `rev` must already be
/// a full oid by the time it reaches this function. `HEAD~3` names a different commit tomorrow;
/// `git_resolve_rev` is what callers put in front of anything a user typed.
///
/// `from` is the walk that led here — see [`cide_core::workspace::revision_chain`], which is
/// where the three rules about it live and where the merge that would otherwise grow it without
/// bound is written up.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_revision(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
    rev: String,
    from: Vec<String>,
) -> Result<TabId> {
    state.update(|ws| open_revision(ws, project, repo, &path, &rev, &from))
}

/// [`tab_open_revision`] without Tauri. See [`open_git_diff`] for why it is split out.
fn open_revision(
    ws: &mut cide_ipc::Workspace,
    project: ProjectId,
    repo: RepoId,
    path: &str,
    rev: &str,
    from: &[String],
) -> Result<TabId> {
    let existing = workspace::project(ws, project)?
        .tabs
        .iter()
        .find(|t| shows_revision(&t.kind, repo, path, rev))
        .map(|t| t.id);
    if let Some(id) = existing {
        // Activated and nothing else. No `promote_diff` — this is not a diff tab and that call
        // would report it as one — and, more importantly, no rewrite of `from`: see
        // `shows_revision` for why the chain the tab already has is the one that stands.
        workspace::activate_tab(ws, project, id)?;
        return Ok(id);
    }

    let title = revision_title(path, rev);
    workspace::open_tab(
        ws,
        project,
        TabKind::Revision {
            repo,
            path: path.to_owned(),
            rev: rev.to_owned(),
            from: workspace::revision_chain(rev, from),
            title: title.clone(),
        },
        revision_pane(title),
    )
}

/// Whether an open tab is already the resolver for this conflicted path.
///
/// Keyed on `(repo, path)` and nothing else, because that pair *is* the document: there is one
/// conflict per path in an operation, and a second tab over it would be two editable centre
/// panes writing the same file.
fn shows_merge(kind: &TabKind, repo: RepoId, path: &str) -> bool {
    matches!(kind, TabKind::Merge { repo: r, path: p, .. } if *r == repo && p == path)
}

/// Open the three-pane conflict resolver for one path, or activate the one already open. (M20)
///
/// [`tab_open_revision`]'s cousin, and the same rule holds for the same reason: **no text is
/// accepted**. `TabKind::Merge` is persisted inside `Workspace`, `cide_core::persist` debounces
/// that to `workspace.json`, and the three sides of a conflict passed in here would be three
/// documents written to disk. The pane calls `git_conflict_read` with `(repo, path)` when it
/// mounts, exactly as a revision pane calls `git_file_at_revision` with its key.
///
/// Nothing here checks that the path is *actually* conflicted. That is deliberate and it is the
/// same division `tab_open_diff` makes: this command opens a view, the pane asks the question,
/// and a pane that draws *"no longer conflicted"* is a better answer than a command that refuses
/// — because by the time a user clicks, another window may have resolved it, and a refusal here
/// would be a dead menu item with no explanation.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_open_merge(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    repo: RepoId,
    path: String,
) -> Result<TabId> {
    state.update(|ws| {
        let existing = workspace::project(ws, project)?
            .tabs
            .iter()
            .find(|t| shows_merge(&t.kind, repo, &path))
            .map(|t| t.id);
        if let Some(id) = existing {
            workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }
        let title = format!("{} — merge", path.rsplit('/').next().unwrap_or(&path));
        workspace::open_tab(
            ws,
            project,
            TabKind::Merge {
                repo,
                path: path.clone(),
                dirty: false,
            },
            revision_pane(title),
        )
    })
}

/// Record whether a file tab has unsaved edits.
///
/// The dirty flag lives in the Rust-owned tree rather than in the editor component because
/// it outlives the component: a tab switch unmounts nothing today, but a detached editor
/// window is a different JavaScript realm, and the tab strip that draws the dot is in the
/// other one.
///
/// It is also the guard. `cide_core::workspace::close_tab` refuses a tab carrying this flag
/// unless the caller passes `force`, `close_project` refuses a project holding one, and
/// `app_quit_requested` reports every one of them by name. Setting it therefore has a
/// consequence beyond the dot in the tab strip: an editor that stops reporting a clean save
/// leaves a tab the user can no longer close without a dialog, and one that stops reporting
/// a dirty buffer takes the guard down with it.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_set_dirty(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    dirty: bool,
) -> Result<Mutated> {
    state.update(|ws| {
        let t = workspace::tab_mut(ws, project, tab)?;
        let TabKind::File { dirty: flag, .. } = &mut t.kind else {
            return Err(CoreError::Invariant(format!(
                "tab {tab} is not a file tab and has no dirty state"
            )));
        };
        // Bumping `rev` on a no-op would broadcast a snapshot per keystroke: the editor
        // reports on every transaction, and only the transitions are news.
        if *flag == dirty {
            return Ok(Mutated { rev: ws.rev });
        }
        *flag = dirty;
        workspace::bump(ws);
        Ok(Mutated { rev: ws.rev })
    })
}

/// Every root of every open project, for the read-only rule below.
///
/// Every project, not the active one, and not a `project` argument on the commands: `file_read`
/// has never taken one, and the question these roots answer is not "which project is this file
/// in" but "has the user opened this directory at all". A file that is a project root in the
/// window behind this one is still the user's own code.
fn open_roots(state: &WorkspaceState) -> Vec<PathBuf> {
    state.with(|ws| {
        ws.projects
            .iter()
            .flat_map(|(_, project)| project.roots.iter().map(|root| root.path.clone()))
            .collect()
    })
}

/// Read an image file for an image pane — identity and a *permit*, never the pixels. (M18)
///
/// > *"need render images when opening them"*
///
/// # What used to happen
///
/// A file tab has always had exactly one reader: `document::read`, which refuses anything with
/// a NUL byte in its first 8 KiB. So `logo.png` opened a tab and the tab said
/// **"logo.png looks like a binary file"** — a refusal, not mojibake and not a crash, which is
/// the one piece of luck in the report. (`cide_core::image`'s
/// `a_png_is_refused_by_the_text_reader` pins that, so the sentence stays a fact.) SVG was the
/// exception and opened as XML source, because it is text.
///
/// # How the bytes reach the webview, and the two candidates that lost
///
/// **Tauri's asset protocol.** `convertFileSrc(path)` gives the pane an `asset://localhost/…`
/// URL that wry serves from `tauri::protocol::asset`: a real streaming response with `Range`
/// support, read off the webview's own thread, never marshalled and never copied through
/// Rust. A 40 MB PNG costs this command a `stat` and an 8 KiB header read.
///
/// * **A `data:` URI over the IPC** was the obvious one. Tauri's IPC is JSON; base64 inflates
///   by a third; and the string is built in Rust, parsed by the JSON reader, retained by the
///   JS engine and decoded again by the image decoder — over 100 MB of peak footprint for that
///   same 40 MB PNG, on the thread that also draws every terminal in the window. `CLAUDE.md`'s
///   rule about the control plane is written about `emit`/`listen`, and the reasoning
///   (interpolated JSON evaluated on the GTK main loop) is the same one.
/// * **A custom `cide-image://` protocol** would work and is what the asset protocol already
///   *is*. Registering a second one means a second CSP source to add, a second scope
///   implementation to get right, and a second answer to "how does a file reach the webview".
///
/// ## The CSP admits it, and here is how that was verified rather than assumed
///
/// `crates/cide-app/tauri.conf.json` sets
/// `img-src 'self' data: blob: asset: http://asset.localhost`, and enables
/// `app.security.assetProtocol` (the `protocol-asset` cargo feature is on in the workspace
/// manifest). Tauri's injected `convertFileSrc` — `tauri-2.11.5/scripts/core.js` — emits
/// `asset://localhost/<encoded>` on Linux and macOS and `http://asset.localhost/<encoded>` on
/// Windows, and **both** spellings are in that `img-src` list. A source the CSP forbids fails
/// silently in this engine, which is precisely why this paragraph names the file and the line
/// rather than saying "the CSP allows it".
///
/// # The permit, which is the part that is easy to leave out
///
/// `assetProtocol.scope` in the config is `[]` — *nothing* is servable — and it is left that
/// way on purpose. A static `["**"]` would turn the asset protocol into an unauthenticated
/// read of every file on the machine for the lifetime of the process, which is a much larger
/// grant than this feature needs and one no gesture would ever narrow again.
///
/// Instead the scope is widened here, one file at a time, **after** `cide_core::image::read`
/// has proved the path is a regular file under the cap whose bytes really are an image. So the
/// protocol can serve exactly the images the user has opened, and a path that failed any
/// refusal is never admitted at all. The cost is that the pattern list grows by one entry per
/// distinct image opened in a session; `Scope::is_allowed` is a linear walk over it, and a
/// user who opens a thousand images pays a thousand glob matches on each fetch, which is
/// nothing next to decoding the image.
///
/// This is a real widening and it is worth being honest about its size: a *compromised
/// webview* could already read any file through `file_read` beside this function, which has no
/// containment check at all and says so. What this adds is a second route to bytes the same
/// origin could already ask for, restricted to files a user opened. Containment against the
/// project roots is deliberately **not** applied, for `file_read`'s stated reason: an image
/// under *External Libraries*, or one the user approved by name through
/// `terminal_open_path`'s out-of-project dialog, is a file they asked for.
///
/// `spawn_blocking` for the same reason as `file_read`: a `canonicalize`, a `stat` and an
/// 8 KiB read on a path the user chose, which on a stalled NFS mount would otherwise take the
/// event loop and with it every terminal in the window.
#[tauri::command(rename_all = "camelCase")]
pub async fn image_read(app: tauri::AppHandle, path: String) -> Result<cide_ipc::ImageDoc> {
    let doc = blocking(move || cide_core::image::read(&PathBuf::from(path))).await?;

    // After the read, never before. `allow_file` is the capability grant, and granting it
    // ahead of the refusals would admit a directory, a FIFO or a 2 GB tarball to the protocol
    // on the strength of the user having clicked something.
    app.asset_protocol_scope()
        .allow_file(&doc.path)
        .map_err(|e| {
            CoreError::Io(format!(
                "{} could not be served to the viewer: {e}",
                doc.path.display()
            ))
        })?;

    Ok(doc)
}

/// Read a file for an editor pane.
///
/// # The read-only rule, and the bug it closes
///
/// `FileDoc::writable` was the file's mode bits and nothing else, which was correct for every
/// path this command could reach until M12 gave the editor Go to definition. That command opens
/// `~/.cargo/registry/src/index.crates.io-…/serde-1.0.229/src/lib.rs`, and cargo unpacks a crate
/// **mode 644** — so the buffer was editable, Ctrl+S went through `document::write`, and the edit
/// landed in the registry copy *every project on the machine* builds against. Cargo's checksum
/// verification then fails builds in repositories the user never touched, and `cargo clean` does
/// not undo it. Go's module cache is mode 444, so the same gesture there failed at `File::create`
/// and the user saw an error; Rust's being writable was the whole difference, and it was luck.
///
/// M13 makes that population one click away — every row under *External Libraries* is one of
/// these files — so the rule is fixed here rather than left to the group. It is deliberately about
/// the **dependency cache**, not about the group: a path dependency on a sibling crate is a row in
/// the group and is the user's own code, and Go to definition reaches a registry source whether or
/// not the group has ever been opened.
///
/// `cide_core::toolchain::read_only_reason` is the rule and a project root overrides it — see
/// there. This clears `writable`, which is what `EditorPane` turns into `readOnly` and what
/// `EditorSurface` turns into `EditorState.readOnly` plus `EditorView.editable.of(false)`: the
/// buffer cannot be typed into at all, so there is no silently-dropped Ctrl+S to explain.
#[tauri::command(rename_all = "camelCase")]
pub async fn file_read(state: State<'_, WorkspaceState>, path: String) -> Result<FileDoc> {
    let roots = open_roots(&state);
    let caches = cide_core::toolchain::dependency_roots();
    blocking(move || read_document(&PathBuf::from(path), &roots, &caches)).await
}

/// [`file_read`]'s body, over values.
///
/// Split out for the same reason `cmd::fs::index_project` is, and here the reason is sharper: the
/// only other way to exercise this composition — `document::read` over a *real* mode-644 file,
/// plus the rule — would be to write a file into the user's own `~/.cargo/registry`, which is
/// precisely what the rule exists to prevent. Taking the cache list as an argument lets the test
/// at the foot of this file build a cache of its own in a temporary directory.
pub(crate) fn read_document(path: &Path, roots: &[PathBuf], caches: &[PathBuf]) -> Result<FileDoc> {
    let mut doc = document::read(path)?;
    if let Some(reason) = cide_core::toolchain::read_only_reason_in(path, roots, caches) {
        tracing::debug!(%reason, "opening a dependency source read-only");
        doc.writable = false;
    }
    Ok(doc)
}

/// Write a buffer back.
///
/// The second lock on the rule [`file_read`] describes, and it is not redundant. `writable` is a
/// flag that travelled through the webview: it decides what the *editor* allows, which is the
/// right place for it, and it is not evidence about what the disk should accept. The house rule
/// throughout this file is that a claim coming back from the webview is re-checked in Rust — see
/// `openable` above, which makes the same argument about an approved out-of-project path — and a
/// pane that somehow held a stale writable buffer over a registry source would otherwise overwrite
/// it.
///
/// The refusal carries the sentence rather than a code, because the only useful thing to do with
/// it is show it.
/// # `ifUnchanged`, and which caller passes it
///
/// `None` writes unconditionally. That is what Ctrl+S passes, and it is the right thing for a
/// keystroke the user typed on purpose.
///
/// `Some(stamp)` refuses with [`CoreError::FileChanged`] when the file on disk is no longer the
/// one the buffer was read from. **Autosave** passes it, and the gap it closes is one autosave
/// creates: `EditorPane` raises its conflict bar from `cide://session-tool`, which a `sed -i`,
/// a `cargo fmt` or a `git checkout` never sends. Before autosave, clobbering one of those took
/// a deliberate Ctrl+S; with autosave-on-blur it takes switching to a terminal, running
/// `cargo fmt`, and clicking back into the editor — three things nobody decides to do.
///
/// The new stamp comes back on success so the buffer's token moves with the write, and the next
/// autosave compares against what *this* one produced rather than against what the file was
/// when the tab opened.
#[tauri::command(rename_all = "camelCase")]
pub async fn file_write(
    state: State<'_, WorkspaceState>,
    path: String,
    text: String,
    if_unchanged: Option<cide_ipc::FileStamp>,
) -> Result<Option<cide_ipc::FileStamp>> {
    let roots = open_roots(&state);
    let caches = cide_core::toolchain::dependency_roots();
    blocking(move || write_document(&PathBuf::from(path), &text, &roots, &caches, if_unchanged))
        .await
}

/// [`file_write`]'s body, over values. See [`read_document`].
pub(crate) fn write_document(
    path: &Path,
    text: &str,
    roots: &[PathBuf],
    caches: &[PathBuf],
    if_unchanged: Option<cide_ipc::FileStamp>,
) -> Result<Option<cide_ipc::FileStamp>> {
    write_document_bytes(path, text.as_bytes(), roots, caches, if_unchanged)
}

/// [`write_document`] over bytes: the rule, then the precondition, then the write. (M63)
///
/// One body for both roads, so a drawing saved as a PNG under a dependency cache is refused by
/// the same sentence a buffer is.
pub(crate) fn write_document_bytes(
    path: &Path,
    bytes: &[u8],
    roots: &[PathBuf],
    caches: &[PathBuf],
    if_unchanged: Option<cide_ipc::FileStamp>,
) -> Result<Option<cide_ipc::FileStamp>> {
    if let Some(reason) = cide_core::toolchain::read_only_reason_in(path, roots, caches) {
        return Err(CoreError::Io(reason));
    }
    // Checked **after** the read-only rule and before anything is written. Order matters only
    // for the message: a buffer over a registry source that also happens to have moved should
    // report the rule the user can act on.
    document::write_bytes_if_unchanged(path, bytes, if_unchanged)
}

/// The file's stamp right now, for a pane that follows the disk without re-reading it. (M63)
///
/// The drawing pane compares this against the stamp it read with, on every `cide://git-status`,
/// exactly as `EditorPane::recheckOnDisk` does — except that the editor answers the question by
/// re-reading the whole text through `file_read`, which for a two-megabyte `.excalidraw.png` on
/// every status event would be a needless read the user can feel. `None` is a filesystem that
/// would not answer, and the pane treats it as `FileStamp` documents: no precondition.
#[tauri::command(rename_all = "camelCase")]
pub async fn file_stat(path: String) -> Result<Option<cide_ipc::FileStamp>> {
    blocking(move || Ok(document::stamp_at(&PathBuf::from(path)))).await
}

/// Read a file's bytes for the drawing pane — the one command that answers bytes. (M63)
///
/// # The rule this is the exception to, and why it holds anyway
///
/// [`image_read`] above refuses to send pixels over the IPC, and `cide_ipc::image` states the
/// argument: Tauri's IPC is JSON, and a `Vec<u8>` through it is a decimal array or a base64
/// string, either of which parks a 40 MB PNG in three heaps on the thread that draws every
/// terminal. The asset protocol was the answer for an `<img>`, and it is **not** available
/// here: the pane needs the bytes as a `Blob` it can hand to Excalidraw's `loadFromBlob`, and
/// `fetch()` on `asset://` fails on Linux — wry registers the scheme as secure and not as
/// CORS-enabled, and the CSP's `connect-src` does not name it either.
///
/// What holds is the *reason* for the rule rather than its letter. `tauri::ipc::Response::new`
/// is neither JSON nor base64: it travels as `application/octet-stream` over the same custom
/// protocol the terminal's scrollback takes (`cmd::diag::diag_echo_bytes`, `session_attach`),
/// one round trip, no `eval`, and the webview receives an `ArrayBuffer`. The bytes are copied
/// once, into the frame.
///
/// # Why a frame and not a `Response` alone
///
/// A `Response` is a body and nothing else, and the pane needs the stamp the bytes were read
/// with — from the **same** `stat`, or a rewrite landing between two calls is a spurious
/// conflict bar on the next autosave. So the answer is `cide_ipc::frame`: a JSON
/// [`cide_ipc::FileBytesHead`] and the payload, in one buffer, split by four bytes of length.
///
/// The read-only rule is applied exactly as [`read_document`] applies it, for the same
/// registry-source reason; a drawing under `~/.cargo/registry` opens in view mode.
///
/// `blocking` for [`file_read`]'s reason — a stalled mount must not take the event loop.
#[tauri::command(rename_all = "camelCase")]
pub async fn file_read_bytes(
    state: State<'_, WorkspaceState>,
    path: String,
) -> Result<tauri::ipc::Response> {
    let roots = open_roots(&state);
    let caches = cide_core::toolchain::dependency_roots();
    blocking(move || {
        let path = PathBuf::from(path);
        let mut raw = document::read_bytes(&path)?;
        if let Some(reason) = cide_core::toolchain::read_only_reason_in(&path, &roots, &caches) {
            tracing::debug!(%reason, "opening a dependency source read-only");
            raw.writable = false;
        }
        let head = cide_ipc::FileBytesHead {
            writable: raw.writable,
            stamp: raw.stamp,
        };
        let head = serde_json::to_string(&head)
            .map_err(|e| CoreError::Io(format!("{} could not be framed: {e}", path.display())))?;
        let frame = cide_ipc::frame::pack(&head, &raw.bytes)
            .map_err(|e| CoreError::Io(format!("{}: {e}", path.display())))?;
        Ok(tauri::ipc::Response::new(frame))
    })
    .await
}

/// Write a file's bytes back — [`file_write`] for a drawing saved as PNG. (M63)
///
/// The request is a **raw body**, not JSON arguments: `invoke('file_write_bytes', bytes)` with a
/// `Uint8Array` arrives as `tauri::ipc::InvokeBody::Raw`, which is the request-side twin of the
/// `Response` [`file_read_bytes`] answers with, and the same argument applies. The path and the
/// precondition ride in the frame's head ([`cide_ipc::FileBytesWrite`]) rather than in request
/// headers, because a header value is visible ASCII and a path is not.
///
/// `async` with a borrowed `Request` is allowed by Tauri's macro only for a command returning a
/// `Result`, which this does; the body is copied into an owned frame before the blocking pool
/// takes it. Every rule [`write_document`] applies — the dependency-cache refusal, then the
/// stamp — is applied here through the same function.
#[tauri::command(rename_all = "camelCase")]
pub async fn file_write_bytes(
    state: State<'_, WorkspaceState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Option<cide_ipc::FileStamp>> {
    let (write, bytes) = bytes_request(request.body())?;
    let roots = open_roots(&state);
    let caches = cide_core::toolchain::dependency_roots();
    blocking(move || {
        write_document_bytes(
            &PathBuf::from(write.path),
            &bytes,
            &roots,
            &caches,
            write.if_unchanged,
        )
    })
    .await
}

/// [`file_write_bytes`]'s parse, over the body alone, so it can be driven without a webview.
///
/// A JSON body is refused by name rather than deserialised into something: a caller that reached
/// this command through `invoke(cmd, { path, bytes })` has put the bytes through exactly the road
/// this command exists to avoid, and the sentence should say so.
pub(crate) fn bytes_request(
    body: &tauri::ipc::InvokeBody,
) -> Result<(cide_ipc::FileBytesWrite, Vec<u8>)> {
    let raw = match body {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes,
        tauri::ipc::InvokeBody::Json(_) => {
            return Err(CoreError::Io(
                "file_write_bytes takes a raw body — a framed Uint8Array, never JSON arguments; \
                 see cide_ipc::frame"
                    .into(),
            ));
        }
    };
    let (head, payload) = cide_ipc::frame::unpack(raw)
        .map_err(|e| CoreError::Io(format!("file_write_bytes: {e}")))?;
    let write: cide_ipc::FileBytesWrite = serde_json::from_str(head).map_err(|e| {
        CoreError::Io(format!(
            "file_write_bytes: the frame head is not a write request: {e}"
        ))
    })?;
    Ok((write, payload.to_vec()))
}

/// Remember where the user is in a file. (M12)
///
/// **Deliberately infallible and deliberately not a workspace mutation.** Nothing here can fail
/// in a way the caller could act on — the state is a `Mutex<Vec<_>>` and the disk write happens
/// later, on the store's own debounce — and a scroll gesture that could reject would be a
/// scroll gesture that can raise a dialog. Compare `tab_set_dirty` above, which goes through
/// `WorkspaceState::update` and therefore bumps `rev` and broadcasts the whole tree to every
/// window: correct for a dirty dot, ruinous for something the editor reports as the user
/// scrolls. `positions_state.rs` has the whole ladder.
///
/// `touched_at` is filled in on this side, whatever the caller sent: it is the eviction key,
/// and a renderer's clock is not something the store should trust to order its own LRU.
#[tauri::command(rename_all = "camelCase")]
pub fn file_note_position(
    positions: State<'_, std::sync::Arc<crate::positions_state::PositionsState>>,
    at: cide_ipc::ViewPosition,
) {
    positions.note(at);
}

/// Where the user last was in `path`, or `null`.
///
/// Answered from Rust rather than from a per-webview cache, and that is the point: a detached
/// pane window is a separate JS realm with its own module instances, and two windows showing
/// one file must not race two private copies of its position. Rust owning it is also what makes
/// the memory survive a relaunch, which is the half of the report that a module-level map
/// cannot reach at all.
#[tauri::command(rename_all = "camelCase")]
pub fn file_position(
    positions: State<'_, std::sync::Arc<crate::positions_state::PositionsState>>,
    path: String,
) -> Option<cide_ipc::ViewPosition> {
    positions.get(Path::new(&path))
}

/// Everything the properties card's first block shows about a path. (M70)
///
/// One `symlink_metadata` and, for a plausible text file, one bounded read. Fast enough that the
/// card opens on it, which is the whole reason the directory walk and the git walk are separate
/// commands rather than fields of this answer: a card that waited for a recursive walk of
/// `target/` before drawing the name of the file would be a card nobody opens twice.
///
/// **No root containment check**, unlike [`read_document`] and deliberately: this answers only
/// *about* a path and never returns its contents, so there is nothing to leak, and the file
/// picker can open something outside every project. What it does refuse is an empty path, which
/// `symlink_metadata` would otherwise report as a puzzling "No such file or directory" about
/// nothing at all.
///
/// `blocking` for [`file_read`]'s reason — a stalled mount must not take the event loop.
#[tauri::command(rename_all = "camelCase")]
pub async fn file_properties(path: String) -> Result<cide_ipc::FileProperties> {
    let path = PathBuf::from(path);
    blocking(move || {
        cide_core::properties::check_path(&path)?;
        cide_core::properties::read(&path)
    })
    .await
}

/// What a directory contains: entry counts and the bytes they add up to. (M70)
///
/// Its own command because it is the slow half. A properties card draws the name, the mode and
/// the mtime from [`file_properties`] immediately and fills this row in when it lands, so a
/// right-click on `node_modules/` costs a spinner in one row rather than a card that does not
/// appear.
///
/// The walk is bounded and says so — `cide_core::properties::dir_summary`, and
/// [`cide_ipc::DirSummary`] argues why `truncated` is not optional. The short version is
/// `cide_fs::copy::preview_merge`'s: a count that stops early is honest when it says so and
/// simply wrong when it does not.
#[tauri::command(rename_all = "camelCase")]
pub async fn file_properties_dir(path: String) -> Result<cide_ipc::DirSummary> {
    let path = PathBuf::from(path);
    blocking(move || {
        cide_core::properties::check_path(&path)?;
        cide_core::properties::dir_summary(&path)
    })
    .await
}

/// Run a fallible blocking job on the pool, reporting a lost worker as an IO error.
///
/// The join can only fail if the task panicked or the runtime is shutting down. Neither is
/// something a caller can act on differently from a failed read, and inventing a variant
/// for it would put a case in the frontend's error switch that no test can reach.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tauri::async_runtime::spawn_blocking(job).await {
        Ok(result) => result,
        Err(error) => Err(CoreError::Io(format!("file worker failed: {error}"))),
    }
}

// There used to be a `claude_selection_changed` command here, registered in `lib.rs` and named
// in `contract/commands.json`, that `EditorPane` called on an 80 ms debounce from the editor's
// selection listener — every caret move, in every open file — and that **broadcast** the
// selection to every `claude` connected to the project. It was added because the addressed
// `IdeServer::selection_changed` had no producer, and the argument was that a selection is a
// fact about the editor rather than a message to one conversation. The chair disagreed: the
// CLI shows a received selection as `⧉ Selected N lines from <file>` in its prompt and sends
// the text as context with the next message, so reading a file while four agents worked put
// that file into all four conversations, and there was nothing on screen in the editor to say
// so. The command is gone rather than gated, because there is no reading of "the user looked
// at a file" that makes it a message to anybody. `claude_send_lines` below is the deliberate
// gesture — a context-menu row naming one session — and it now carries the selection as well as
// the mention, to that pane only. Deleting it is the same three-file change as the note below
// describes, so a reviewer sees the surface shrink.

// There used to be a `claude_mention_file` command here, registered in `lib.rs` and named in
// `contract/commands.json`, whose body was `claude_send_lines`'s second half with the result
// thrown away. **Nothing called it.** Its one frontend wrapper (`claude.mentionFile` in
// `ipc/client.ts`) ended in `.catch(() => {})` and had no call sites either: Ctrl+P's ⌥⏎ went
// to `claudeSend.lines` instead, precisely *because* a command that swallows its own failure
// is indistinguishable from a control wired to nothing.
//
// So it is gone rather than kept as a second, quieter route into the same server — which is
// the shape this project keeps finding: a complete, correct implementation reachable from no
// call site, drifting away from the one that ships. Deleting it is a three-file change
// (`lib.rs`, here, `contract/commands.json`) exactly so a reviewer sees the surface shrink.

/// Why *Send lines to Claude* could not send.
///
/// A dedicated error rather than `CoreError`, for the reason `SessionError` in `cmd::session`
/// is: neither of these is a domain refusal, and both of them are sentences a user has to
/// read. Tagged `{kind, message}` like every other error crossing this boundary, so the
/// frontend branches on the variant and `chrome/Failures.tsx` shows the prose.
///
/// **This type existing is the point of the change.** `claude_mention_file` above returns
/// `()`: every failure it can have — no IDE server, no connected CLI, the wrong pane — leaves
/// the frontend with a resolved promise and the user with a menu item that did nothing, which
/// is byte-for-byte how a control wired to nothing behaves.
#[derive(Debug, thiserror::Error)]
pub enum ClaudeSendError {
    /// The project has no IDE server. Either it failed to bind a port at startup, or this
    /// window is showing a project the app has since closed.
    #[error(
        "cide has no IDE server for this project, so nothing can be sent to Claude — see the \
         log (Help ▸ Open log folder) for why it did not start"
    )]
    NoServer,

    /// A server is running and **no** pane in the project has a `claude` on it.
    ///
    /// Not "that pane's `claude` is missing" any more, and the strengthening is the routing:
    /// [`claude_send_lines`] now walks every Claude pane in the project and stops at the first
    /// one a notification can actually reach, so getting here means every one of them was
    /// unreachable.
    ///
    /// The two wordings are deliberately different. Zero connections means the feature has
    /// never been reachable in this project and the user needs to start or connect a session;
    /// a non-zero count means CLIs are attached but not one of them is identified with a pane,
    /// which is a completely different thing to go looking for.
    #[error("{}", not_connected(*connections))]
    NotConnected { connections: usize },
}

/// The sentence for [`ClaudeSendError::NotConnected`].
///
/// A function rather than two `#[error]` attributes because the count decides the wording, and
/// `thiserror`'s format strings have no room to branch. Named so it is greppable from the
/// frontend, which shows this text verbatim.
///
/// # `connections` is not a count of *other* panes, and the prose must not say it is
///
/// [`cide_ide_mcp::Delivery::NoConnection`] carries every connection on this project's server,
/// including one sitting in the very pane that was addressed whose pid `pane_bind_session`
/// never bound — and that is the single likeliest way to reach this error, because it is what
/// happens to a `claude` a user started by hand inside a pane. An earlier wording said "one
/// other session in this project is [connected] — focus that pane", which sends the user
/// hunting for a second pane that in that case does not exist. So the count is reported as
/// what it is (sessions connected to this project) and the advice is the one action that fixes
/// every variant of the case: `/ide` in the pane they meant.
///
/// # The `/ide` advice used to be *usually* right and is now *exactly* right
///
/// Before the routing in [`claude_send_lines`], a non-zero count here had two possible
/// meanings: an unbound connection, or a perfectly good bound `claude` in some *other* pane
/// that the caller simply had not addressed. Only the first is fixed by `/ide`, and the second
/// was the common one — it is what the reported bug actually was. Now that every Claude pane
/// in the project is tried before this error is produced, a non-zero count can only mean that
/// none of the attached CLIs is identified with a pane, which is precisely the case `/ide`
/// repairs. The sentence did not have to change to become true; the code around it did.
///
/// # What a non-zero count means now (M29), and why the advice narrowed
///
/// It narrowed again, and this time the sentence *was* wrong for a whole class of user. A
/// `claude` behind a launcher — a version manager, an npm shim, a `bbin agent claude`-style
/// wrapper — announces its own pid and cide holds the wrapper's, so nothing bound it, so this
/// error fired; and `/ide` does not repair that, because reconnecting announces the same pid
/// again. The repair is [`crate::ide::resolve_unbound_connections`], which walks the announced
/// pid's ancestry before this error can be produced at all, and it runs on this command's own
/// path a few lines below.
///
/// So a non-zero count now means something narrower still: connected CLIs with **no ancestor
/// cide forked** — somebody's `claude` in a Terminal window that found this project's lockfile
/// — or a platform where [`cide_core::proc::PARENT_LOOKUP_WORKS`] is false. `/ide` is right for
/// the first (it is what makes a foreign CLI connect from the pane you meant) and is all
/// anybody can do about the second. The prose is unchanged; what changed is that far fewer
/// people can now reach it. The log line
/// `resolve_unbound_connections` writes on that path names the chain it walked and the
/// children this project has, which is what a report of this error should carry.
fn not_connected(connections: usize) -> String {
    match connections {
        0 => "No Claude session in this project is connected to cide's IDE server. Start a \
              Claude pane (or run /ide inside one) and try again."
            .to_string(),
        1 => "cide has no Claude bound to any pane in this project. One session is connected \
              to the IDE server, but nothing identifies which pane it belongs to — run /ide \
              in the pane you meant to send to."
            .to_string(),
        n => format!(
            "cide has no Claude bound to any pane in this project. {n} sessions are connected \
             to the IDE server, but nothing identifies which panes they belong to — run /ide \
             in the pane you meant to send to."
        ),
    }
}

/// Every Claude pane in a project that a mention could go to, best first.
///
/// # Why this list exists at all
///
/// The frontend names one pane and four separate call sites derive that name by the same
/// unwritten rule: *the focused pane if it is a Claude, otherwise `tabs[0]`'s first Claude
/// pane in map order*. Neither half of that rule asks whether the pane it picks has a `claude`
/// running, and it routinely picks one that does not: a Claude pane is only spawned eagerly
/// when it holds the project's primary session or has a transcript to resume
/// (`lifecycle::restore_for`), so every other Claude pane in a restored project sits at a
/// **resume splash with no process at all**. `tabs[0]`'s first pane in map order is very often
/// one of those, and the user's report — *"one session in this project is connected … but
/// nothing identifies it as that pane's"* — is that error, verbatim, with three live Claude
/// panes elsewhere in the same tab.
///
/// So the caller's pane is treated as a *preference*, not as an address, and the rest of this
/// list is what to try when the preference cannot receive.
///
/// # The order, and why each rung is where it is
///
/// 1. **`asked`.** Always first, so nothing changes for a send that was going to work. A
///    fallback that reordered a working case would be a behaviour change dressed as a fix.
/// 2. **The active tab's focused pane**, if it is a Claude one. This is the app's only
///    recency signal and it is a real one: `PaneTree::focused` is Rust-owned and moved by
///    `pane_focus`, so in an editor tab it names the last Claude pane the user actually
///    worked in. There is no cross-tab MRU anywhere in this app and none was invented for
///    this — see the note below.
/// 3. **Every other tab's focused pane**, `tabs[0]` first. Same signal, one tab further out.
/// 4. **`tabs[0]`'s [`PaneRole::Primary`] pane.** The project's durable default: `tabs[0]` is
///    always the pinned console and its primary pane cannot be closed.
///
///    Deliberately **not** `Project::primary_session`, which is the field this looks like it
///    should use. That id is written once when the project is created and never re-pointed
///    when a pane respawns fresh, so in a workspace that has been used it commonly names a
///    session **no pane holds and no transcript exists for** — routing to it would address
///    nothing at all. The pane is the durable thing; the session id on it is not.
/// 5. **Everything else**, tab order then detached panes, so a project whose console has been
///    rearranged still finds its remaining conversations.
///
/// # What this deliberately is not
///
/// Not a most-recently-used *store*. A store would need a subscription, a coalescer and a
/// staleness window to hold an ordering that `PaneTree::focused` already holds durably and for
/// free. If a true cross-tab MRU is ever wanted, the one-field version is a
/// `Project::last_claude_pane` written in `pane_focus` — a DTO field and a codegen run, in the
/// layer that owns durable state, not a mirror in the webview.
///
/// And not a connectivity check: this is a pure function of the workspace so it can be tested
/// without an `AppHandle`. Which of these panes can actually receive is asked of the IDE
/// server by the caller, at the moment it sends.
fn mention_candidates(ws: &cide_ipc::Workspace, project: ProjectId, asked: PaneId) -> Vec<PaneId> {
    let mut out: Vec<PaneId> = vec![asked];
    let Ok(p) = workspace::project(ws, project) else {
        return out;
    };

    let push = |pane: PaneId, out: &mut Vec<PaneId>| {
        if !out.contains(&pane) {
            out.push(pane);
        }
    };
    let is_claude = |tab: &cide_ipc::Tab, pane: &PaneId| {
        tab.tree
            .panes
            .get(pane)
            .is_some_and(|p| p.kind == PaneKind::Claude)
    };

    // The focused pane of the active tab, then of every other tab in strip order. `tabs[0]`
    // is the pinned console and comes first among the rest, which `Vec::contains` above makes
    // idempotent when the active tab *is* the console.
    let active = p.tabs.iter().find(|t| t.id == p.active_tab);
    for tab in active.into_iter().chain(p.tabs.iter()) {
        if is_claude(tab, &tab.tree.focused) {
            push(tab.tree.focused, &mut out);
        }
    }

    // The console's primary pane: the project's default conversation, named by the pane that
    // cannot be closed rather than by a session id that may name nothing.
    if let Some(console) = p.tabs.first()
        && let Some(primary) = console
            .tree
            .panes
            .values()
            .find(|pane| pane.kind == PaneKind::Claude && pane.role == PaneRole::Primary)
    {
        push(primary.id, &mut out);
    }

    for tab in &p.tabs {
        for pane in tab.tree.panes.values() {
            if pane.kind == PaneKind::Claude {
                push(pane.id, &mut out);
            }
        }
    }
    for pane in p.detached.values() {
        if pane.kind == PaneKind::Claude {
            push(pane.id, &mut out);
        }
    }

    out
}

/// A pane's title, wherever in the project it lives.
///
/// Used only to name the destination in the sentence the frontend shows when a mention did
/// not go where it was aimed. Falls back to the id — an unreadable but unambiguous name — for
/// the pane that vanished between the send and this lookup, because a blank in that sentence
/// would be worse than an ugly one.
fn pane_title(ws: &cide_ipc::Workspace, project: ProjectId, pane: PaneId) -> String {
    workspace::project(ws, project)
        .ok()
        .and_then(|p| {
            p.tabs
                .iter()
                .find_map(|t| t.tree.panes.get(&pane))
                .or_else(|| p.detached.get(&pane))
                .map(|p| p.title.clone())
        })
        .unwrap_or_else(|| pane.to_string())
}

impl serde::Serialize for ClaudeSendError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let kind = match self {
            Self::NoServer => "noServer",
            Self::NotConnected { .. } => "notConnected",
        };
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("ClaudeSendError", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

/// The `@`-mention for a codex console, typed rather than sent. (M93) `None` when `pane` is not
/// a codex console, which leaves the IDE road to answer.
fn mention_into_codex(
    app: &tauri::AppHandle,
    state: &WorkspaceState,
    project: cide_ipc::ProjectId,
    pane: cide_ipc::PaneId,
    path: &str,
    line_start: Option<u32>,
    line_end: Option<u32>,
) -> Option<std::result::Result<ClaudeSendTarget, ClaudeSendError>> {
    let (session, root, title) = state.with(|ws| {
        let project_ref = cide_core::workspace::project(ws, project).ok()?;
        let target = project_ref
            .tabs
            .iter()
            .flat_map(|t| t.tree.panes.values())
            .chain(project_ref.detached.values())
            .find(|p| p.id == pane)?;
        (target.kind == cide_ipc::PaneKind::Claude
            && cide_core::workspace::pane_harness(target) == cide_ipc::Harness::Codex)
            .then(|| {
                (
                    target.session,
                    project_ref.roots.first().map(|r| r.path.clone()),
                    target.title.clone(),
                )
            })
    })?;
    let pty = session.and_then(|session| {
        app.try_state::<crate::state::SessionRegistry>()?
            .get(session)
            .filter(|pty| !pty.has_exited())
    });
    let Some(pty) = pty else {
        // A codex pane with no live child: nothing can receive the mention, and "not connected"
        // is the honest sentence the webview already has.
        return Some(Err(ClaudeSendError::NotConnected { connections: 0 }));
    };
    pty.write(codex_mention(path, root.as_deref(), line_start, line_end));
    Some(Ok(ClaudeSendTarget {
        pane,
        title,
        fallback: false,
    }))
}

/// `@path` — relative to the project root when the file is inside it, as codex's own file
/// picker writes it — with the lines after it, as a bracketed paste and nothing more.
fn codex_mention(
    path: &str,
    root: Option<&std::path::Path>,
    line_start: Option<u32>,
    line_end: Option<u32>,
) -> Vec<u8> {
    let shown = root
        .and_then(|root| std::path::Path::new(path).strip_prefix(root).ok())
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let lines = match (line_start, line_end) {
        (Some(a), Some(b)) if a != b => format!(" (lines {a}-{b})"),
        (Some(a), _) => format!(" (line {a})"),
        _ => String::new(),
    };
    let mut bytes = b"\x1b[200~".to_vec();
    bytes.extend_from_slice(format!("@{shown}{lines} ").as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// *Send lines to Claude* — the editor's gesture, reported when it cannot land.
///
/// # What the gesture does, and why this shape
///
/// Two notifications, in this order, **both addressed to the same pane**:
///
/// 1. `selection_changed`, which gives that `claude` the selected text — the CLI shows it as
///    `⧉ Selected N lines` and sends it as context with the next message.
/// 2. `at_mentioned`, which puts `@path#L10-20` into that conversation's prompt.
///
/// The first used to be a broadcast, sent before the pane was even resolved, on the theory
/// that a status line in every other conversation should agree with the one being mentioned.
/// That made the one deliberate gesture in this file type into every conversation in the
/// project, which is the failure `cide_ide_mcp::server`'s module doc names. Both go to the
/// resolved target now, after resolution, so the pane that gets the mention is the pane that
/// gets the text and nobody else gets either.
///
/// The alternative that lost was pasting the selected *text* into the prompt. It sounds more
/// direct and is worse: a mention is what the protocol offers, it costs the agent one read of
/// a range it can widen at will, and pasting forty lines of source into a prompt box is a
/// gesture the user cannot undo and Claude cannot see the surroundings of. `claudeTasks
/// ::explainSelection` already exists for the case where the *text* is the point.
///
/// # Which pane it goes to
///
/// `pane` is the pane the gesture was *aimed* at, and it is tried first. When its `claude`
/// is not on the IDE server — most often because that pane is showing a resume splash and has
/// no process at all — the mention goes to the best other Claude pane in the same project that
/// can receive it. [`mention_candidates`] is the ordering and states the reasoning; the answer
/// says which pane was used and whether that was the asked-for one.
///
/// **Unless `exact`**, which turns the preference into an address. It is set by the editor
/// menu's per-session rows — the ones that read `2: git-details` — where the pane is not a
/// guess the router may improve on but a conversation the user picked by name. See the
/// comment on the routing below, which is where the argument for each half is written out.
///
/// # Why a fallback is safe here, and would not be elsewhere
///
/// An `at_mentioned` **is not a turn**. It types `@path#L10-20` into a prompt box; nothing is
/// submitted, nothing is edited, and the user can delete it. Contrast `openDiff`, which blocks
/// an agent turn on an answer — nothing near that gets a fallback. Four properties keep this
/// one honest, and removing any of them makes it a bad idea:
///
/// * **Same project only.** The IDE server is per project and token-gated, so a fallback
///   cannot cross into another project's conversations even in principle.
/// * **The asked-for pane always wins** when it can receive. The fallback only fires where
///   today's behaviour is a hard error.
/// * **Only *connected* panes are candidates** — never "some plausible pane". A pane with no
///   `claude` on the wire is not in the list at all.
/// * **The user is taken to where it landed and told.** The frontend reveals the returned
///   pane and, when `fallback` is set, says which conversation got the lines. A selection that
///   arrives in a prompt the user cannot see is exactly as useful as one that never arrived,
///   and one that arrives in the *wrong* visible prompt is worse than either.
///
/// # Line numbers
///
/// 1-based in, 0-based on the wire, converted here exactly once, at the boundary. `None` for
/// both means the whole file, so the caret
/// sitting in a buffer mentions the file rather than one arbitrary line. Verified against
/// `cide_ide_mcp::protocol::AtMentioned`, which documents the wire as 0-based inclusive and
/// omits an absent bound rather than sending `null` (the CLI validates against a schema where
/// those fields are optional but not nullable).
///
/// # Not `spawn_blocking`
///
/// It does no I/O. Both notifications are a `serde_json::to_string` and a push into an
/// unbounded in-memory channel that a connection task drains; the socket write happens on the
/// IDE runtime, not here. The workspace snapshot the routing reads is one uncontended lock and
/// a clone, which is what every other command in this file already does. `file_read` and
/// `file_write` above are the commands that touch a disk and they are the ones that go through
/// the pool.
#[tauri::command(rename_all = "camelCase")]
// Nine, and every one of them is a value the webview has to send: two managed-state handles,
// the project, the pane, the path, the text, a line range and whether the pane was chosen by
// name. A Tauri command's arguments are its wire shape, so grouping them into a struct would
// be a DTO to keep in step with the frontend for no gain — the same call `pane_split` and
// `session_spawn` beside it make.
#[allow(clippy::too_many_arguments)]
pub fn claude_send_lines(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: cide_ipc::ProjectId,
    pane: cide_ipc::PaneId,
    path: String,
    text: String,
    line_start: Option<u32>,
    line_end: Option<u32>,
    exact: bool,
) -> std::result::Result<ClaudeSendTarget, ClaudeSendError> {
    let servers = app
        .try_state::<crate::ide::IdeServers>()
        .ok_or(ClaudeSendError::NoServer)?;

    // The wire's 0-based numbering, applied once, to both notifications from the same source
    // values — so the range Claude highlights and the range it is told about cannot disagree.
    // A codex console (M93) has no IDE connection to receive a mention over — codex speaks no
    // `claude --ide` protocol — so the mention is **typed into its composer** as codex's own
    // `@path` reference, as a paste and without an Enter: what the IDE road does for claude,
    // which inserts and does not submit. Only for the pane the user aimed at; the fallback below
    // chooses among IDE-connected panes, which a codex pane never is.
    if let Some(sent) = mention_into_codex(&app, &state, project, pane, &path, line_start, line_end)
    {
        return sent;
    }

    let start = line_start.map(|l| l.saturating_sub(1));
    let end = line_end.map(|l| l.saturating_sub(1));

    // Before asking who can receive, work out who the connected CLIs *are*. A `claude` behind
    // a launcher announces its own pid rather than the wrapper's, which is the pid this pane
    // bound, so without this the answer below is empty on exactly the machines the feature was
    // reported broken on. Ordinarily a no-op: the same resolution ran when the connection
    // arrived, and this only catches the one that beat its pane's binding. See
    // `ide::resolve_unbound_connections`.
    crate::ide::resolve_unbound_connections(&app, project);

    let ws = state.snapshot();
    let reachable: std::collections::HashSet<String> = servers
        .addressable_panes(project)
        .ok_or(ClaudeSendError::NoServer)?
        .into_iter()
        .collect();

    /*
     * The asked-for pane when nothing is reachable, so the error names the pane the user
     * actually aimed at and the server's own dropped-notification log line says the same
     * thing. `at_mentioned` below then produces the connection count that decides the wording.
     *
     * `exact` collapses the list to that one pane, and it is the whole difference between the
     * two gestures that reach this command. ⌥⏎ and the editor menu's parent row *guess* a
     * destination — `useMentionTarget` picks the focused Claude pane or the console's — so a
     * guess that cannot receive should be corrected rather than refused, which is what the
     * ordering in `mention_candidates` is for. The menu's per-session rows do not guess: the
     * user read `2: git-details` and chose it. Rerouting *that* to a different conversation
     * would answer a question nobody asked, and the fallback's own justification above ("the
     * asked-for pane always wins when it can receive; the fallback only fires where today's
     * behaviour is a hard error") stops holding the moment the pane is a stated choice.
     *
     * A refusal is the honest answer there, and it is an actionable one: `NotConnected` says
     * to run `/ide` in the pane that was meant, which for a pane sitting at a resume splash is
     * exactly what has to happen before it can ever receive.
     */
    let target = if exact {
        pane
    } else {
        mention_candidates(&ws, project, pane)
            .into_iter()
            .find(|candidate| reachable.contains(&candidate.to_string()))
            .unwrap_or(pane)
    };

    if let (Some(start), Some(end)) = (start, end) {
        // To `target`, never to anyone else, and only once the target is known — a
        // selection sent before resolution went to a pane the mention then did not. Its
        // delivery is deliberately not checked: it reaches exactly the socket the mention
        // below reaches, and the mention is the half that reports. Failing the gesture
        // because the *text* did not land would refuse a mention that was about to work,
        // and reporting it separately would be two sentences about one socket.
        servers.selection_changed(
            project,
            target,
            cide_ide_mcp::SelectionChanged {
                file_path: path.clone(),
                text,
                start_line: start,
                end_line: end,
            },
        );
    }

    match servers.at_mentioned(
        project,
        target,
        cide_ide_mcp::AtMentioned {
            file_path: path,
            line_start: start,
            line_end: end,
        },
    ) {
        None => Err(ClaudeSendError::NoServer),
        Some(cide_ide_mcp::Delivery::NoConnection { connections }) => {
            Err(ClaudeSendError::NotConnected { connections })
        }
        // `Sent` from a pane that was not asked for is still reported as success — it is one,
        // the lines are in a prompt — but the answer carries enough for the caller to reveal
        // that pane and name it. Losing either half here turns a helpful reroute into a
        // selection that silently went somewhere else.
        Some(cide_ide_mcp::Delivery::Sent) => Ok(ClaudeSendTarget {
            pane: target,
            title: pane_title(&ws, project, target),
            fallback: target != pane,
        }),
    }
}

/// What the user has named each running conversation, keyed by the id cide addresses it under.
///
/// The labels behind the editor menu's *Send … to Claude ▸* submenu. Every Claude pane in a
/// project carries the same `Pane::title` — `cide : claude` — so a submenu built from titles
/// alone offers four identical lines; this is the one fact that tells them apart, and it is a
/// fact only the CLI holds. [`cide_claude::roster`] is the reader and its module doc is where
/// the bargain with an undocumented directory is written down.
///
/// # Why the whole machine rather than this project's sessions
///
/// The caller matches by conversation id against `Pane::conversation ?? Pane::session`, both of
/// which cide minted itself, so a name for a conversation in another project simply never
/// matches anything and costs a map entry. Filtering here would mean answering for symlinked
/// roots, multi-root projects and a session started in a subdirectory — three ways to lose a
/// name the user can see in their own terminal, in exchange for nothing.
///
/// # Why it is polled rather than pushed
///
/// There is no event: `/rename` is typed into a CLI that tells cide nothing, and the only
/// signal is a file rewritten under `~/.claude/sessions`. A watcher on that directory would
/// fire on every status change of every session on the machine — the records carry a `status`
/// and it moves several times a turn — to keep a string that is read at most once per context
/// menu. So the frontend asks when it is about to need the answer; see
/// `ui/src/editor/claudeNames.ts`, which owns the refresh policy and says what it costs.
///
/// # Why a name is dated before it is answered
///
/// The CLI's name belongs to the **process**, not to the conversation: `/rename` sets it on a
/// per-process singleton, and `/clear` starts a fresh conversation inside that same `claude`
/// and rewrites its record with the new `sessionId` and the old `name` still attached. Passing
/// the roster straight through therefore handed the name the user gave one conversation to the
/// one that replaced it, which is the reported *"`/clear` doesn't reset the session fully"*.
///
/// So each name arrives with the `nameSince` it was given at, `claude_name_cutoffs` says when
/// each pane arrived on the conversation it is on, and anything not later than its cutoff is
/// dropped here. The wire type stays `id -> name`, because the timestamp is evidence for this
/// decision and nothing the frontend has any use for.
///
/// # Not `spawn_blocking`
///
/// It reads a directory of small files — eight of them on the machine this was written on, one
/// `read_dir` and a `read_to_string` each. That is the same order of I/O as `file_position`
/// above, which is also synchronous, and an order less than `file_read`, which is not. The
/// call is made from a context menu opening, so a hop onto the pool would cost more in
/// scheduling than the read costs in total.
#[tauri::command(rename_all = "camelCase")]
pub fn claude_session_names(
    state: State<'_, WorkspaceState>,
) -> std::collections::HashMap<String, String> {
    let cutoffs = state.with(cide_core::workspace::claude_name_cutoffs);
    let mut names: std::collections::HashMap<String, String> = cide_claude::roster::names()
        .into_iter()
        .filter(|(id, named)| cutoffs.get(id).is_none_or(|cutoff| named.since > *cutoff))
        .map(|(id, named)| (id, named.name))
        .collect();
    // A codex console's name is its thread's (M93): codex titles a thread itself and `/rename`
    // rewrites it, both into `$CODEX_HOME/session_index.jsonl`, keyed by the thread id — which is
    // the conversation id the pane holds, so the webview's lookup finds it where it finds a
    // claude name. Only the threads open in a pane are read out of the index.
    let threads: Vec<String> = state.with(|ws| {
        ws.projects
            .values()
            .flat_map(|p| {
                p.tabs
                    .iter()
                    .flat_map(|t| t.tree.panes.values())
                    .chain(p.detached.values())
            })
            .filter(|p| cide_core::workspace::pane_harness(p) == cide_ipc::Harness::Codex)
            .filter_map(|p| p.conversation.map(|c| c.to_string()))
            .collect()
    });
    if !threads.is_empty()
        && let Some(home) = cide_core::codex_cli::codex_home()
    {
        let index = cide_core::codex_cli::thread_names(&home);
        for thread in threads {
            if let Some(name) = index.get(&thread) {
                names.insert(thread, name.clone());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole of what Back does with the closed-tab stack, driven.
    ///
    /// This is the decision the feature was designed around and it is the one that is invisible
    /// when it goes wrong: a record spent here is a Ctrl+Shift+T *later* that opens a tab the
    /// user did not ask for, with the split and the pane id naming a live `claude` gone for
    /// good. Nothing on screen reports it at the moment it happens, which is why it is a rule
    /// with a test rather than three arms in a Tauri command.
    #[test]
    fn back_spends_a_closed_tab_record_only_when_it_actually_puts_the_tab_back() {
        use crate::cmd::project::Reopen;

        assert_eq!(
            reopen_file_step(true, Some(Reopen::Reinsert)),
            ReopenFile::Restore,
            "the tab is coming back with its pane tree, so the record is honoured — and spent"
        );

        // The three that must NOT spend it. `Show` and `Skip` mean the tab is open already, so
        // the press is an activation and the record is still the right answer for a Ctrl+Shift+T
        // after the user closes that tab again; `None` means there was never a record at all.
        assert_eq!(
            reopen_file_step(true, Some(Reopen::Show(TabId::new()))),
            ReopenFile::Open,
            "a tab that is open is shown, and its record stays where it is"
        );
        assert_eq!(
            reopen_file_step(true, Some(Reopen::Skip)),
            ReopenFile::Open,
            "and so is the one that is open AND active — `open_file_tab` finds it either way"
        );
        assert_eq!(
            reopen_file_step(true, None),
            ReopenFile::Open,
            "with no record, Back is an ordinary open"
        );

        /*
         * The stat outranks the stack, and this is the row that says a record is not burned on
         * the way to discovering the file is gone. Note the input: `reopen_plan` answers `Skip`
         * for a deleted file — which is right for Ctrl+Shift+T, whose loop moves on to the next
         * record, and wrong here, where the user named *this* file. `Reinsert` is driven too,
         * because it is what a record for a live path answers and the version that consulted the
         * plan first would restore a tab over nothing.
         */
        for plan in [None, Some(Reopen::Skip), Some(Reopen::Reinsert)] {
            assert_eq!(
                reopen_file_step(false, plan),
                ReopenFile::Gone,
                "a file that is not there is reported, whatever the stack remembers about it"
            );
        }
    }

    fn spec(path: &str, old: Option<&str>) -> DiffSpec {
        git_diff_spec(
            RepoId::new(),
            path,
            DiffSide::Combined,
            old.map(str::to_owned),
        )
    }

    #[test]
    fn a_git_diff_tab_is_titled_by_its_basename() {
        assert_eq!(
            spec("crates/cide-git/src/patch.rs", None).title,
            "patch.rs — diff"
        );
        // A path with no separator is its own basename. `rsplit` always yields at least one
        // item, which is why the fallback in `git_diff_spec` can never actually fire — it is
        // there so the reader does not have to prove that.
        assert_eq!(spec("README.md", None).title, "README.md — diff");
    }

    /// The fetch key is what `git_diff_file` is called with, so it must stay repo-relative
    /// and must not pick up the display paths' shape.
    #[test]
    fn the_origin_carries_the_repo_relative_path_and_no_content() {
        let repo = RepoId::new();
        let s = git_diff_spec(repo, "src/main.rs", DiffSide::Unstaged, None);
        let DiffOrigin::Git {
            repo: r,
            path,
            side,
        } = s.origin
        else {
            panic!("a git diff must carry a git origin");
        };
        assert_eq!(r, repo);
        assert_eq!(path, "src/main.rs");
        assert_eq!(side, DiffSide::Unstaged);
    }

    /// A rename shows both names; everything else shows one path twice, which is what
    /// `DiffPane`'s path label collapses to a single label.
    #[test]
    fn a_rename_keeps_both_sides() {
        let renamed = spec("src/new.rs", Some("src/old.rs"));
        assert_eq!(renamed.old_path, PathBuf::from("src/old.rs"));
        assert_eq!(renamed.new_path, PathBuf::from("src/new.rs"));

        let plain = spec("src/main.rs", None);
        assert_eq!(plain.old_path, plain.new_path);
    }

    /// Reuse ignores the side and the display paths, and never matches another repository's
    /// file of the same name — the case a path-keyed lookup gets wrong in a monorepo.
    #[test]
    fn reuse_is_keyed_on_the_repository_and_the_path_alone() {
        let repo = RepoId::new();
        let other = RepoId::new();
        let tab = TabKind::Diff {
            spec: git_diff_spec(repo, "src/main.rs", DiffSide::Combined, None),
            preview: false,
        };

        assert!(shows_git_diff(&tab, repo, "src/main.rs"));
        assert!(!shows_git_diff(&tab, other, "src/main.rs"));
        assert!(!shows_git_diff(&tab, repo, "src/other.rs"));

        // A Claude diff over the same file is a different tab: it is holding an agent turn
        // open, and answering it is not the same act as staging.
        let claude = TabKind::Diff {
            spec: DiffSpec {
                title: "main.rs".into(),
                old_path: PathBuf::from("src/main.rs"),
                new_path: PathBuf::from("src/main.rs"),
                origin: DiffOrigin::ClaudeMcp {
                    request_id: "r1".into(),
                },
            },
            preview: false,
        };
        assert!(!shows_git_diff(&claude, repo, "src/main.rs"));
    }

    /// A workspace with one project and nothing but its pinned console.
    fn bare() -> (cide_ipc::Workspace, ProjectId) {
        let mut ws = cide_ipc::Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec![PathBuf::from("/repo")], None).expect("opens");
        (ws, project)
    }

    /// Every git diff tab in strip order, as (path, preview).
    fn diff_tabs(ws: &cide_ipc::Workspace, project: ProjectId) -> Vec<(String, bool)> {
        workspace::project(ws, project)
            .expect("exists")
            .tabs
            .iter()
            .filter_map(|t| match &t.kind {
                TabKind::Diff { spec, preview } => match &spec.origin {
                    DiffOrigin::Git { path, .. } => Some((path.clone(), *preview)),
                    // Neither of these is a *working-tree* diff, which is the only thing the
                    // preview-slot rule this helper drives is about. A revision diff has its own
                    // preview slot for exactly that reason — see `PreviewSlot` — so counting one
                    // here would make the working-tree assertions below fail whenever a commit's
                    // file list happened to be open beside them.
                    DiffOrigin::GitRevision { .. }
                    | DiffOrigin::ClaudeMcp { .. }
                    | DiffOrigin::GitLab { .. } => None,
                },
                _ => None,
            })
            .collect()
    }

    /// The bug report, as a loop, because that is how the user hit it.
    ///
    /// > *"in git files tree when i do one click on element - we should select it, but not
    /// > open the diff. Only when diff is already opened one click should change current
    /// > diff to selected file."*
    ///
    /// Thirty single clicks down a changelist used to be thirty calls to `tab_open_diff`,
    /// whose reuse is keyed on `(repo, path)` — so every row after the first missed and
    /// opened a tab. One tab now, showing the last file clicked.
    #[test]
    fn thirty_single_clicks_leave_one_diff_tab() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        let before = workspace::project(&ws, project).expect("exists").tabs.len();

        for i in 0..30 {
            retarget_git_diff(
                &mut ws,
                project,
                repo,
                &format!("src/file{i}.rs"),
                DiffSide::Combined,
                None,
            )
            .expect("retargets");
        }

        assert_eq!(
            diff_tabs(&ws, project),
            vec![("src/file29.rs".into(), true)]
        );
        assert_eq!(
            workspace::project(&ws, project).expect("exists").tabs.len(),
            before + 1,
            "thirty clicks may add one tab, and only on the first of them"
        );
    }

    /// The same loop with a tab the user opened on purpose already up.
    ///
    /// The kept tab is untouched — that is what double-click bought — and the thirty clicks
    /// still share a single scratch tab between them. Two tabs, not thirty-one.
    #[test]
    fn a_double_clicked_tab_survives_thirty_single_clicks() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        open_git_diff(
            &mut ws,
            project,
            repo,
            "src/keep.rs",
            DiffSide::Combined,
            None,
        )
        .expect("opens");

        for i in 0..30 {
            retarget_git_diff(
                &mut ws,
                project,
                repo,
                &format!("src/file{i}.rs"),
                DiffSide::Combined,
                None,
            )
            .expect("retargets");
        }

        // Strip order, and it is the reverse of the order they were opened in: `open_tab`
        // inserts each new tab immediately right of the pinned console, so the scratch tab —
        // opened on the first of the thirty clicks — sits in front of the kept one. What the
        // test is about is unchanged: two tabs, and the double-clicked one still there.
        assert_eq!(
            diff_tabs(&ws, project),
            vec![
                ("src/file29.rs".into(), true),
                ("src/keep.rs".into(), false),
            ]
        );
    }

    /// Clicking a file that is already on screen activates its tab instead of dragging the
    /// scratch slot onto a duplicate — which would cost the user whatever was in it.
    #[test]
    fn clicking_a_file_that_is_already_open_activates_it() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        let kept = open_git_diff(
            &mut ws,
            project,
            repo,
            "src/keep.rs",
            DiffSide::Combined,
            None,
        )
        .expect("opens");
        let preview =
            retarget_git_diff(&mut ws, project, repo, "src/a.rs", DiffSide::Combined, None)
                .expect("retargets");

        let again = retarget_git_diff(
            &mut ws,
            project,
            repo,
            "src/keep.rs",
            DiffSide::Combined,
            None,
        )
        .expect("retargets");

        assert_eq!(again, kept);
        assert_eq!(
            workspace::project(&ws, project).expect("exists").active_tab,
            kept
        );
        assert_eq!(
            diff_tabs(&ws, project),
            vec![("src/a.rs".into(), true), ("src/keep.rs".into(), false)],
            "the preview tab still holds what it held — and is still where it was opened,              immediately right of the console"
        );
        assert_eq!(
            workspace::preview_diff_tab(&ws, project, PreviewSlot::Working).expect("exists"),
            Some(preview)
        );
    }

    /// Double-clicking the file in the scratch slot keeps it there — VS Code's promotion,
    /// and the reason the *next* single click has to start a new preview tab rather than
    /// eating this one.
    #[test]
    fn double_clicking_the_preview_promotes_it() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        let preview =
            retarget_git_diff(&mut ws, project, repo, "src/a.rs", DiffSide::Combined, None)
                .expect("retargets");

        let promoted = open_git_diff(&mut ws, project, repo, "src/a.rs", DiffSide::Combined, None)
            .expect("opens");

        assert_eq!(promoted, preview, "no second tab over the same file");
        assert_eq!(diff_tabs(&ws, project), vec![("src/a.rs".into(), false)]);
        assert_eq!(
            workspace::preview_diff_tab(&ws, project, PreviewSlot::Working).expect("exists"),
            None
        );

        // And now the slot is free again, so the next click opens one rather than stealing
        // the tab that was just promoted.
        retarget_git_diff(&mut ws, project, repo, "src/b.rs", DiffSide::Combined, None)
            .expect("retargets");
        // The new preview tab opens where every new tab opens — next to the console — so it
        // lands in front of the one that was just promoted rather than after it.
        assert_eq!(
            diff_tabs(&ws, project),
            vec![("src/b.rs".into(), true), ("src/a.rs".into(), false)]
        );
    }

    // --- the revision diff (M18) ------------------------------------------------------

    fn at(oid: &str) -> RevSide {
        RevSide::Commit {
            oid: oid.to_owned(),
        }
    }

    /// Every revision diff tab in strip order, as (title, preview).
    ///
    /// Keyed on the *title* rather than the path, because the title is where the pair shows up
    /// and the pair is the identity here — two rows reading `main.rs` would tell the reader of
    /// a failure nothing about which comparison was lost.
    fn revision_tabs(ws: &cide_ipc::Workspace, project: ProjectId) -> Vec<(String, bool)> {
        workspace::project(ws, project)
            .expect("exists")
            .tabs
            .iter()
            .filter_map(|t| match &t.kind {
                TabKind::Diff { spec, preview } => match &spec.origin {
                    DiffOrigin::GitRevision { .. } => Some((spec.title.clone(), *preview)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// The title is the file plus the revision it is being read at, which is what makes two
    /// history entries for one file two distinguishable rows in the strip.
    #[test]
    fn a_revision_tab_is_titled_by_its_new_side() {
        let repo = RepoId::new();
        let spec = revision_diff_spec(
            repo,
            "src/main.rs",
            at("a1b2c3d4e5f6"),
            RevSide::FirstParent,
            None,
        );
        assert_eq!(spec.title, "main.rs @ a1b2c3d");

        // The two sides that are not commits get a word. An invented `0000000` would name a
        // commit that does not exist and read exactly like one that does.
        let parent =
            revision_diff_spec(repo, "src/main.rs", RevSide::FirstParent, at("dead"), None);
        assert_eq!(parent.title, "main.rs @ parent");
        let dirty = revision_diff_spec(repo, "src/main.rs", RevSide::WorkingTree, at("dead"), None);
        assert_eq!(dirty.title, "main.rs @ working tree");
    }

    /// The asymmetry with [`shows_git_diff`], pinned: the **pair** is a revision tab's
    /// identity, so changing either side names a different document.
    #[test]
    fn reuse_of_a_revision_tab_is_keyed_on_all_four_of_repo_path_and_the_two_sides() {
        let repo = RepoId::new();
        let other = RepoId::new();
        let tab = TabKind::Diff {
            spec: revision_diff_spec(repo, "src/main.rs", at("aaa"), at("bbb"), None),
            preview: false,
        };

        assert!(shows_revision_diff(
            &tab,
            repo,
            "src/main.rs",
            &at("aaa"),
            &at("bbb")
        ));
        assert!(!shows_revision_diff(
            &tab,
            other,
            "src/main.rs",
            &at("aaa"),
            &at("bbb")
        ));
        assert!(!shows_revision_diff(
            &tab,
            repo,
            "src/other.rs",
            &at("aaa"),
            &at("bbb")
        ));
        // The two that a working-tree diff would deliberately ignore.
        assert!(!shows_revision_diff(
            &tab,
            repo,
            "src/main.rs",
            &at("ccc"),
            &at("bbb")
        ));
        assert!(!shows_revision_diff(
            &tab,
            repo,
            "src/main.rs",
            &at("aaa"),
            &RevSide::FirstParent
        ));

        // And a working-tree diff over the same file is not this tab at all.
        let working = TabKind::Diff {
            spec: git_diff_spec(repo, "src/main.rs", DiffSide::Combined, None),
            preview: false,
        };
        assert!(!shows_revision_diff(
            &working,
            repo,
            "src/main.rs",
            &at("aaa"),
            &at("bbb")
        ));
        assert!(!shows_git_diff(&tab, repo, "src/main.rs"));
    }

    /// Two comparisons of one file are two tabs, and opening the same one twice is one.
    #[test]
    fn two_comparisons_of_one_file_are_two_tabs() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();

        let first = open_revision_diff(
            &mut ws,
            project,
            repo,
            "src/main.rs",
            at("a1b2c3d"),
            RevSide::FirstParent,
            None,
        )
        .expect("opens");
        let second = open_revision_diff(
            &mut ws,
            project,
            repo,
            "src/main.rs",
            at("a1b2c3d"),
            at("9f8e7d6"),
            None,
        )
        .expect("opens");
        assert_ne!(
            first, second,
            "a different old side is a different document"
        );

        let again = open_revision_diff(
            &mut ws,
            project,
            repo,
            "src/main.rs",
            at("a1b2c3d"),
            RevSide::FirstParent,
            None,
        )
        .expect("activates");
        assert_eq!(again, first, "the same pair is the same tab");
        assert_eq!(revision_tabs(&ws, project).len(), 2);
    }

    // --- the revision tab (M18) -----------------------------------------------------------

    /// Every revision tab in strip order, as (title, rev, chain).
    fn revision_file_tabs(
        ws: &cide_ipc::Workspace,
        project: ProjectId,
    ) -> Vec<(String, String, Vec<String>)> {
        workspace::project(ws, project)
            .expect("exists")
            .tabs
            .iter()
            .filter_map(|t| match &t.kind {
                TabKind::Revision {
                    title, rev, from, ..
                } => Some((title.clone(), rev.clone(), from.clone())),
                _ => None,
            })
            .collect()
    }

    /// The title is the file plus the commit, spelled the way `revision_diff_spec` spells it —
    /// so `log.rs @ a1b2c3d` and a revision *diff* of the same file sit next to each other in
    /// the strip under one convention rather than two.
    #[test]
    fn a_revision_tab_is_titled_file_at_short_oid() {
        assert_eq!(
            revision_title("crates/cide-git/src/log.rs", "a1b2c3d4e5f60718"),
            "log.rs @ a1b2c3d"
        );
        // A path with no directory, and an oid shorter than the cut. Neither is reachable from
        // the product today; both are reachable from the wire, and `[..7]` would panic on the
        // second.
        assert_eq!(revision_title("README.md", "abc"), "README.md @ abc");
    }

    /// **The rule this tab exists to get right**: the key is the document, not the route.
    ///
    /// Two walks that arrive at one commit of one file are looking at the same bytes, so the
    /// second finds the first — and the chain the first tab was opened with survives, because
    /// that is the trail the user can actually see and retrace.
    #[test]
    fn one_revision_of_one_file_is_one_tab_however_the_user_walked_there() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();

        let first = open_revision(
            &mut ws,
            project,
            repo,
            "src/main.rs",
            "a1b2c3d",
            &["9f8e7d6".to_string()],
        )
        .expect("opens");

        // The same file at the same commit, reached down a different branch of the DAG.
        let again = open_revision(
            &mut ws,
            project,
            repo,
            "src/main.rs",
            "a1b2c3d",
            &["deadbee".to_string(), "cafe123".to_string()],
        )
        .expect("activates");

        assert_eq!(again, first, "the route is not part of the tab's identity");
        assert_eq!(
            revision_file_tabs(&ws, project),
            vec![(
                "main.rs @ a1b2c3d".to_string(),
                "a1b2c3d".to_string(),
                vec!["9f8e7d6".to_string()],
            )],
            "and the chain of the tab that already exists wins — the second walk must not \
             rewrite a trail the user has already been shown"
        );

        // The three things that *are* the identity, each on its own.
        let other_rev =
            open_revision(&mut ws, project, repo, "src/main.rs", "9f8e7d6", &[]).expect("opens");
        let other_path =
            open_revision(&mut ws, project, repo, "src/lib.rs", "a1b2c3d", &[]).expect("opens");
        let other_repo = open_revision(
            &mut ws,
            project,
            RepoId::new(),
            "src/main.rs",
            "a1b2c3d",
            &[],
        )
        .expect("opens");
        assert_eq!(
            [first, other_rev, other_path, other_repo]
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4
        );
        assert_eq!(revision_file_tabs(&ws, project).len(), 4);
    }

    /// The chain reaches the tab normalised, so a walk round a merge cannot grow `workspace.json`
    /// and the tab's own revision never appears in its own trail.
    #[test]
    fn the_chain_a_revision_tab_is_opened_with_is_normalised() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        open_revision(
            &mut ws,
            project,
            repo,
            "src/main.rs",
            "ccc",
            &[
                "aaa".to_string(),
                "bbb".to_string(),
                "aaa".to_string(),
                "ccc".to_string(),
            ],
        )
        .expect("opens");

        assert_eq!(
            revision_file_tabs(&ws, project)
                .into_iter()
                .map(|(_, _, from)| from)
                .collect::<Vec<_>>(),
            vec![vec!["aaa".to_string(), "bbb".to_string()]]
        );
    }

    /// A revision tab is not a diff tab and must not be found by the diff lookups — otherwise a
    /// single click in the git panel would retarget a read-only buffer over a commit.
    #[test]
    fn a_revision_tab_is_invisible_to_the_diff_lookups() {
        let repo = RepoId::new();
        let tab = TabKind::Revision {
            repo,
            path: "src/main.rs".into(),
            rev: "a1b2c3d".into(),
            from: Vec::new(),
            title: "main.rs @ a1b2c3d".into(),
        };
        assert!(shows_revision(&tab, repo, "src/main.rs", "a1b2c3d"));
        assert!(!shows_git_diff(&tab, repo, "src/main.rs"));
        assert!(!shows_revision_diff(
            &tab,
            repo,
            "src/main.rs",
            &at("a1b2c3d"),
            &RevSide::FirstParent
        ));

        // And the converse: a revision *diff* over the same file at the same commit is a
        // different document and does not answer for this one.
        let diff = TabKind::Diff {
            spec: revision_diff_spec(
                repo,
                "src/main.rs",
                at("a1b2c3d"),
                RevSide::FirstParent,
                None,
            ),
            preview: false,
        };
        assert!(!shows_revision(&diff, repo, "src/main.rs", "a1b2c3d"));
    }

    /// Forty single clicks down a commit's file list leave one tab, exactly as they do in the
    /// git panel — the same rule, over the tool window's own scratch slot.
    #[test]
    fn forty_single_clicks_in_the_log_leave_one_revision_tab() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();

        for i in 0..40 {
            retarget_revision_diff(
                &mut ws,
                project,
                repo,
                &format!("src/file{i}.rs"),
                at("a1b2c3d"),
                RevSide::FirstParent,
                None,
            )
            .expect("retargets");
        }

        assert_eq!(
            revision_tabs(&ws, project),
            vec![("file39.rs @ a1b2c3d".into(), true)]
        );
    }

    /// **The report this whole slot split exists for.**
    ///
    /// The git panel's scratch tab and the tool window's are on screen at once. With one slot,
    /// clicking a file in the log retargets the tab the user was staging from and the
    /// half-made selection in it is gone. Two tabs, and neither ever becomes the other.
    #[test]
    fn clicking_in_the_log_does_not_eat_the_diff_being_staged_from() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();

        let staging =
            retarget_git_diff(&mut ws, project, repo, "src/a.rs", DiffSide::Unstaged, None)
                .expect("retargets");
        let history = retarget_revision_diff(
            &mut ws,
            project,
            repo,
            "src/b.rs",
            at("a1b2c3d"),
            RevSide::FirstParent,
            None,
        )
        .expect("retargets");

        assert_ne!(staging, history);
        assert_eq!(diff_tabs(&ws, project), vec![("src/a.rs".into(), true)]);
        assert_eq!(
            revision_tabs(&ws, project),
            vec![("b.rs @ a1b2c3d".into(), true)]
        );

        // And each goes on reusing its own slot rather than the other's.
        retarget_git_diff(&mut ws, project, repo, "src/c.rs", DiffSide::Unstaged, None)
            .expect("retargets");
        retarget_revision_diff(
            &mut ws,
            project,
            repo,
            "src/d.rs",
            at("a1b2c3d"),
            RevSide::FirstParent,
            None,
        )
        .expect("retargets");
        assert_eq!(diff_tabs(&ws, project), vec![("src/c.rs".into(), true)]);
        assert_eq!(
            revision_tabs(&ws, project),
            vec![("d.rs @ a1b2c3d".into(), true)]
        );
    }

    /// Double-click promotes the tool window's scratch tab, and the next single click opens a
    /// new one rather than eating what was just kept — `tab_open_diff`'s rule, unchanged.
    #[test]
    fn double_clicking_a_revision_preview_promotes_it() {
        let (mut ws, project) = bare();
        let repo = RepoId::new();
        let preview = retarget_revision_diff(
            &mut ws,
            project,
            repo,
            "src/a.rs",
            at("a1b2c3d"),
            RevSide::FirstParent,
            None,
        )
        .expect("retargets");

        let promoted = open_revision_diff(
            &mut ws,
            project,
            repo,
            "src/a.rs",
            at("a1b2c3d"),
            RevSide::FirstParent,
            None,
        )
        .expect("opens");

        assert_eq!(promoted, preview, "no second tab over the same comparison");
        assert_eq!(
            revision_tabs(&ws, project),
            vec![("a.rs @ a1b2c3d".into(), false)]
        );

        retarget_revision_diff(
            &mut ws,
            project,
            repo,
            "src/b.rs",
            at("a1b2c3d"),
            RevSide::FirstParent,
            None,
        )
        .expect("retargets");
        assert_eq!(
            revision_tabs(&ws, project),
            vec![
                ("b.rs @ a1b2c3d".into(), true),
                ("a.rs @ a1b2c3d".into(), false),
            ]
        );
    }

    /// The sentence a user reads when *Send lines to Claude* cannot send.
    ///
    /// Tested because it is the entire user-visible half of the feature: the failure path is
    /// the one being reported ("does nothing"), and its only output is this prose. The
    /// assertion that matters is the negative one — [`not_connected`] receives *every*
    /// connection on the project's server, including an unbound one in the pane that was
    /// addressed, so it must never describe them as being somewhere else. The wording it
    /// replaced ("one other session in this project is — focus that pane") sent the user
    /// looking for a second pane that, in the commonest form of this failure, is the one they
    /// were already in.
    #[test]
    fn the_refusal_never_claims_the_connected_session_is_in_another_pane() {
        let none = not_connected(0);
        assert!(
            none.contains("No Claude session in this project"),
            "zero connections is a different problem and gets its own sentence: {none}"
        );
        assert!(
            none.contains("/ide"),
            "the one action that fixes it has to be in the sentence: {none}"
        );

        for (connections, count) in [(1usize, "One session"), (4, "4 sessions")] {
            let message = not_connected(connections);
            assert!(
                message.contains(count),
                "the count is what separates a missing feature from a mis-aimed one: {message}"
            );
            assert!(
                !message.contains("other session"),
                "the count includes an unbound connection in the addressed pane, so calling \
                 them 'other' sends the user hunting for a pane that need not exist: {message}"
            );
            assert!(
                !message.contains("Focus"),
                "and for the same reason it must not tell them to focus one: {message}"
            );
            assert!(
                message.contains("/ide"),
                "the one action that fixes every variant has to be in the sentence: {message}"
            );
        }
    }

    // --- where a mention goes when the pane it was aimed at cannot take it ------------------

    /// A Claude pane with a title, so a candidate list is readable when it is wrong.
    fn claude(title: &str) -> Pane {
        Pane {
            id: PaneId::new(),
            kind: PaneKind::Claude,
            role: PaneRole::Auxiliary,
            session: Some(cide_ipc::SessionId::new()),
            conversation: None,
            conversation_since: None,
            continues: None,
            harness: None,
            title: title.into(),
            docker: None,
            origin: None,
        }
    }

    fn shell(title: &str) -> Pane {
        Pane {
            id: PaneId::new(),
            kind: PaneKind::Shell,
            role: PaneRole::Auxiliary,
            session: Some(cide_ipc::SessionId::new()),
            conversation: None,
            conversation_since: None,
            continues: None,
            harness: None,
            title: title.into(),
            docker: None,
            origin: None,
        }
    }

    /// Put `pane` in a tab beside whatever is focused there, and answer its id.
    ///
    /// `layout::split` focuses what it creates, so every fixture that cares about focus sets
    /// it explicitly afterwards rather than relying on the order things were added in.
    fn add(ws: &mut cide_ipc::Workspace, project: ProjectId, tab: TabId, pane: Pane) -> PaneId {
        let t = workspace::tab_mut(ws, project, tab).expect("the tab exists");
        let anchor = t.tree.focused;
        cide_core::layout::split(
            &mut t.tree,
            anchor,
            cide_ipc::Axis::Col,
            cide_ipc::Side::After,
            pane,
        )
        .expect("splits")
    }

    fn focus(ws: &mut cide_ipc::Workspace, project: ProjectId, tab: TabId, pane: PaneId) {
        let t = workspace::tab_mut(ws, project, tab).expect("the tab exists");
        cide_core::layout::focus(&mut t.tree, pane).expect("focuses");
    }

    /// The console pane of a fresh project: `tabs[0]`'s only pane, and the only
    /// [`PaneRole::Primary`] one anywhere in it.
    fn console_of(ws: &cide_ipc::Workspace, project: ProjectId) -> (TabId, PaneId) {
        let p = workspace::project(ws, project).expect("exists");
        (p.tabs[0].id, p.tabs[0].tree.focused)
    }

    /// Every pane of the ranking fixture below.
    struct Ranked {
        ws: cide_ipc::Workspace,
        project: ProjectId,
        console: TabId,
        /// `tabs[0]`'s primary pane — the project's durable default. Deliberately **last** in
        /// the console's pane map.
        primary: PaneId,
        /// The pane the gesture is aimed at. Named by no other rung.
        asked: PaneId,
        /// The console's own `tree.focused`.
        console_focus: PaneId,
        /// The active tab's `tree.focused`, in a second Claude tab.
        tab2_focus: PaneId,
        /// A console pane no rung names at all. Only the sweep reaches it.
        unnamed: PaneId,
    }

    /// A project arranged so that **every rung of [`mention_candidates`] is observable**.
    ///
    /// This is the part that is easy to get wrong in a test rather than in the code. The sweep
    /// at the end of the list reaches every Claude pane in the project, so a fixture where the
    /// rungs happen to agree with map order produces the same *set* and very nearly the same
    /// *order* whichever rungs are working — and a test written against it passes with the
    /// focused-pane rule deleted, which is the rule the whole change is for. Three properties
    /// are therefore arranged deliberately:
    ///
    /// * the console's pane map does **not** start with the primary pane. Real workspaces are
    ///   like this — `PaneTree::panes` is an `IndexMap` persisted in order, and a project whose
    ///   panes have been closed and re-added restores in whatever order it was left in — and it
    ///   is the only arrangement in which the primary rung differs from the sweep.
    /// * the focused panes are not the first panes in their maps.
    /// * one pane is named by no rung, so the sweep has something of its own to contribute and
    ///   the tests can tell "reached by its rung" from "reached at the end anyway".
    ///
    /// Console map order: `asked`, `console_focus`, `unnamed`, `primary`.
    /// A second Claude tab, which is the active one, holds `tab2_focus`.
    fn ranked() -> Ranked {
        let (mut ws, project) = bare();
        let (console, primary) = console_of(&ws, project);
        let asked = add(
            &mut ws,
            project,
            console,
            claude("cide : claude — aimed at"),
        );
        let console_focus = add(
            &mut ws,
            project,
            console,
            claude("cide : claude — worked in"),
        );
        let unnamed = add(&mut ws, project, console, claude("cide : claude — idle"));
        focus(&mut ws, project, console, console_focus);

        // The primary pane to the back of the console's map. See the doc comment.
        {
            let t = workspace::tab_mut(&mut ws, project, console).expect("the console tab");
            let at = t
                .tree
                .panes
                .get_index_of(&primary)
                .expect("the primary pane");
            t.tree.panes.move_index(at, t.tree.panes.len() - 1);
        }

        // A second Claude tab, which `open_tab` makes active — so its focused pane is the
        // "most recently used" one for the purposes of the ranking.
        let tab2 = workspace::open_tab(
            &mut ws,
            project,
            TabKind::ClaudeFull {
                title: "review".into(),
                ephemeral: false,
            },
            claude("cide : claude — review"),
        )
        .expect("opens a second claude tab");
        let tab2_focus = workspace::tab(&ws, project, tab2)
            .expect("exists")
            .tree
            .focused;

        Ranked {
            ws,
            project,
            console,
            primary,
            asked,
            console_focus,
            tab2_focus,
            unnamed,
        }
    }

    /// The whole ordering, in one assertion, over a fixture where each rung answers something
    /// the others do not.
    ///
    /// A vector rather than five `contains` checks, and that is the point: every rung in this
    /// function names panes the sweep at the end would reach anyway, so membership proves
    /// nothing at all. Only the order says which rule produced the answer — and the order is
    /// the entire content of the function, because the first addressable candidate wins.
    #[test]
    fn the_candidate_order_is_asked_then_focused_then_the_projects_default() {
        let r = ranked();
        assert_eq!(
            mention_candidates(&r.ws, r.project, r.asked),
            vec![r.asked, r.tab2_focus, r.console_focus, r.primary, r.unnamed,],
            "aimed at, then the pane the user was last in, then the console they were last in, \
             then the project's default, then whatever is left"
        );
    }

    /// The rule that has to keep holding: a send that was going to work is untouched.
    ///
    /// The fallback is only ever allowed to fire where the old code produced a hard error, so
    /// the asked-for pane leads whatever else the project contains — including a pane that
    /// every other rung would prefer. Anything else is a behaviour change wearing a bug fix's
    /// clothes.
    #[test]
    fn the_pane_the_gesture_named_is_always_tried_first() {
        let r = ranked();
        for asked in [r.asked, r.unnamed, r.primary, r.console_focus] {
            assert_eq!(
                mention_candidates(&r.ws, r.project, asked).first(),
                Some(&asked),
                "whichever pane the gesture named has to be tried before any preference"
            );
        }
    }

    /// The workspace shape from the bug report, reduced to its bones.
    ///
    /// The console holds four Claude panes. The one the frontend's rule picks — `tabs[0]`'s
    /// first in map order — is not the one the user was working in, and in the live workspace
    /// it was a pane sitting at a **resume splash with no process at all**: a Claude pane is
    /// spawned eagerly only when it holds the project's primary session or has a transcript to
    /// resume, so in a restored project most Claude panes have no `claude` behind them. So the
    /// pane the user last worked in must be tried before anything else, or the fallback walks
    /// straight past the one conversation they meant.
    ///
    /// `PaneTree::focused` is the only recency signal this app has, and it is Rust-owned and
    /// durable. No MRU store was added; see [`mention_candidates`].
    #[test]
    fn the_focused_claude_pane_is_the_first_thing_tried_after_the_asked_for_one() {
        let r = ranked();
        let order = mention_candidates(&r.ws, r.project, r.asked);
        assert_eq!(
            order.first(),
            Some(&r.asked),
            "the asked-for pane is still first"
        );
        assert_eq!(
            order.get(1),
            Some(&r.tab2_focus),
            "then the focused pane of the tab the user is in — this is the whole fix: {order:?}"
        );
        assert!(
            order.iter().position(|p| *p == r.console_focus)
                < order.iter().position(|p| *p == r.unnamed),
            "and the console's own focused pane outranks a console pane nothing points at: \
             {order:?}"
        );
    }

    /// An editor tab is the ordinary case, and it names no Claude of its own.
    ///
    /// The gesture is made *from an editor*, where by definition no Claude pane is focused. So
    /// the active tab contributes nothing and the console the user last worked in is what the
    /// send falls back to — which is a different pane from the console's first, and from its
    /// primary.
    #[test]
    fn an_editor_tab_falls_back_to_the_console_the_user_last_worked_in() {
        let mut r = ranked();
        workspace::open_tab(
            &mut r.ws,
            r.project,
            TabKind::File {
                path: PathBuf::from("/repo/src/main.rs"),
                dirty: false,
            },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                conversation_since: None,
                continues: None,
                harness: None,
                title: "main.rs".into(),
                docker: None,
                origin: None,
            },
        )
        .expect("opens a file tab");

        let order = mention_candidates(&r.ws, r.project, r.asked);
        assert_eq!(
            order.get(1),
            Some(&r.console_focus),
            "an editor tab names no Claude, so the console's own focused pane is next: {order:?}"
        );
    }

    /// The project's default is a **pane**, not `Project::primary_session`.
    ///
    /// That field is written once when the project is created and never re-pointed when a pane
    /// respawns fresh, so in a used workspace it commonly names a session no pane holds — the
    /// live workspace this bug was found in is exactly that, and routing to it would address
    /// nothing at all. `tabs[0]`'s `PaneRole::Primary` pane is the durable default that is
    /// actually true: `tabs[0]` is always the pinned console and its primary pane cannot be
    /// closed.
    #[test]
    fn the_default_is_the_consoles_primary_pane_and_not_the_session_id_that_names_nothing() {
        let mut r = ranked();
        {
            let p = r.ws.projects.get_mut(&r.project).expect("the project");
            p.primary_session = cide_ipc::SessionId::new();
        }
        let orphan = workspace::project(&r.ws, r.project)
            .expect("exists")
            .primary_session;
        assert!(
            workspace::project(&r.ws, r.project)
                .expect("exists")
                .tabs
                .iter()
                .all(|t| t.tree.panes.values().all(|p| p.session != Some(orphan))),
            "the fixture is only meaningful if primary_session names no pane, as it did live"
        );

        let order = mention_candidates(&r.ws, r.project, r.asked);
        assert!(
            order.iter().position(|p| *p == r.primary) < order.iter().position(|p| *p == r.unnamed),
            "the console's primary pane is the project's default and is tried before the rest \
             of the map, however stale `primary_session` is and wherever the pane sits: {order:?}"
        );
    }

    /// Shells, editors and diffs are not conversations, and detached panes still are.
    #[test]
    fn only_claude_panes_are_candidates_and_a_torn_out_one_still_counts() {
        let mut r = ranked();
        let bash = add(&mut r.ws, r.project, r.console, shell("cide : bash"));
        let torn = add(
            &mut r.ws,
            r.project,
            r.console,
            claude("cide : claude — detached"),
        );
        workspace::detach_pane(&mut r.ws, r.project, r.console, torn).expect("detaches");

        let order = mention_candidates(&r.ws, r.project, r.asked);
        assert!(
            !order.contains(&bash),
            "a shell has no prompt to mention into: {order:?}"
        );
        assert!(
            order.contains(&torn),
            "a pane torn into its own window is still this project's conversation: {order:?}"
        );
        assert_eq!(
            order.last(),
            Some(&torn),
            "and it goes last, behind every docked pane: {order:?}"
        );
    }

    /// Every rung names panes the rungs above it may already have named.
    ///
    /// Duplicates would be harmless to correctness and actively misleading to read. Pinned
    /// because the obvious refactor — pushing each rung and deduplicating at the end — loses
    /// the ordering that is the entire content of this function.
    #[test]
    fn a_pane_appears_exactly_once_however_many_rules_name_it() {
        let r = ranked();
        // Asking for the primary pane: three rules name it — the asked-for rung, the project
        // default rung, and the sweep — and exactly one entry may result. The console's own
        // focused pane is named twice over as well, by rung 3 and by the sweep.
        let order = mention_candidates(&r.ws, r.project, r.primary);
        let mut seen = order.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            order.len(),
            "no pane may appear twice: {order:?}"
        );
        assert_eq!(
            order.first(),
            Some(&r.primary),
            "and deduplication must not cost the ordering: the first mention of a pane is the \
             one that counts, so a later rule naming it again may not move it: {order:?}"
        );
    }

    /// A project id this window does not hold answers with the asked-for pane and nothing else.
    ///
    /// Reachable: a window can be showing a project that has since been closed, and the send
    /// then has to end in `NoServer` or `NotConnected` rather than in a panic or an empty list
    /// that would make the caller address nobody at all.
    #[test]
    fn an_unknown_project_still_yields_the_pane_that_was_asked_for() {
        let (ws, _) = bare();
        let asked = PaneId::new();
        assert_eq!(
            mention_candidates(&ws, ProjectId::new(), asked),
            vec![asked],
            "the caller must always have something to address, so the error names the pane \
             the user actually aimed at"
        );
    }

    // --- terminal_open_path's refusals -----------------------------------------------------
    //
    // The half of the terminal-link feature that must not rot. Every case below is reachable
    // from a program printing one line of text into a pane, which is why they are exercised
    // against real directories rather than asserted about in prose.

    /// A scratch project root, removed and recreated so a rerun is not a failure.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-openable-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("scratch root");
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").expect("scratch file");
        dir
    }

    /// Nobody has approved anything — the state every first click is in.
    const UNASKED: Option<&Path> = None;

    #[test]
    fn a_file_inside_the_project_opens() {
        let root = scratch("inside");
        let roots = vec![root.clone()];
        let opened = openable(&roots, &root.join("src/main.rs"), UNASKED).expect("openable");
        assert!(
            !opened.outside,
            "a file the project contains is not an out-of-project open, and must not be logged \
             or treated as one"
        );
    }

    /// The line at the top of this feature's brief. Not opened without an answer.
    ///
    /// `⏺ Read(/home/you/.claude/.credentials.json)` printed by any program in any pane is a
    /// small, valid-UTF-8 text file: without containment it would open and its contents would
    /// be in the webview. Since M13 the user may overrule this — but only by name, and only
    /// after reading the path, which is what the second half of this test pins.
    #[test]
    fn a_path_outside_every_root_is_refused_until_the_user_approves_that_exact_file() {
        let root = scratch("outside");
        let elsewhere = std::env::temp_dir().join(format!(
            "cide-openable-outside-secret-{}.json",
            std::process::id()
        ));
        std::fs::write(&elsewhere, "{\"token\":\"nope\"}\n").expect("a perfectly readable file");
        let roots = vec![root.clone()];

        let refusal = openable(&roots, &elsewhere, UNASKED).expect_err("unapproved");
        let TerminalOpenError::Outside { real, .. } = &refusal else {
            panic!("a readable text file outside the project must be refused: {refusal:?}");
        };
        let real = real
            .clone()
            .expect("the refusal has to name what would be opened");
        assert_eq!(
            real,
            std::fs::canonicalize(&elsewhere)
                .expect("canonical")
                .display()
                .to_string(),
            "the confirmation names the canonical path, so the refusal has to carry it"
        );

        let approved = openable(&roots, &elsewhere, Some(Path::new(&real))).expect("approved");
        assert!(
            approved.outside,
            "an approved open is still an out-of-project open and has to say so"
        );
        let _ = std::fs::remove_file(&elsewhere);
    }

    /// The approval is an approval of a *file*, not of a string.
    ///
    /// Swap what the path resolves to between the dialog and the click and the user's answer no
    /// longer describes anything they were shown. Refused, and refused with the *new* target, so
    /// the second question is about what is actually there.
    #[test]
    fn an_approval_for_a_different_target_is_not_an_approval() {
        let root = scratch("approval-mismatch");
        let elsewhere =
            std::env::temp_dir().join(format!("cide-openable-mismatch-{}.txt", std::process::id()));
        std::fs::write(&elsewhere, "hello\n").expect("a file");
        let roots = vec![root.clone()];
        let stale = std::env::temp_dir().join("cide-openable-mismatch-something-else.txt");

        let refusal = openable(&roots, &elsewhere, Some(&stale)).expect_err("mismatch");
        assert!(
            matches!(&refusal, TerminalOpenError::Outside { real: Some(r), .. }
                if *r == std::fs::canonicalize(&elsewhere).expect("canonical").display().to_string()),
            "a stale approval is answered by asking again about the real target: {refusal:?}"
        );
        let _ = std::fs::remove_file(&elsewhere);
    }

    /// `..` is refused as a component rather than normalised.
    ///
    /// And it is refused **with no `real`**, which is load-bearing: a malformed path is the one
    /// out-of-project refusal that no approval can ever satisfy, so the frontend must not be
    /// able to raise a dialog for it. `real: None` is that signal.
    #[test]
    fn a_path_that_climbs_out_is_refused_even_though_it_starts_inside_a_root() {
        let root = scratch("climb");
        let roots = vec![root.clone()];
        let climbing = root.join("src/../../etc/passwd");
        assert!(matches!(
            openable(&roots, &climbing, UNASKED),
            Err(TerminalOpenError::Outside { real: None, .. })
        ));
        // And approving it changes nothing — there is no target to approve.
        assert!(matches!(
            openable(&roots, &climbing, Some(Path::new("/etc/passwd"))),
            Err(TerminalOpenError::Outside { real: None, .. })
        ));
    }

    /// The case textual containment cannot see, and the reason `canonicalize` is in there.
    #[test]
    fn a_symlink_inside_the_project_pointing_out_of_it_is_refused() {
        let root = scratch("symlink");
        let target = std::env::temp_dir().join(format!(
            "cide-openable-symlink-target-{}.txt",
            std::process::id()
        ));
        std::fs::write(&target, "secret\n").expect("target");
        let link = root.join("src/escape.rs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let roots = vec![root.clone()];
        let refusal = openable(&roots, &link, UNASKED).expect_err("refused");
        assert!(
            matches!(&refusal, TerminalOpenError::Outside { real: Some(r), .. }
                if r.contains("cide-openable-symlink-target")),
            "check_within is textual by design, so the canonical path has to be contained too — \
             and the dialog has to name where the link actually goes: {refusal:?}"
        );
        let _ = std::fs::remove_file(&target);
    }

    /// A machine whose project path runs through a symlink must not fail its own files.
    ///
    /// This is what the roots are canonicalised for. Without it, a `$HOME` that is a symlink —
    /// ordinary on managed machines — would make every file in every project refuse to open.
    #[test]
    fn a_root_reached_through_a_symlink_still_opens_its_own_files() {
        let real = scratch("root-symlink-real");
        let alias =
            std::env::temp_dir().join(format!("cide-openable-root-alias-{}", std::process::id()));
        let _ = std::fs::remove_file(&alias);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &alias).expect("symlink the root itself");
        let roots = vec![alias.clone()];
        assert!(
            openable(&roots, &alias.join("src/main.rs"), UNASKED).is_ok(),
            "the file is inside the root the user opened; that the root is a symlink is not \
             the user's problem"
        );
        let _ = std::fs::remove_file(&alias);
    }

    /// `Compiling cide-app v0.1.0 (/home/…/cide)` is a real, frequent line.
    #[test]
    fn a_directory_is_not_a_file_and_cargo_prints_one_on_every_build() {
        let root = scratch("dir");
        let roots = vec![root.clone()];
        assert!(matches!(
            openable(&roots, &root.join("src"), UNASKED),
            Err(TerminalOpenError::NotAFile(_))
        ));
    }

    /// The one that would not merely be wrong but would hang.
    ///
    /// `document::read`'s only metadata check is `is_dir`, so a FIFO would reach `read_to_end`
    /// and park a blocking-pool worker there for as long as nobody writes to it — which, for a
    /// pipe some build script left in the tree, is for ever.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_before_anything_tries_to_read_it() {
        let root = scratch("fifo");
        let fifo = root.join("src/pipe");
        let path = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).expect("c string");
        // SAFETY: a valid NUL-terminated path and a constant mode; `mkfifo` touches nothing else.
        let made = unsafe { libc::mkfifo(path.as_ptr(), 0o644) };
        if made != 0 {
            // A filesystem that cannot hold a FIFO is not a reason to fail the suite; the
            // assertion below is only meaningful if the node exists.
            return;
        }
        let roots = vec![root.clone()];
        assert!(matches!(
            openable(&roots, &fifo, UNASKED),
            Err(TerminalOpenError::NotAFile(_))
        ));
    }

    /// Output outlives the files it names, so this is ordinary rather than exceptional.
    #[test]
    fn a_path_that_no_longer_exists_says_so() {
        let root = scratch("missing");
        let roots = vec![root.clone()];
        assert!(matches!(
            openable(&roots, &root.join("src/gone.rs"), UNASKED),
            Err(TerminalOpenError::Missing(_))
        ));
    }

    /// A relative path never reaches here — the frontend resolves — but the check is the
    /// backend's, because the frontend's answer is a claim and not evidence.
    #[test]
    fn a_relative_path_is_refused_rather_than_resolved_against_something() {
        let root = scratch("relative");
        let roots = vec![root];
        assert!(matches!(
            openable(&roots, Path::new("src/main.rs"), UNASKED),
            Err(TerminalOpenError::Outside { real: None, .. })
        ));
    }

    // --- the guards that are NOT about the project boundary ---------------------------------
    //
    // Until the out-of-project open existed, containment fired first for everything under
    // `/dev`, `/proc` and `/tmp`, so `is_file` and the size limit were very nearly decorative:
    // nothing they refuse was reachable except through a path inside the project. An approved
    // out-of-project path reaches them for the first time, which is why each one is pinned here
    // *with* an approval — the approval answers one question and must not answer the others.

    /// A FIFO outside the project would still park a blocking-pool worker for ever.
    #[cfg(unix)]
    #[test]
    fn an_approved_fifo_outside_the_project_is_still_refused() {
        let root = scratch("approved-fifo");
        // Named apart from `scratch`'s own directory: `scratch("approved-fifo")` builds
        // `cide-openable-approved-fifo-<pid>`, and giving the FIFO that same path made
        // `mkfifo` fail with EEXIST — which this test treats as "this filesystem cannot hold a
        // FIFO" and passes. A silent pass is the one outcome an escape hatch must not produce
        // for the wrong reason, and it did, until a mutation run noticed the test never failed.
        let fifo = std::env::temp_dir().join(format!(
            "cide-openable-approved-fifo-{}.pipe",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&fifo);
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).expect("c string");
        // SAFETY: a valid NUL-terminated path and a constant mode; `mkfifo` touches nothing else.
        if unsafe { libc::mkfifo(c.as_ptr(), 0o644) } != 0 {
            return;
        }
        let roots = vec![root];
        assert!(
            matches!(
                openable(&roots, &fifo, Some(&fifo)),
                Err(TerminalOpenError::NotAFile(_))
            ),
            "the user approved crossing the project boundary, not hanging the blocking pool"
        );
        let _ = std::fs::remove_file(&fifo);
    }

    /// `/dev/zero` reports `len == 0`, so only `is_file` stands between it and `read_to_end`.
    #[cfg(unix)]
    #[test]
    fn an_approved_device_node_is_still_refused() {
        let root = scratch("approved-dev");
        let roots = vec![root];
        let zero = Path::new("/dev/zero");
        if !zero.exists() {
            return;
        }
        assert!(
            matches!(
                openable(&roots, zero, Some(zero)),
                Err(TerminalOpenError::NotAFile(_))
            ),
            "`/dev/zero` has a length of 0, so the size guard is inert there and `is_file` is \
             the only thing between it and a read that allocates until the process is killed"
        );
    }

    /// A directory outside the project is as un-openable as one inside it.
    #[test]
    fn an_approved_directory_outside_the_project_is_still_refused() {
        let root = scratch("approved-dir");
        let elsewhere = scratch("approved-dir-elsewhere");
        let roots = vec![root];
        assert!(matches!(
            openable(&roots, &elsewhere, Some(&elsewhere)),
            Err(TerminalOpenError::NotAFile(_))
        ));
    }

    /// The size limit is the editor's, not the project's.
    #[test]
    fn an_approved_enormous_file_outside_the_project_is_still_refused() {
        let root = scratch("approved-huge");
        let huge = std::env::temp_dir().join(format!(
            "cide-openable-approved-huge-{}.log",
            std::process::id()
        ));
        // Sparse: `set_len` past the limit costs no blocks and no time.
        let f = std::fs::File::create(&huge).expect("create");
        f.set_len(document::MAX_FILE_BYTES + 1).expect("set_len");
        drop(f);
        let roots = vec![root];
        let real = std::fs::canonicalize(&huge).expect("canonical");
        assert!(matches!(
            openable(&roots, &huge, Some(&real)),
            Err(TerminalOpenError::TooLarge { .. })
        ));
        let _ = std::fs::remove_file(&huge);
    }

    /// An approval cannot conjure a file. Refused as `Missing`, and with no dialog behind it.
    #[test]
    fn an_approved_path_that_does_not_exist_is_still_missing() {
        let root = scratch("approved-gone");
        let gone = std::env::temp_dir().join(format!(
            "cide-openable-approved-gone-{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&gone);
        let roots = vec![root];
        assert!(matches!(
            openable(&roots, &gone, Some(&gone)),
            Err(TerminalOpenError::Missing(_))
        ));
    }

    /// A refusal that cannot be overruled must never reach the confirmation.
    ///
    /// The frontend's rule is "`kind == outside` and `real != null` asks; everything else is a
    /// sentence" (`ui/src/terminal/outsideOpen.ts`). This is the Rust half of that agreement:
    /// no other variant may ever serialise a `real`, or a device node would grow an *Open
    /// anyway* button that opens nothing.
    #[test]
    fn only_the_outside_refusal_carries_a_target_to_approve() {
        let cases = [
            TerminalOpenError::Missing("/p/gone.rs".into()),
            TerminalOpenError::NotAFile("/p/src".into()),
            TerminalOpenError::TooLarge {
                path: "/p/huge.log".into(),
                size: 2048,
                limit: 32,
            },
            TerminalOpenError::Failed("the project closed".into()),
            TerminalOpenError::Outside {
                path: "../../.ssh/id_rsa".into(),
                real: None,
            },
        ];
        for case in cases {
            let json = serde_json::to_value(&case).expect("serialisable");
            assert!(
                json["real"].is_null(),
                "this refusal is final; a target on it would offer an approval that cannot \
                 work: {json}"
            );
        }
        let asked = serde_json::to_value(TerminalOpenError::Outside {
            path: "/home/you/.ssh/id_rsa".into(),
            real: Some("/home/you/.ssh/id_rsa".into()),
        })
        .expect("serialisable");
        assert_eq!(asked["real"], "/home/you/.ssh/id_rsa");
    }

    /// Every refusal is a sentence, because every one of them is shown to a person.
    #[test]
    fn every_refusal_carries_a_message_and_a_kind() {
        let cases = [
            TerminalOpenError::Outside {
                path: "/etc/shadow".into(),
                real: Some("/etc/shadow".into()),
            },
            TerminalOpenError::Missing("/p/gone.rs".into()),
            TerminalOpenError::NotAFile("/p/src".into()),
            TerminalOpenError::TooLarge {
                path: "/p/huge.log".into(),
                size: 2048,
                limit: 32,
            },
            TerminalOpenError::Failed("the project closed".into()),
        ];
        for case in cases {
            let json = serde_json::to_value(&case).expect("serialisable");
            let kind = json["kind"].as_str().expect("a kind to branch on");
            let message = json["message"].as_str().expect("a sentence to read");
            let path = json["path"].as_str().expect("the path it is about");
            assert!(!kind.is_empty(), "{json}");
            assert!(!path.is_empty(), "{json}");
            assert!(
                message.len() > 10 && message.contains(' '),
                "a refusal the user cannot read is a link wired to nothing: {json}"
            );
        }
    }
    /*
     * The bytes road's parse. (M63)
     *
     * Everything about `file_write_bytes` that is not Tauri is here: the frame is split, the head
     * is a `FileBytesWrite`, the payload is the file. The refusals carry sentences that name the
     * road the caller should have taken, because the failure a user sees is "my drawing did not
     * save" and the log line is the only clue.
     */

    #[test]
    fn a_framed_write_is_split_into_its_request_and_its_bytes() {
        let head = serde_json::to_string(&cide_ipc::FileBytesWrite {
            path: "/home/иван/схема.excalidraw.png".into(),
            if_unchanged: Some(cide_ipc::FileStamp {
                mtime_nanos: 1_700_000_000_000_000_001,
                len: 42,
            }),
        })
        .expect("serialises");
        let payload = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let frame = cide_ipc::frame::pack(&head, payload).expect("packs");

        let (write, bytes) =
            bytes_request(&tauri::ipc::InvokeBody::Raw(frame)).expect("a well-formed frame");
        assert_eq!(write.path, "/home/иван/схема.excalidraw.png");
        assert_eq!(write.if_unchanged.map(|s| s.len), Some(42));
        assert_eq!(bytes, payload);
    }

    #[test]
    fn a_json_body_is_refused_by_name() {
        let error = bytes_request(&tauri::ipc::InvokeBody::Json(serde_json::json!({
            "path": "/tmp/x.excalidraw",
            "bytes": [1, 2, 3]
        })))
        .expect_err("JSON is the road this command exists to avoid");
        let text = format!("{error}");
        assert!(text.contains("raw body"), "{text}");
        assert!(text.contains("never JSON"), "{text}");
    }

    #[test]
    fn a_truncated_frame_and_a_head_that_is_not_a_write_are_refused_with_the_reason() {
        let error = bytes_request(&tauri::ipc::InvokeBody::Raw(vec![9, 0, 0, 0, b'{']))
            .expect_err("the prefix promises nine bytes of head");
        assert!(format!("{error}").contains("truncated"), "{error}");

        let frame = cide_ipc::frame::pack(r#"{"writable":true}"#, b"").expect("packs");
        let error = bytes_request(&tauri::ipc::InvokeBody::Raw(frame))
            .expect_err("a read head is not a write head");
        assert!(
            format!("{error}").contains("not a write request"),
            "{error}"
        );
    }
}

/// The read-only rule, driven end to end over a real file.
///
/// A module of its own so it does not have to share the `openable` suite's scratch-directory
/// helpers, and because the claim is different in kind: that one is about a path the *user*
/// clicked in a terminal, this one is about every file the editor opens.
#[cfg(test)]
mod read_only_tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-readonly-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// A crate as cargo actually unpacks one: mode **644**.
    ///
    /// That mode is the whole bug. Go's module cache is 444, so an edit there failed at
    /// `File::create` and the user saw an error; Rust's registry is writable, so before this rule
    /// existed Ctrl+S in a `serde` buffer opened by Go to definition wrote through
    /// `document::write` into the copy every project on the machine builds against.
    #[test]
    #[cfg(unix)]
    fn a_registry_source_is_read_only_even_though_its_mode_bits_say_otherwise() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("registry");
        let cache = dir.join("registry/src");
        let crate_dir = cache.join("index.crates.io-1949/serde-1.0.229/src");
        std::fs::create_dir_all(&crate_dir).expect("mkdir");
        let path = crate_dir.join("lib.rs");
        std::fs::write(&path, "pub fn de() {}\n").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        let roots = vec![dir.join("work/cide")];
        let caches = vec![cache.clone()];

        assert!(
            document::read(&path).expect("read").writable,
            "the mode bits really do say this file is writable — that is the trap"
        );
        let doc = read_document(&path, &roots, &caches).expect("read");
        assert!(
            !doc.writable,
            "a dependency source must open read-only whatever its mode bits say"
        );

        // The second lock. `writable` is a flag that travelled through the webview and is not
        // evidence about what the disk should accept.
        let error = write_document(&path, "wiped", &roots, &caches, None)
            .expect_err("a dependency source must not be written");
        let message = error.to_string();
        assert!(message.contains("read-only"), "{message}");
        assert!(
            message.contains("serde-1.0.229"),
            "the refusal has to name the file, or nobody can tell which save failed: {message}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("still there"),
            "pub fn de() {}\n",
            "the refusal has to happen BEFORE the write, not after it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The override, and it has to exist: a user who opened a vendored crate *as a project root*
    /// has said with the strongest gesture the app has that this is their code.
    #[test]
    fn a_project_root_over_the_cache_stays_writable() {
        let dir = scratch("patched");
        let cache = dir.join("registry/src");
        let crate_dir = cache.join("index.crates.io-1949/serde-1.0.229");
        std::fs::create_dir_all(&crate_dir).expect("mkdir");
        let path = crate_dir.join("lib.rs");
        std::fs::write(&path, "pub fn de() {}\n").expect("seed");

        let caches = vec![cache];
        assert!(
            read_document(&path, std::slice::from_ref(&crate_dir), &caches)
                .expect("read")
                .writable,
            "opening the crate as a project root is an explicit instruction to edit it"
        );
        assert!(
            write_document(
                &path,
                "patched\n",
                std::slice::from_ref(&crate_dir),
                &caches,
                None
            )
            .is_ok()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And the rule must not fire for the 99.9% case, or every save in the project fails.
    #[test]
    fn an_ordinary_project_file_is_untouched() {
        let dir = scratch("ordinary");
        let root = dir.join("work/cide/src");
        std::fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("main.rs");
        std::fs::write(&path, "fn main() {}\n").expect("seed");

        let roots = vec![dir.join("work/cide")];
        let caches = vec![dir.join("registry/src")];
        assert!(
            read_document(&path, &roots, &caches)
                .expect("read")
                .writable
        );
        assert!(write_document(&path, "fn main() { }\n", &roots, &caches, None).is_ok());
        assert_eq!(
            std::fs::read_to_string(&path).expect("written"),
            "fn main() { }\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The commands have to pass the *real* cache list, not an empty one — an empty `caches`
    /// makes the rule a no-op and every assertion above vacuous at runtime.
    #[test]
    fn the_machine_has_dependency_roots_to_check_against() {
        let roots = cide_core::toolchain::dependency_roots();
        assert!(
            !roots.is_empty(),
            "no cargo or go cache could be derived, so `file_read` would pass an empty list"
        );
        assert!(
            roots.iter().any(|r| r.ends_with("registry/src")),
            "the cargo registry is the one that is writable and so the one that matters: {roots:?}"
        );
    }
}
