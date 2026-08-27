//! One spec board per project, read on demand and broadcast when the files move. (M28)
//!
//! # There is no store here, and that is the difference from `tasks_state`
//!
//! `TasksStores` exists because `.cide/tasks.json` has a *writer*: a debounced one, with an
//! out-of-process merge, a flusher thread and a `write_now` on the way out. None of that applies
//! to `openspec/`. A board is a re-read — `cide_agents::load_project`'s posture, for its stated
//! reason — so what this type holds is not state but a **coalescer**.
//!
//! # Why the coalescing is not optional
//!
//! A board costs two subprocesses. The watcher hands over bursts: an `openspec init` creates a
//! directory tree, a `git checkout` rewrites a dozen files, an agent's turn touches `tasks.md`
//! several times a second. Reading per event would be a Node process per file.
//!
//! The `flushing` flag buys a second property that matters more than the first. Only one flusher
//! runs per burst, so **only one read is ever in flight for a project** — which is what makes it
//! safe for `spec-changed` to carry no `rev`. Two overlapping reads could otherwise land in the
//! order they finished rather than the order they started, and the older board would win. That is
//! the ordering hazard `tasks-changed`'s revision exists to close; here it is closed by
//! construction instead, which is why the event has nothing to compare.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cide_ipc::{ProjectId, SpecBoard};
use parking_lot::Mutex;
use tauri::AppHandle;

/// How long a burst has to be quiet before the board is read.
///
/// Twice `AgentRegistry`'s, deliberately: its flush rebuilds a roster from a handful of small
/// files, and this one spawns `openspec` twice. A user watching a checklist tick does not need
/// 120 ms; they need the bar to move while the agent works.
const COALESCE: Duration = Duration::from_millis(250);
/// And the ceiling, so an agent that never stops writing still updates about once a second.
const COALESCE_CEILING: Duration = Duration::from_secs(1);
/// How often the flusher wakes while a burst settles.
const COALESCE_TICK: Duration = Duration::from_millis(25);

/// The per-project coalescer behind `cide://spec-changed`.
#[derive(Default)]
pub struct SpecBoards {
    state: Mutex<CoalesceState>,
}

#[derive(Default)]
struct CoalesceState {
    pending: HashSet<ProjectId>,
    first: Option<Instant>,
    last: Option<Instant>,
    flushing: bool,
}

impl SpecBoards {
    /// Note that a project's `openspec/` moved, and start a flusher if none is waiting.
    ///
    /// Called from the watcher thread, so it does no I/O and takes no lock but its own.
    pub fn mark_changed(self: &Arc<Self>, app: &AppHandle, project: ProjectId) {
        if !self.mark(project) {
            return;
        }
        let app = app.clone();
        let this = Arc::clone(self);
        // One short-lived named thread per burst, `AgentRegistry::mark_changed`'s shape. Named,
        // because an unnamed thread in a backtrace is a thread nobody can attribute.
        let spawned = std::thread::Builder::new()
            .name("cide-spec-emit".to_string())
            .spawn(move || this.flush(&app));
        if let Err(error) = spawned {
            // The flag is already set, so failing quietly here would leave the project unable to
            // ever start another flusher. Clear it, and say so.
            tracing::warn!(%error, "could not start the spec board flusher");
            self.state.lock().flushing = false;
        }
    }

    fn mark(&self, project: ProjectId) -> bool {
        let mut state = self.state.lock();
        let now = Instant::now();
        state.pending.insert(project);
        state.first.get_or_insert(now);
        state.last = Some(now);
        if state.flushing {
            return false;
        }
        state.flushing = true;
        true
    }

    fn take_due(&self) -> Option<Vec<ProjectId>> {
        let mut state = self.state.lock();
        let (first, last) = (state.first?, state.last?);
        if last.elapsed() < COALESCE && first.elapsed() < COALESCE_CEILING {
            return None;
        }
        state.first = None;
        state.last = None;
        state.flushing = false;
        Some(state.pending.drain().collect())
    }

