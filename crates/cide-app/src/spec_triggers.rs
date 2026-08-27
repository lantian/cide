//! A finished checklist moves its task to `review`. (M28)
//!
//! # Why this is a sibling of `task_triggers` and not part of it
//!
//! That module is *"a task was mutated — ask a pure policy what to start"*: its input is a
//! [`crate::task_triggers::TaskMutation`], its decision is `cide_agents::autodispatch::trigger`,
//! and its header promises it only adapts shapes. This one is *"a file moved — ask a subprocess
//! how far the work got"*. They share no input, no purity property and no thread discipline, and
//! folding them would put an `openspec` spawn inside a module that documents itself as holding
//! none.
//!
//! # The root comes from the event, never from the project
//!
//! This is the detail the whole feature turns on. While a run is live, its checklist is the one
//! in **its own worktree** — `.cide/worktrees/<agent>/openspec/changes/<name>/tasks.md` — and
//! `openspec` resolves its root from the working directory. A read made from the project root
//! would answer with the *project's* progress, look entirely plausible, and be about a different
//! copy of the file. So the directory to ask is derived from the path that changed.
//!
//! `cide_spec` refuses a root that resolved to an ancestor, which is the backstop: a worktree
//! that somehow has no `openspec/` of its own reports an error rather than the parent's board.
//!
//! # Idempotency is the status gate, and there is nothing else
//!
//! [`note_checklist_complete`] moves a task only from `Doing`. That single condition is the whole
//! mechanism, and it is worth stating what it replaces: no ledger of changes already seen, no
//! "fired" set to persist, nothing to rebuild after a restart, and no window in which a crash
//! mid-burst loses or repeats a hop. The second, tenth and hundredth event about the same
//! finished checklist read `Review` and return.
//!
//! It also cannot move backwards. A reviewer who set the task `Done` and then touched
//! `tasks.md` — or a `git checkout` that rewrote it — finds `Done`, not `Doing`, and nothing
//! happens. And un-ticking a box fires nothing at all, because the condition is one-directional:
//! a user who drags the card back to `Doing` themselves is the one who re-arms it, which is the
//! only re-fire that should exist.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cide_ipc::{ChangeName, ProjectId, TaskAuthor, TaskEdit, TaskId, TaskStatus};
use tauri::{AppHandle, Manager as _};

use crate::tasks_state::TasksStores;

/// Look at a batch of changed paths and, for any change whose checklist just finished, move its
/// task to `review`. Returns immediately; the work happens on the blocking pool.
///
/// The scan itself is allocation-light and does no I/O, because this is called from the watcher
/// thread — `dotcide`'s rule.
pub fn consider(app: &AppHandle, project: ProjectId, paths: &[PathBuf], root: &Path) {
    let touched = changes_in(paths, root);
    if touched.is_empty() {
        return;
    }
    let app = app.clone();
    let root = root.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        for (cwd, change) in touched {
            consider_one(&app, project, &cwd, &change, &root);
        }
    });
}

/// The `(directory to ask, change)` pairs a batch of paths implies.
///
/// A [`BTreeSet`], so an agent that writes `tasks.md` six times in one burst costs one
/// subprocess and not six — and so the order is stable, which keeps a test able to assert on it.
///
/// The directory is the one **containing** the `openspec/` the path is under, which for a
/// worktree path is the worktree and for a project path is the project. That is exactly what
/// `openspec` needs as a working directory.
fn changes_in(paths: &[PathBuf], root: &Path) -> BTreeSet<(PathBuf, ChangeName)> {
    let mut found = BTreeSet::new();
    for path in paths {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let parts: Vec<&std::ffi::OsStr> = relative.components().map(|c| c.as_os_str()).collect();
        // Two shapes only, and they are the two `dotcide::classify` routes to `Spec`:
        //   openspec/changes/<name>/…
        //   .cide/worktrees/<agent>/openspec/changes/<name>/…
        let (cwd, rest) = if parts
            .first()
            .is_some_and(|p| *p == cide_spec::SPEC_RELATIVE)
        {
            (root.to_path_buf(), &parts[..])
        } else if parts.len() >= 3
            && parts[0] == ".cide"
            && parts[1] == "worktrees"
            && parts.get(3).is_some_and(|p| *p == cide_spec::SPEC_RELATIVE)
        {
            (root.join(".cide/worktrees").join(parts[2]), &parts[3..])
        } else {
            continue;
        };
        // rest is `openspec / changes / <name> / …`
        if rest.len() >= 3 && rest[1] == "changes" {
            let name = rest[2].to_string_lossy().to_string();
            found.insert((cwd, ChangeName(name)));
        }
    }
    found
}

