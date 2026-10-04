//! A rename row must operate on the entire move, not just one path.
mod support;

use cide_git::{changelist, commit, diff, message, repo, shelf, stage, status};
use cide_ipc::git::{CommitRequest, DiffSide, FileState, GitError, PathSelection, Selection};
use support::TempRepo;

const TEXT: &[u8] = b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";

fn fixture(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("old.txt", TEXT);
    repo.write("other.txt", b"unrelated\n");
    repo.commit_all("base");
    std::fs::rename(repo.root.join("old.txt"), repo.root.join("new.txt")).unwrap();
    repo
}

fn changes(repo: &TempRepo) -> cide_ipc::git::RepoChanges {
    let info = repo::discover(std::slice::from_ref(&repo.root)).remove(0);
    status::repo_changes(&info, status::StatusRequest::default()).unwrap()
}

fn commit(repo: &TempRepo, selections: Option<Vec<PathSelection>>) {
    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "rename".into(),
            amend: false,
            amend_of: None,
            changelist: None,
            selections,
            force: false,
        },
    )
    .unwrap();
}

#[test]
fn a_rename_row_keeps_its_destination_and_changelist_when_staged() {
    let repo = fixture("rename-row");
    let before = changes(&repo);
    assert!(before.unversioned.is_empty());
    let row = &before.changelists[0].changes[0];
    assert_eq!(row.path, "new.txt");
    assert_eq!(row.orig_path.as_deref(), Some("old.txt"));
    assert_eq!(row.worktree, FileState::Renamed);

    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    let list = changelist::update(&repo.root, |s| s.create("Held", "")).unwrap();
    changelist::update(&repo.root, |s| s.move_paths(&list, &["new.txt".into()])).unwrap();
    let staged = changes(&repo);
    assert!(staged.unversioned.is_empty());
    assert!(!staged.index_changed_externally);
    let row = &staged
        .changelists
        .iter()
        .find(|l| l.id == list)
        .unwrap()
        .changes[0];
    assert_eq!(row.path, "new.txt");
    assert_eq!(row.orig_path.as_deref(), Some("old.txt"));
    assert_eq!(row.index, FileState::Renamed);
    // An already staged row can still be moved back into Changes.
    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    changelist::update(&repo.root, |s| s.move_paths("default", &["new.txt".into()])).unwrap();
    commit(&repo, None);
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
    assert_eq!(repo.git(&["show", "HEAD:new.txt"]).as_bytes(), TEXT);
    assert!(!repo.try_git(&["cat-file", "-e", "HEAD:old.txt"]).0);
}

#[test]
fn committing_an_unstaged_rename_preserves_unrelated_staging() {
    let repo = fixture("rename-selected");
    repo.write("other.txt", b"staged other\n");
    repo.write("held.txt", b"held\n");
    stage::stage(
        &repo.root,
        &[
            PathSelection::whole("other.txt"),
            PathSelection::whole("held.txt"),
        ],
    )
    .unwrap();
    let before_other = repo.git(&["show", ":other.txt"]);
    commit(&repo, Some(vec![PathSelection::whole("new.txt")]));
    assert_eq!(repo.git(&["show", "HEAD:new.txt"]).as_bytes(), TEXT);
    assert!(!repo.try_git(&["cat-file", "-e", "HEAD:old.txt"]).0);
    assert_eq!(repo.git(&["show", ":other.txt"]), before_other);
    assert_eq!(repo.git(&["show", ":held.txt"]), "held\n");
    assert!(changes(&repo).unversioned.is_empty());
}

#[test]
fn staged_and_unstaged_rename_chains_commit_without_restoring_the_intermediate_name() {
    let repo = fixture("rename-chain");
    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    let held = changelist::update(&repo.root, |s| s.create("Held", "")).unwrap();
    changelist::update(&repo.root, |s| s.move_paths(&held, &["new.txt".into()])).unwrap();
    std::fs::rename(repo.root.join("new.txt"), repo.root.join("final.txt")).unwrap();
    let tree = changes(&repo);
    let row = &tree
        .changelists
        .iter()
        .find(|l| l.id == held)
        .unwrap()
        .changes[0];
    assert_eq!(row.path, "final.txt");
    assert_eq!(row.orig_path.as_deref(), Some("old.txt"));
    commit(&repo, Some(vec![PathSelection::whole("final.txt")]));
    assert_eq!(repo.git(&["ls-files"]), "final.txt\nother.txt\n");
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
}

