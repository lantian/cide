//! The queue of changes agents propose to what only the user may change. (M83)
//!
//! `cide_ipc::proposals` says why this exists; `cide_agents::milestones::proposal_draft` decides
//! what may be proposed. This module keeps the queue — per project, in the profile's state
//! directory — and does the two things only the user can ask for:
//!
//! * **Accept** applies the change exactly as the user read it. A plan goes through
//!   `milestones::set_plan`, the same writer as the panel. Files are written and committed — only
//!   those paths — and refused, untouched, when any of them no longer says what it said when the
//!   proposal was made: a diff reviewed against one text and applied over another is a change
//!   nobody reviewed.
//! * **Reject** removes it.
//!
//! Either is noted on the task the proposal came out of, so the agent that asked learns the answer
//! the way it learns everything else — by reading the board.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cide_agents::milestones::{DraftChange, ProposalDraft};
use cide_ipc::{ProjectId, Proposal, ProposalChange, ProposedFile, TaskAuthor, TaskEdit};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::tasks_state::TasksStores;
use crate::workspace_state::WorkspaceState;

#[derive(Default)]
pub struct Proposals {
    inner: Mutex<Option<HashMap<PathBuf, Queue>>>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Queue {
    next: u64,
    items: Vec<Proposal>,
}

fn store_path() -> PathBuf {
    cide_core::persist::state_dir().join("proposals.json")
}

impl Proposals {
    /// Change the queue for `root` and save it. Reads go through [`Self::list`], which does not
    /// write: the panel asks for the view on every board change.
    fn with<T>(&self, root: &Path, f: impl FnOnce(&mut Queue) -> T) -> T {
        self.access(root, true, f)
    }

    fn access<T>(&self, root: &Path, save: bool, f: impl FnOnce(&mut Queue) -> T) -> T {
        let mut guard = self.inner.lock().expect("proposals lock");
        let map = guard.get_or_insert_with(|| {
            std::fs::read(store_path())
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        });
        let answer = f(map.entry(root.to_path_buf()).or_default());
        if !save {
            return answer;
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&*map)
            && let Err(error) = cide_core::persist::write_atomic(&store_path(), &bytes)
        {
            tracing::warn!(%error, "could not save the proposal queue");
        }
        answer
    }