/// Ask one directory about one change, and hop the task if the checklist is finished.
fn consider_one(app: &AppHandle, project: ProjectId, cwd: &Path, change: &ChangeName, root: &Path) {
    // Find the task first: it is a memory read, and it is the common way for this to be a no-op.
    // Spawning `openspec` for a change nothing on the board is linked to would be a subprocess
    // per keystroke for somebody hand-editing a proposal.
    let Some(stores) = app.try_state::<Arc<TasksStores>>() else {
        return;
    };
    let Some(store) = stores.get(project) else {
        return;
    };
    let linked: Vec<TaskId> = store
        .list()
        .into_iter()
        .filter(|task| task.change.as_ref() == Some(change))
        .filter(|task| task.status == TaskStatus::Doing)
        .map(|task| task.id)
        .collect();
    let [task] = linked.as_slice() else {
        // Zero is the ordinary case — nothing is linked, or the task is not `Doing`. More than
        // one is a board that fans a change across several tasks, which has no single hop to
        // make; guessing would move a card somebody else is holding.
        if linked.len() > 1 {
            tracing::debug!(
                %project, change = %change, tasks = linked.len(),
                "several tasks name this change; not choosing one to move"
            );
        }
        return;
    };

    let os = match cide_spec::Openspec::open(cwd) {
        Ok(os) => os,
        // No CLI. Nothing to say here — the panel says it, once, where somebody is looking.
        Err(error) => {
            tracing::debug!(%error, "no openspec, so no checklist to read");
            return;
        }
    };
    let progress = match os.progress(change) {
        Ok(progress) => progress,
        Err(error) => {
            tracing::debug!(%error, change = %change, "could not read the checklist");
            return;
        }
    };
    if !progress.complete() {
        return;
    }

    tracing::info!(
        %project, %task, change = %change, cwd = %cwd.display(),
        total = progress.total,
        "the checklist is finished; moving the task to review"
    );
    note_checklist_complete(app, project, task);
    let _ = root;
}

/// A finished checklist moves its task `Doing → Review`, and only that hop.
///
/// [`crate::task_triggers::note_run_started`]'s twin at the other end of a run, with the same
/// shape and the same argument: authored [`TaskAuthor::Orchestrator`], because since M27
/// `SetStatus` records its author into `Task::history` and the card's log should say honestly
/// that cide observed this rather than that a person decided it.
///
/// The status gate is the idempotency — see the module header.
pub fn note_checklist_complete(app: &AppHandle, project: ProjectId, task: &TaskId) {
    let Some(stores) = app.try_state::<Arc<TasksStores>>() else {
        return;
    };
    let Some(store) = stores.get(project) else {
        return;
    };
    if store.get(task).map(|task| task.status) != Some(TaskStatus::Doing) {
        return;
    }
    let edit = TaskEdit::SetStatus {
        status: TaskStatus::Review,
    };
    match store.edit(task, edit, TaskAuthor::Orchestrator) {
        Ok(_) => {
            tracing::info!(%project, %task, "the finished checklist moved its task to review");
            crate::tasks_state::broadcast(app, project, &store);
        }
        Err(error) => {
            tracing::debug!(%task, %error, "could not move the finished task to review")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_projects_own_change_is_asked_about_from_the_project_root() {
        let root = Path::new("/repo");
        let found = changes_in(
            &paths(&[
                "/repo/openspec/changes/add-dark-mode/tasks.md",
                "/repo/openspec/changes/add-dark-mode/proposal.md",
            ]),
            root,
        );
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            vec![(
                PathBuf::from("/repo"),
                ChangeName("add-dark-mode".to_string())
            )],
            "two files in one change cost one subprocess, not two"
        );
    }

    #[test]
    fn a_live_agents_change_is_asked_about_from_that_agents_worktree() {
        // The detail the feature turns on: `openspec` resolves its root from the working
        // directory, so asking from the project root would answer with the project's own
        // progress — plausible, and about a different copy of the file.
        let root = Path::new("/repo");
        let found = changes_in(
            &paths(&["/repo/.cide/worktrees/dev-t-17/openspec/changes/add-dark-mode/tasks.md"]),
            root,
        );
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            vec![(
                PathBuf::from("/repo/.cide/worktrees/dev-t-17"),
                ChangeName("add-dark-mode".to_string())
            )]
        );
    }

    #[test]
    fn nothing_else_under_a_worktree_asks_anything() {
        // The storm filter's rule, restated here because this scan runs over the same batch.
        let root = Path::new("/repo");
        assert!(
            changes_in(
                &paths(&[
                    "/repo/.cide/worktrees/dev/target/debug/x",
                    "/repo/.cide/worktrees/dev/src/openspec/changes/a/tasks.md",
                    "/repo/src/openspec/changes/a/tasks.md",
                    "/repo/openspec/specs/dark-mode/spec.md",
                    "/elsewhere/openspec/changes/a/tasks.md",
                ]),
                root
            )
            .is_empty(),
            "only a change directory under the project's or an agent's openspec/ counts"
        );
    }

    #[test]
    fn several_files_across_several_changes_are_one_ask_each() {
        let root = Path::new("/repo");
        let found = changes_in(
            &paths(&[
                "/repo/openspec/changes/a/tasks.md",
                "/repo/openspec/changes/a/design.md",
                "/repo/openspec/changes/b/tasks.md",
                "/repo/.cide/worktrees/dev/openspec/changes/a/tasks.md",
            ]),
            root,
        );
        assert_eq!(found.len(), 3, "two changes here plus one in a worktree");
    }
}
