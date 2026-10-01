//! Message generation must describe the commit without building or rewriting its index.
mod support;

use cide_git::{changelist, diff, message};
use cide_ipc::git::{DiffSide, GitError, LineRef, PathSelection, Selection};
use support::TempRepo;

fn describe(repo: &TempRepo, selections: &[PathSelection]) -> cide_git::Result<String> {
    let index = std::fs::read(repo.root.join(".git/index")).ok();
    let head = std::fs::read(repo.root.join(".git/HEAD")).unwrap();
    let sidecar = std::fs::read(cide_git::sidecar::changelists_path(&repo.root)).ok();
    let changes = repo.git(&["diff", "HEAD"]);
    let result = message::commit_diff(&repo.root, selections);
    assert_eq!(std::fs::read(repo.root.join(".git/index")).ok(), index);
    assert_eq!(std::fs::read(repo.root.join(".git/HEAD")).unwrap(), head);
    assert_eq!(
        std::fs::read(cide_git::sidecar::changelists_path(&repo.root)).ok(),
        sidecar
    );
    assert_eq!(repo.git(&["diff", "HEAD"]), changes);
    result
}

fn fixture(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("picked.txt", b"one\ntwo\nthree\n");
    repo.write("unchecked.txt", b"original\n");
    repo.commit_all("base");
    repo
}

#[test]
fn selected_changes_use_the_working_tree_even_when_other_changes_are_staged() {
    let repo = fixture("message-selected");
    repo.write("picked.txt", b"one\nSTAGED\nthree\n");
    repo.git(&["add", "picked.txt"]);
    repo.write("picked.txt", b"one\nWORKTREE\nthree\n");
    repo.write("unchecked.txt", b"UNSELECTED\n");
    repo.git(&["add", "unchecked.txt"]);
    let text = describe(&repo, &[PathSelection::whole("picked.txt")]).unwrap();
    assert!(text.contains("+WORKTREE"));
    assert!(text.contains("-two"));
    assert!(!text.contains("STAGED"));
    assert!(!text.contains("UNSELECTED"));
    assert!(!text.contains("unchecked.txt"));
    assert!(describe(&repo, &[]).unwrap().is_empty());
}