    /// The queue for `root`, oldest first.
    pub fn list(&self, root: &Path) -> Vec<Proposal> {
        self.access(root, false, |q| q.items.clone())
    }
}

fn proposals(app: &AppHandle) -> Option<Arc<Proposals>> {
    app.try_state::<Arc<Proposals>>().map(|s| Arc::clone(&s))
}

fn root_of(app: &AppHandle, project: ProjectId) -> Option<PathBuf> {
    let workspace = app.try_state::<WorkspaceState>()?;
    crate::tasks_state::project_root(&workspace, project).ok()
}

/// The queue for a project, for the view. Empty when anything is missing.
pub fn list(app: &AppHandle, root: &Path) -> Vec<Proposal> {
    proposals(app).map(|p| p.list(root)).unwrap_or_default()
}

/// Queue a validated draft.
pub fn create(
    app: &AppHandle,
    project: ProjectId,
    draft: ProposalDraft,
    by: TaskAuthor,
) -> Result<Proposal, String> {
    let root = root_of(app, project).ok_or("that project is not open")?;
    let queue = proposals(app).ok_or("cide is shutting down")?;
    let change = match draft.change {
        DraftChange::Note => ProposalChange::Note,
        DraftChange::Plan(plan) => ProposalChange::Plan {
            plan,
            before: cide_agents::config::load_milestones(&root),
        },
        DraftChange::Files(files) => ProposalChange::Files {
            files: files
                .into_iter()
                .map(|(path, content)| {
                    let before = std::fs::read_to_string(root.join(&path)).ok();
                    let diff = unified_diff(&path, before.as_deref(), content.as_deref());
                    ProposedFile {
                        path,
                        content,
                        before,
                        diff,
                    }
                })
                .collect(),
        },
    };
    let proposal = queue.with(&root, |q| {
        q.next += 1;
        let p = Proposal {
            id: format!("p-{}", q.next),
            title: draft.title,
            rationale: draft.rationale,
            by,
            created_unix_ms: now_ms(),
            task: draft.task,
            change,
        };
        q.items.push(p.clone());
        p
    });
    note_on_task(
        app,
        project,
        &proposal,
        &format!(
            "**Proposed {}: {}** — waiting for the user in the Milestones tab of the Tasks panel.\n\n{}",
            proposal.id, proposal.title, proposal.rationale
        ),
        TaskAuthor::Orchestrator,
    );
    crate::emit::milestones_changed(app, project);
    Ok(proposal)
}

/// Take a proposal out of the queue without applying it.
pub fn withdraw(app: &AppHandle, project: ProjectId, id: &str) -> Result<(), String> {
    let root = root_of(app, project).ok_or("that project is not open")?;
    let queue = proposals(app).ok_or("cide is shutting down")?;
    let removed = queue.with(&root, |q| {
        let before = q.items.len();
        q.items.retain(|p| p.id != id);
        before != q.items.len()
    });
    if !removed {
        return Err(format!("no proposal {id} is waiting"));
    }
    crate::emit::milestones_changed(app, project);
    Ok(())
}

/// The user rejects a proposal.
pub fn reject(app: &AppHandle, project: ProjectId, id: &str) -> Result<(), String> {
    let proposal = find(app, project, id)?;
    withdraw(app, project, id)?;
    note_on_task(
        app,
        project,
        &proposal,
        &format!(
            "**The user rejected {}: {}.** Nothing was changed.",
            proposal.id, proposal.title
        ),
        TaskAuthor::User,
    );
    Ok(())
}

/// The user accepts a proposal: apply it, then take it out of the queue.
pub fn accept(app: &AppHandle, project: ProjectId, id: &str) -> Result<String, String> {
    let root = root_of(app, project).ok_or("that project is not open")?;
    let proposal = find(app, project, id)?;
    let outcome = match &proposal.change {
        ProposalChange::Note => "acknowledged".to_string(),
        ProposalChange::Plan { plan, .. } => {
            let mut plan = plan.clone();
            crate::milestones::set_plan(app, project, &mut plan, TaskAuthor::User)?;
            crate::milestones::run_gate(app, project);
            "the milestones were replaced".to_string()
        }
        ProposalChange::Files { files } => {
            let committed = apply_files(&root, files, &proposal)?;
            crate::milestones::run_gate(app, project);
            committed
        }
    };
    withdraw(app, project, id)?;
    note_on_task(
        app,
        project,
        &proposal,
        &format!(
            "**The user accepted {}: {}** — {outcome}.",
            proposal.id, proposal.title
        ),
        TaskAuthor::User,
    );
    Ok(outcome)
}

fn find(app: &AppHandle, project: ProjectId, id: &str) -> Result<Proposal, String> {
    let root = root_of(app, project).ok_or("that project is not open")?;
    list(app, &root)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("no proposal {id} is waiting"))
}