    fn flush(&self, app: &AppHandle) {
        loop {
            std::thread::sleep(COALESCE_TICK);
            let Some(projects) = self.take_due() else {
                continue;
            };
            for project in projects {
                let board = read(app, project);
                crate::emit::spec_changed(app, project, &board);
            }
            return;
        }
    }
}

/// Read one project's board, turning every failure into a board rather than an error.
///
/// # Every refusal is a screen
///
/// `TaskBoard`'s rule: a panel that receives an error has nothing to draw and no way to say what
/// to do next, while a board that *says* the CLI is missing has a sentence and a Retry. So a
/// project with no `openspec/` is `Absent`, and everything else — no binary, a refusal, a root
/// that belongs to a parent repository — is `Unusable` with the reason in it.
pub fn read(app: &AppHandle, project: ProjectId) -> SpecBoard {
    use tauri::Manager as _;

    let Some(workspace) = app.try_state::<crate::workspace_state::WorkspaceState>() else {
        return SpecBoard::Unusable {
            reason: "this window has no workspace".to_string(),
        };
    };
    let root = match crate::tasks_state::project_root(&workspace, project) {
        Ok(root) => root,
        Err(error) => {
            return SpecBoard::Unusable {
                reason: error.to_string(),
            };
        }
    };
    read_at(&root)
}

/// [`read`] for a directory already in hand — the project's root, or one agent's checkout.
pub fn read_at(root: &std::path::Path) -> SpecBoard {
    // cide's own answer, before the binary is even looked for: a project that does not use
    // OpenSpec is not a machine that is missing a tool, and telling somebody to `npm i -g`
    // because they opened a repository without specs would be an answer to a question nobody
    // asked. See `cide_spec::present`.
    if !cide_spec::present(root) {
        return SpecBoard::Absent {
            hint: ABSENT_HINT.to_string(),
            path: cide_spec::spec_path(root),
        };
    }
    let os = match cide_spec::Openspec::open(root) {
        Ok(os) => os,
        Err(error) => {
            return SpecBoard::Unusable {
                reason: error.to_string(),
            };
        }
    };
    match os.board() {
        Ok(board) => board,
        Err(error) => SpecBoard::Unusable {
            reason: error.to_string(),
        },
    }
}

/// The sentence the panel prints above its Set-up button.
///
/// Here rather than in the frontend because it is *about the project* — the same reason
/// `TaskBoard::Absent` carries its own — and because the frontend must not be the place that
/// decides what OpenSpec is for.
pub const ABSENT_HINT: &str = "OpenSpec keeps this project's requirements in openspec/specs/, and each piece of work in \
     openspec/changes/ — a proposal, a task list, and the requirement edits it makes. It is an \
     open standard: the same folder is read by Claude Code, Cursor and thirty other tools.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_costs_one_read_and_only_one_thread_flushes_it() {
        // The property that lets `spec-changed` carry no `rev`: one flusher per burst means one
        // read in flight per project, so two boards can never land out of order.
        let boards = SpecBoards::default();
        let project = ProjectId::new();

        assert!(boards.mark(project), "the first caller flushes");
        for _ in 0..50 {
            assert!(
                !boards.mark(project),
                "every later caller in the burst is told somebody else is already on it"
            );
        }
        assert!(boards.take_due().is_none(), "the burst is still arriving");

        std::thread::sleep(COALESCE + Duration::from_millis(60));
        let due = boards.take_due().expect("the burst has settled");
        assert_eq!(due, vec![project], "fifty events, one board");

        // …and the flag is clear again, so the next burst gets its own flusher.
        assert!(boards.mark(project));
    }

    #[test]
    fn a_project_without_openspec_is_absent_and_never_a_missing_tool() {
        // Telling somebody to `npm i -g` because they opened a repository that does not use
        // OpenSpec would be an answer to a question nobody asked.
        let dir = std::env::temp_dir().join(format!("cide-spec-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        match read_at(&dir) {
            SpecBoard::Absent { path, hint } => {
                assert_eq!(path, dir.join("openspec"));
                assert!(hint.contains("openspec/specs/"), "{hint}");
            }
            other => panic!("expected Absent, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