#[test]
fn staged_mode_reads_the_whole_index_and_never_falls_back_to_unstaged_changes() {
    let repo = fixture("message-staged");
    changelist::update(&repo.root, |state| {
        state.use_staging_area = true;
        Ok(())
    })
    .unwrap();
    repo.write("picked.txt", b"one\nINDEX\nthree\n");
    repo.write("unchecked.txt", b"OTHER_INDEX\n");
    repo.git(&["add", "."]);
    repo.write("picked.txt", b"one\nUNSTAGED\nthree\n");
    let text = describe(&repo, &[PathSelection::whole("picked.txt")]).unwrap();
    assert!(text.contains("+INDEX"));
    assert!(text.contains("+OTHER_INDEX"));
    assert!(!text.contains("UNSTAGED"));
    repo.git(&["reset", "-q", "HEAD"]);
    assert!(
        describe(&repo, &[PathSelection::whole("picked.txt")])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn individual_lines_exclude_unselected_additions_and_stale_positions_are_refused() {
    let repo = fixture("message-lines");
    repo.write("picked.txt", b"one\nPICK_ME\nSKIP_ME\ntwo\nthree\n");
    let git = cide_git::repo::open(&repo.root).unwrap();
    let file = diff::file_diff(
        &git,
        "picked.txt",
        diff::DiffRequest::new(DiffSide::Combined),
    )
    .unwrap()
    .unwrap();
    let line = file.hunks[0]
        .lines
        .iter()
        .position(|line| line.content == b"PICK_ME\n")
        .unwrap();
    let selected = PathSelection {
        path: "picked.txt".into(),
        rev: Some(file.rev()),
        selection: Selection::Lines {
            lines: vec![LineRef {
                hunk: 0,
                line: line as u32,
            }],
        },
    };
    let text = describe(&repo, std::slice::from_ref(&selected)).unwrap();
    assert!(text.contains("+PICK_ME"));
    assert!(!text.contains("SKIP_ME"));
    repo.write("picked.txt", b"changed again\n");
    assert!(matches!(
        describe(&repo, &[selected]),
        Err(GitError::StaleSelection { .. })
    ));
}

#[test]
fn selected_hunks_exclude_other_hunks() {
    let repo = fixture("message-hunks");
    let original: String = (0..30).map(|n| format!("line {n}\n")).collect();
    repo.write("picked.txt", original.as_bytes());
    repo.commit_all("long file");
    repo.write(
        "picked.txt",
        original
            .replace("line 1\n", "FIRST\n")
            .replace("line 28\n", "LAST\n")
            .as_bytes(),
    );
    let git = cide_git::repo::open(&repo.root).unwrap();
    let file = diff::file_diff(
        &git,
        "picked.txt",
        diff::DiffRequest::new(DiffSide::Combined),
    )
    .unwrap()
    .unwrap();
    assert_eq!(file.hunks.len(), 2);
    let text = describe(
        &repo,
        &[PathSelection {
            path: "picked.txt".into(),
            rev: Some(file.rev()),
            selection: Selection::Hunks { hunks: vec![1] },
        }],
    )
    .unwrap();
    assert!(text.contains("+LAST"));
    assert!(!text.contains("FIRST"));
}

#[test]
fn new_files_binary_files_and_renames_can_be_described_whole() {
    let repo = fixture("message-special");
    repo.write("image.bin", b"\x00old\x00");
    repo.commit_all("binary base");
    repo.write("image.bin", b"\x00new\x00");
    repo.write("new.txt", "Unicode é\n".as_bytes());
    std::fs::rename(repo.root.join("picked.txt"), repo.root.join("renamed.txt")).unwrap();
    repo.git(&["update-index", "--chmod=+x", "unchecked.txt"]);
    let text = describe(
        &repo,
        &[
            PathSelection::whole("image.bin"),
            PathSelection::whole("new.txt"),
            PathSelection::whole("picked.txt"),
            PathSelection::whole("renamed.txt"),
        ],
    )
    .unwrap();
    assert!(text.contains("image.bin"));
    assert!(text.contains("+Unicode é"));
    assert!(text.contains("deleted file mode"));
    assert!(text.contains("+++ b/renamed.txt"));
    assert_eq!(
        std::fs::read(repo.root.join("image.bin")).unwrap(),
        b"\x00new\x00"
    );
    assert_eq!(
        std::fs::read(repo.root.join("new.txt")).unwrap(),
        "Unicode é\n".as_bytes()
    );
    assert!(repo.root.join("renamed.txt").exists());
    assert!(!repo.root.join("picked.txt").exists());
}

#[cfg(unix)]
#[test]
fn executable_mode_changes_have_a_diff_even_without_changed_lines() {
    use std::os::unix::fs::PermissionsExt;
    let repo = fixture("message-mode");
    let path = repo.root.join("picked.txt");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let text = describe(&repo, &[PathSelection::whole("picked.txt")]).unwrap();
    assert!(text.contains("old mode 100644"));
    assert!(text.contains("new mode 100755"));
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn an_unborn_repository_can_describe_selected_new_files() {
    let repo = TempRepo::new("message-unborn");
    repo.write("first.txt", b"first commit\n");
    let text = message::commit_diff(&repo.root, &[PathSelection::whole("first.txt")]).unwrap();
    assert!(text.contains("+first commit"));
    assert!(!repo.root.join(".git/index").exists());
}

#[test]
fn merge_conclusions_describe_the_index_instead_of_rebuilding_selected_files() {
    let repo = fixture("message-merge");
    repo.git(&["switch", "-qc", "other"]);
    repo.write("other.txt", b"OTHER_BRANCH\n");
    repo.commit_all("other branch");
    repo.git(&["switch", "-q", "main"]);
    repo.write("main.txt", b"MAIN_BRANCH\n");
    repo.commit_all("main branch");
    repo.git(&["merge", "--no-commit", "--no-ff", "other"]);
    repo.write("picked.txt", b"UNSTAGED\n");
    let text = describe(&repo, &[PathSelection::whole("picked.txt")]).unwrap();
    assert!(text.contains("+OTHER_BRANCH"));
    assert!(!text.contains("UNSTAGED"));
    assert!(repo.root.join(".git/MERGE_HEAD").exists());
}
