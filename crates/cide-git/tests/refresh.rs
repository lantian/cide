//! Refresh must describe what the current staging model can actually commit.
mod support;

use cide_git::{changelist, commit, diff, repo, stage, status};
use cide_ipc::git::{CommitRequest, DiffSide, FileState, GitError, PathSelection};
use support::TempRepo;

const TASK: &str = ".cide/tasks/t-1/task.json";

fn changes(repo: &TempRepo) -> cide_ipc::git::RepoChanges {
    let info = repo::discover(std::slice::from_ref(&repo.root)).remove(0);
    status::repo_changes(&info, status::StatusRequest::default()).unwrap()
}

fn paths(changes: &cide_ipc::git::RepoChanges) -> Vec<&str> {
    changes
        .changelists
        .iter()
        .flat_map(|list| list.changes.iter().map(|entry| entry.path.as_str()))
        .collect()
}

fn fixture(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("other.txt", b"base\n");
    repo.commit_all("base");
    repo
}

fn request(selections: Option<Vec<PathSelection>>) -> CommitRequest {
    CommitRequest {
        message: "remaining changes".into(),
        amend: false,
        amend_of: None,
        changelist: None,
        selections,
        force: false,
    }
}

#[test]
fn removing_a_staged_new_task_drops_its_commit_candidate_on_every_refresh() {
    let repo = fixture("refresh-deleted-task");
    repo.write(TASK, b"{\"body\":\"task\"}\n");
    stage::stage(&repo.root, &[PathSelection::whole(TASK)]).unwrap();
    repo.write("other.txt", b"remaining change\n");
    assert_eq!(paths(&changes(&repo)), vec![TASK, "other.txt"]);

    // The Tasks panel removes the task's content directory, leaving its staged addition.
    std::fs::remove_dir_all(repo.root.join(".cide/tasks/t-1")).unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]);
    let error =
        commit::commit(&repo.root, &request(Some(vec![PathSelection::whole(TASK)]))).unwrap_err();
    assert!(matches!(error, GitError::NoSuchChange { .. }));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    let index = repo.save_index();
    // A failed submission may refresh index bookkeeping; a status read must never write it.
    assert_eq!(
        repo.git(&["show", &format!(":{TASK}")]),
        "{\"body\":\"task\"}\n"
    );
    for _ in 0..2 {
        assert_eq!(paths(&changes(&repo)), vec!["other.txt"]);
        assert_eq!(repo.save_index(), index);
        assert!(!repo.root.join(TASK).exists());
    }

    let selected = paths(&changes(&repo))
        .into_iter()
        .map(PathSelection::whole)
        .collect();
    commit::commit(&repo.root, &request(Some(selected))).unwrap();
    assert_eq!(repo.git(&["show", "HEAD:other.txt"]), "remaining change\n");
    assert!(!repo.try_git(&["cat-file", "-e", &format!("HEAD:{TASK}")]).0);
    // The default selection used by other callers must filter the same obsolete entry.
    repo.write("other.txt", b"another change\n");
    commit::commit(&repo.root, &request(None)).unwrap();
    assert_eq!(repo.git(&["show", "HEAD:other.txt"]), "another change\n");
}