/// Write the files, refusing before any write if one has moved, then commit just those paths.
fn apply_files(root: &Path, files: &[ProposedFile], proposal: &Proposal) -> Result<String, String> {
    for f in files {
        let now = std::fs::read_to_string(root.join(&f.path)).ok();
        if now != f.before {
            return Err(format!(
                "`{}` has changed since {} was proposed, so the diff you read is not the change \
                 it would make. Reject it and ask for it again against the file as it is now.",
                f.path, proposal.id
            ));
        }
    }
    for f in files {
        let path = root.join(&f.path);
        match &f.content {
            Some(text) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                // A rewritten file keeps its mode, so an executable gate stays executable; a new
                // shell script is made executable, since a gate names it as a command.
                std::fs::write(&path, text).map_err(|e| format!("{}: {e}", f.path))?;
                #[cfg(unix)]
                if f.before.is_none() && f.path.ends_with(".sh") {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                }
            }
            None => {
                if path.exists() {
                    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", f.path))?;
                }
            }
        }
    }
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let message = format!(
        "Accept {}: {}\n\n{}",
        proposal.id, proposal.title, proposal.rationale
    );
    let mut add = vec!["add", "--all", "--"];
    add.extend(&paths);
    let mut commit = vec!["commit", "--quiet", "-m", message.as_str(), "--"];
    commit.extend(&paths);
    match git(root, &add).and_then(|_| git(root, &commit)) {
        Ok(()) => Ok(format!("written and committed ({})", paths.join(", "))),
        Err(error) => Ok(format!(
            "written ({}); not committed: {error}",
            paths.join(", ")
        )),
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    cide_core::child_env::prepare_command(&mut cmd);
    cide_core::child_env::arm(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `before` → `after` as a unified diff, by `git diff --no-index` over two temporary files.
fn unified_diff(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let dir = cide_core::persist::state_dir().join("proposal-diff");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = format!("{}-{}", std::process::id(), now_ms());
    let a = dir.join(format!("{stamp}-a"));
    let b = dir.join(format!("{stamp}-b"));
    let _ = std::fs::write(&a, before.unwrap_or(""));
    let _ = std::fs::write(&b, after.unwrap_or(""));
    let mut cmd = std::process::Command::new("git");
    cmd.args(["diff", "--no-index", "--no-color", "-U3", "--"])
        .arg(&a)
        .arg(&b)
        .stdin(std::process::Stdio::null());
    cide_core::child_env::prepare_command(&mut cmd);
    cide_core::child_env::arm(&mut cmd);
    let body = cmd
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
    // Drop git's header (the temp names); keep the hunks, under the real path.
    let hunks: String = body
        .lines()
        .skip_while(|l| !l.starts_with("@@"))
        .collect::<Vec<_>>()
        .join("\n");
    let from = if before.is_some() {
        format!("a/{path}")
    } else {
        "/dev/null".into()
    };
    let to = if after.is_some() {
        format!("b/{path}")
    } else {
        "/dev/null".into()
    };
    format!("--- {from}\n+++ {to}\n{hunks}")
}

fn note_on_task(app: &AppHandle, project: ProjectId, p: &Proposal, text: &str, by: TaskAuthor) {
    let Some(task) = p.task.as_ref() else {
        return;
    };
    let Some(store) = app
        .try_state::<Arc<TasksStores>>()
        .and_then(|stores| stores.get(project))
    else {
        return;
    };
    if store.get(task).is_none() {
        return;
    }
    let _ = store.edit(
        task,
        TaskEdit::Comment {
            text: text.to_string(),
        },
        by,
    );
    crate::tasks_state::broadcast(app, project, &store);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-proposal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tools")).expect("mkdir");
        std::fs::write(dir.join("tools/gate.sh"), "check\n").expect("seed");
        std::fs::write(dir.join("other.txt"), "untouched\n").expect("seed");
        for args in [
            vec!["init", "-q"],
            // Repository-local identity, so the commit `apply_files` makes needs nothing from
            // the machine running the test.
            vec!["config", "user.name", "t"],
            vec!["config", "user.email", "t@t"],
            vec!["add", "-A"],
            vec!["commit", "-qm", "seed"],
        ] {
            let ok = std::process::Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .status()
                .expect("git")
                .success();
            assert!(ok, "{args:?}");
        }
        dir
    }

    fn proposal(files: Vec<ProposedFile>) -> Proposal {
        Proposal {
            id: "p-1".into(),
            title: "Add e2e".into(),
            rationale: "the gate misses e2e".into(),
            by: TaskAuthor::Orchestrator,
            created_unix_ms: 1,
            task: None,
            change: ProposalChange::Files { files },
        }
    }

    /// A file that moved since it was proposed is refused before anything is written; one that
    /// did not is written and committed — that path only, whatever else is dirty.
    #[test]
    fn accepting_files_applies_exactly_what_was_read_or_nothing() {
        let dir = repo("apply");
        let file = |before: &str| ProposedFile {
            path: "tools/gate.sh".into(),
            content: Some("check && e2e\n".into()),
            before: Some(before.into()),
            diff: unified_diff("tools/gate.sh", Some(before), Some("check && e2e\n")),
        };
        assert!(
            file("check\n").diff.contains("+check && e2e"),
            "{}",
            file("check\n").diff
        );

        let stale = proposal(vec![file("something else\n")]);
        let ProposalChange::Files { files } = &stale.change else {
            unreachable!()
        };
        let why = apply_files(&dir, files, &stale).expect_err("moved since");
        assert!(why.contains("has changed since"), "{why}");
        assert_eq!(
            std::fs::read_to_string(dir.join("tools/gate.sh")).unwrap(),
            "check\n"
        );

        std::fs::write(dir.join("other.txt"), "dirty, and not ours\n").unwrap();
        let fresh = proposal(vec![file("check\n")]);
        let ProposalChange::Files { files } = &fresh.change else {
            unreachable!()
        };
        let outcome = apply_files(&dir, files, &fresh).expect("applied");
        assert!(outcome.contains("committed"), "{outcome}");
        assert_eq!(
            std::fs::read_to_string(dir.join("tools/gate.sh")).unwrap(),
            "check && e2e\n"
        );
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dir)
            .output()
            .expect("git");
        assert_eq!(
            String::from_utf8_lossy(&status.stdout).trim(),
            "M other.txt",
            "only the proposal's path was committed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