#[test]
fn a_rename_can_be_unstaged_or_rolled_back_as_one_row() {
    let repo = fixture("rename-unstage");
    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    stage::unstage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    assert!(repo.git(&["diff", "--cached"]).is_empty());
    assert_eq!(repo.read("new.txt"), TEXT);
    assert_eq!(changes(&repo).changelists[0].changes[0].path, "new.txt");
    stage::rollback(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    assert_eq!(repo.read("old.txt"), TEXT);
    assert!(!repo.root.join("new.txt").exists());
    assert!(repo.git(&["status", "--porcelain"]).is_empty());

    let repo = fixture("rename-chain-rollback");
    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    std::fs::rename(repo.root.join("new.txt"), repo.root.join("final.txt")).unwrap();
    stage::rollback(&repo.root, &[PathSelection::whole("final.txt")]).unwrap();
    assert_eq!(repo.read("old.txt"), TEXT);
    assert!(!repo.root.join("final.txt").exists());
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
}

#[test]
fn rename_diffs_refuse_partial_selections_and_validate_pinned_whole_selections() {
    let repo = fixture("rename-revisions");
    repo.write(
        "new.txt",
        b"one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\n",
    );
    let handle = repo::open(&repo.root).unwrap();
    let file = diff::file_diff(
        &handle,
        "new.txt",
        diff::DiffRequest::new(DiffSide::Combined).renames(true),
    )
    .unwrap()
    .unwrap();
    assert_eq!(file.old_path.as_deref(), Some("old.txt"));
    assert_eq!(
        file.partial_refusal(),
        Some(cide_ipc::git::PartialRefusal::Rename)
    );
    let partial = PathSelection {
        path: "new.txt".into(),
        selection: Selection::Hunks { hunks: vec![0] },
        rev: Some(file.rev()),
    };
    assert!(matches!(
        stage::stage(&repo.root, std::slice::from_ref(&partial)),
        Err(GitError::PartialRefused { .. })
    ));
    assert!(matches!(
        message::commit_diff(&repo.root, &[partial]),
        Err(GitError::PartialRefused { .. })
    ));
    let pinned = PathSelection {
        path: "new.txt".into(),
        selection: Selection::Whole,
        rev: Some(file.rev()),
    };
    let index = repo.index_state();
    repo.rewrite(
        "new.txt",
        b"one\nTWO\nthree\nfour\nfive\nsix\nseven\nEIGHT\n",
    );
    assert!(matches!(
        stage::stage(&repo.root, &[pinned]),
        Err(GitError::StaleSelection { .. })
    ));
    assert_eq!(repo.index_state(), index);
}

#[test]
fn shelf_and_commit_messages_include_both_sides_of_a_rename() {
    let repo = fixture("rename-shelf");
    let selections = [PathSelection::whole("new.txt")];
    let index = repo.index_state();
    let message = message::commit_diff(&repo.root, &selections).unwrap();
    assert!(message.contains("--- a/old.txt"));
    assert!(message.contains("+++ b/new.txt"));
    assert_eq!(repo.index_state(), index);
    let entry = shelf::shelve(&repo.root, "rename", &selections).unwrap();
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
    shelf::unshelve(&repo.root, &entry.id, false).unwrap();
    assert_eq!(repo.read("new.txt"), TEXT);
    assert!(!repo.root.join("old.txt").exists());
}

#[test]
fn committing_a_staged_rename_leaves_a_recreated_source_untracked() {
    let repo = fixture("rename-recreated");
    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    repo.write("old.txt", b"separate new work\n");
    let before = repo.index_state();
    assert!(stage::rollback(&repo.root, &[PathSelection::whole("new.txt")]).is_err());
    assert!(shelf::shelve(&repo.root, "rename", &[PathSelection::whole("new.txt")]).is_err());
    assert!(shelf::list(&repo.root).is_empty());
    assert_eq!(repo.index_state(), before);
    assert_eq!(repo.read("old.txt"), b"separate new work\n");
    commit(&repo, Some(vec![PathSelection::whole("new.txt")]));
    assert!(!repo.try_git(&["cat-file", "-e", "HEAD:old.txt"]).0);
    assert_eq!(repo.git(&["status", "--porcelain"]), "?? old.txt\n");
}

#[test]
fn binary_renames_stage_commit_and_shelve_both_paths() {
    let repo = TempRepo::new("rename-binary");
    let bytes = b"\0binary\xff\0";
    repo.write("old.bin", bytes);
    repo.commit_all("binary base");
    std::fs::rename(repo.root.join("old.bin"), repo.root.join("new.bin")).unwrap();
    let selection = [PathSelection::whole("new.bin")];
    let entry = shelf::shelve(&repo.root, "binary rename", &selection).unwrap();
    assert_eq!(repo.read("old.bin"), bytes);
    shelf::unshelve(&repo.root, &entry.id, false).unwrap();
    assert_eq!(repo.read("new.bin"), bytes);
    stage::stage(&repo.root, &selection).unwrap();
    commit(&repo, Some(selection.to_vec()));
    assert_eq!(repo.git(&["ls-files"]), "new.bin\n");
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
}

#[test]
fn unstage_uses_the_index_names_in_a_rename_chain() {
    let repo = fixture("rename-chain-unstage");
    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    std::fs::rename(repo.root.join("new.txt"), repo.root.join("final.txt")).unwrap();
    stage::unstage(&repo.root, &[PathSelection::whole("final.txt")]).unwrap();
    assert!(repo.git(&["diff", "--cached"]).is_empty());
    assert_eq!(repo.read("final.txt"), TEXT);
    assert_eq!(changes(&repo).changelists[0].changes[0].path, "final.txt");
}

#[test]
fn committing_a_rename_does_not_restore_a_deleted_destination() {
    let repo = fixture("rename-deleted-destination");
    stage::stage(&repo.root, &[PathSelection::whole("new.txt")]).unwrap();
    repo.remove("new.txt");
    commit(&repo, Some(vec![PathSelection::whole("new.txt")]));
    assert_eq!(repo.git(&["ls-files"]), "other.txt\n");
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
}

#[test]
fn selecting_all_openspec_archive_rows_matches_git_add_all() {
    let repo = TempRepo::new("rename-openspec");
    let old = "openspec/changes/add-test3-file";
    let carried = ".cide/spec-carried/spec-add-test3-file/base/openspec/changes/add-test3-file";
    let archive = "openspec/changes/archive/2026-10-04-add-test3-file";
    let files = [
        ".openspec.yaml",
        "design.md",
        "proposal.md",
        "specs/test3-file/spec.md",
        "tasks.md",
    ];
    for (i, name) in files.iter().enumerate() {
        let text = format!("artifact {i}\n").repeat(20);
        repo.write(&format!("{old}/{name}"), text.as_bytes());
        repo.write(&format!("{carried}/{name}"), text.as_bytes());
    }
    repo.commit_all("proposal and carried copy");
    for name in files {
        repo.write(
            &format!("{archive}/{name}"),
            &repo.read(&format!("{old}/{name}")),
        );
    }
    repo.write(
        "openspec/specs/test3-file/spec.md",
        b"synced specification\n",
    );
    std::fs::remove_dir_all(repo.root.join(old)).unwrap();
    std::fs::remove_dir_all(repo.root.join(".cide/spec-carried/spec-add-test3-file")).unwrap();
    let before = repo.save_index();
    repo.git(&["add", "-A"]);
    let expected = repo.git(&["write-tree"]);
    repo.restore_index(&before);
    let tree = changes(&repo);
    let selected: Vec<_> = tree
        .changelists
        .iter()
        .flat_map(|l| &l.changes)
        .chain(&tree.unversioned)
        .map(|e| PathSelection::whole(&e.path))
        .collect();
    stage::stage(&repo.root, &selected).unwrap();
    changelist::update(&repo.root, |s| {
        s.move_paths(
            "default",
            &selected.iter().map(|p| p.path.clone()).collect::<Vec<_>>(),
        )
    })
    .unwrap();
    // Reload accepts the index without altering the rows or the move's paths.
    changelist::record_index(&repo.root, &repo::open(&repo.root).unwrap()).unwrap();
    assert!(changes(&repo).unversioned.is_empty());
    commit(&repo, Some(selected));
    assert_eq!(repo.git(&["rev-parse", "HEAD^{tree}"]), expected);
    assert!(repo.git(&["status", "--porcelain"]).is_empty());
}