#[test]
fn cancelled_staged_edits_keep_the_index_and_changelist_for_staging_area_mode() {
    let repo = fixture("refresh-cancelled-edit");
    repo.write(TASK, b"original\n");
    repo.commit_all("task");
    repo.write(TASK, b"staged change\n");
    stage::stage(&repo.root, &[PathSelection::whole(TASK)]).unwrap();
    let held = changelist::update(&repo.root, |data| data.create("Held", "")).unwrap();
    changelist::update(&repo.root, |data| data.move_paths(&held, &[TASK.into()])).unwrap();
    repo.rewrite(TASK, b"original\n");
    let index = repo.save_index();

    let refreshed = changes(&repo);
    assert!(paths(&refreshed).is_empty(), "{refreshed:#?}");
    assert_eq!(repo.save_index(), index);
    assert_eq!(changelist::load(&repo.root).owner_of(TASK), held);
    let handle = repo::open(&repo.root).unwrap();
    assert!(
        diff::file_diff(&handle, TASK, diff::DiffRequest::new(DiffSide::Combined))
            .unwrap()
            .is_none()
    );

    changelist::update(&repo.root, |data| {
        data.use_staging_area = true;
        Ok(())
    })
    .unwrap();
    let refreshed = changes(&repo);
    assert_eq!(paths(&refreshed), vec![TASK]);
    let row = &refreshed
        .changelists
        .iter()
        .find(|list| list.id == held)
        .unwrap()
        .changes[0];
    assert_eq!(row.index, FileState::Modified);
    assert_eq!(row.worktree, FileState::Modified);
    assert_eq!(repo.save_index(), index);
}

#[test]
fn cancelled_new_files_on_an_unborn_branch_are_not_commit_candidates() {
    let repo = TempRepo::new("refresh-unborn");
    repo.write(TASK, b"new task\n");
    stage::stage(&repo.root, &[PathSelection::whole(TASK)]).unwrap();
    repo.remove(TASK);
    assert!(paths(&changes(&repo)).is_empty());
    assert!(matches!(
        commit::commit(&repo.root, &request(None)),
        Err(GitError::NothingToCommit)
    ));
}

#[test]
fn real_changes_on_both_sides_stay_visible_including_binary_and_deleted_files() {
    for (tag, base, staged, working) in [
        (
            "text",
            &b"base\n"[..],
            &b"staged\n"[..],
            Some(&b"working\n"[..]),
        ),
        (
            "binary",
            &b"\0base"[..],
            &b"\0staged"[..],
            Some(&b"\0working"[..]),
        ),
        ("deleted", &b"base\n"[..], &b"staged\n"[..], None),
        (
            "cancelled-binary",
            &b"\0base"[..],
            &b"\0staged"[..],
            Some(&b"\0base"[..]),
        ),
    ] {
        let repo = fixture(&format!("refresh-{tag}"));
        repo.write(TASK, base);
        repo.commit_all("task");
        repo.write(TASK, staged);
        stage::stage(&repo.root, &[PathSelection::whole(TASK)]).unwrap();
        if let Some(bytes) = working {
            repo.rewrite(TASK, bytes);
        } else {
            repo.remove(TASK);
        }
        let index = repo.save_index();
        let refreshed = changes(&repo);
        assert_eq!(
            paths(&refreshed).is_empty(),
            tag == "cancelled-binary",
            "{tag}"
        );
        assert_eq!(repo.save_index(), index, "{tag}");
    }
}

#[test]
fn operation_conclusions_keep_index_only_changes() {
    for marker in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"] {
        let repo = fixture(&format!("refresh-{marker}"));
        repo.write(TASK, b"staged\n");
        stage::stage(&repo.root, &[PathSelection::whole(TASK)]).unwrap();
        repo.remove(TASK);
        std::fs::write(
            repo.root.join(".git").join(marker),
            repo.git(&["rev-parse", "HEAD"]),
        )
        .unwrap();
        assert_eq!(paths(&changes(&repo)), vec![TASK], "{marker}");
    }
}

#[cfg(unix)]
#[test]
fn executable_mode_changes_are_not_mistaken_for_cancelled_content() {
    use std::os::unix::fs::PermissionsExt;
    let repo = fixture("refresh-mode");
    repo.write(TASK, b"original\n");
    repo.commit_all("task");
    repo.write(TASK, b"staged\n");
    stage::stage(&repo.root, &[PathSelection::whole(TASK)]).unwrap();
    repo.rewrite(TASK, b"original\n");
    std::fs::set_permissions(repo.root.join(TASK), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(paths(&changes(&repo)), vec![TASK]);
}
