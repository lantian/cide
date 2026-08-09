//! The cases that silently break naive patch synthesis, one test each, plus the end-to-end
//! staging, commit and changelist behaviour the milestone is judged on.
//!
//! Every test builds a real repository in a temp directory and compares against the `git`
//! binary wherever a comparison is meaningful — `git add` for whole-file staging,
//! `git ls-files --stage` for the index, `git show` for what actually landed in a commit.

mod support;

use std::collections::BTreeSet;

use cide_git::diff::{self, DiffRequest};
use cide_git::{changelist, commit, patch, push, repo as repo_mod, shelf, stage, stash, status};
use cide_ipc::git::{
    CommitRequest, DiffSide, GitError, LineRef, PartialRefusal, PathSelection, Selection,
};
use support::{Rng, TempRepo, binary_blob};

/// Fifteen lines, edited at line 1 and line 15: far enough apart that three lines of context
/// leave two separate hunks, which is what the hunk-level tests are about.
const BASE_15: &[u8] = b"a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\n";
const EDITED_15: &[u8] = b"A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nO\n";
const FIRST_HUNK_UNSTAGED: &[u8] = b"a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nO\n";
const SECOND_HUNK_ROLLED_BACK: &[u8] = b"A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\n";

fn selection(path: &str, sel: Selection) -> PathSelection {
    PathSelection {
        path: path.into(),
        selection: sel,
        rev: None,
    }
}

fn raw(repo: &TempRepo, path: &str, side: DiffSide) -> cide_git::diff::RawFile {
    let git_repo = git2::Repository::open(&repo.root).expect("open");
    diff::file_diff(&git_repo, path, DiffRequest::new(side))
        .expect("diff")
        .unwrap_or_else(|| panic!("{path} has no changes on {side:?}"))
}

/// Our whole-file staging must be indistinguishable from `git add`.
fn assert_matches_git_add(repo: &TempRepo, paths: &[&str]) {
    let before = repo.save_index();
    let selections: Vec<PathSelection> = paths
        .iter()
        .map(|p| selection(p, Selection::Whole))
        .collect();
    stage::stage(&repo.root, &selections).expect("stage");
    let ours = repo.index_state();

    repo.restore_index(&before);
    let mut args = vec!["add", "--"];
    args.extend(paths);
    repo.git(&args);
    let theirs = repo.index_state();

    assert_eq!(ours, theirs, "whole-file staging diverged from `git add`");
}

// --- the cases that break naive synthesis --------------------------------------------------

#[test]
fn crlf_content_stages_by_hunk() {
    let repo = TempRepo::new("crlf");
    repo.write("f.txt", b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
    repo.commit_all("base");
    repo.write("f.txt", b"one\r\nTWO\r\nthree\r\nfour\r\nFIVE\r\n");

    let file = raw(&repo, "f.txt", DiffSide::Unstaged);
    assert_eq!(
        file.hunks.len(),
        1,
        "three context lines merge the two edits"
    );
    // Every line still carries its `\r`; a synthesizer that trimmed it would produce a patch
    // whose context does not match the pre-image.
    assert!(
        file.hunks[0]
            .lines
            .iter()
            .all(|l| l.content.ends_with(b"\r\n")),
        "CRLF was lost on the way out of libgit2"
    );

    let chosen = BTreeSet::from([(0usize, 1usize), (0, 2)]);
    let text = patch::synthesize(&file, &chosen).unwrap().unwrap();
    let (ok, output) = repo.try_git_stdin(&["apply", "--cached", "-"], &text);
    assert!(
        ok,
        "git refused a CRLF patch:\n{output}\n{}",
        String::from_utf8_lossy(&text)
    );
}

/// `core.autocrlf=true` puts CRLF in the working tree and LF in the index. The diff libgit2
/// produces is post-filter, so the patch is in LF and `git apply --cached` — which also works
/// on index content — has to agree.
#[test]
fn autocrlf_stages_by_hunk() {
    let repo = TempRepo::new("autocrlf");
    repo.git(&["config", "core.autocrlf", "true"]);
    repo.write("f.txt", b"one\ntwo\nthree\nfour\nfive\n");
    repo.commit_all("base");
    // What a Windows editor would leave behind.
    repo.write("f.txt", b"one\r\nTWO\r\nthree\r\nfour\r\nfive\r\n");

    let file = raw(&repo, "f.txt", DiffSide::Unstaged);
    let chosen = patch::every_change(&file);
    let text = patch::synthesize(&file, &chosen).unwrap().unwrap();

    let before = repo.save_index();
    let git_repo = git2::Repository::open(&repo.root).unwrap();
    let parsed = git2::Diff::from_buffer(&text).unwrap();
    git_repo
        .apply(&parsed, git2::ApplyLocation::Index, None)
        .expect("libgit2 apply");
    drop(git_repo);
    let ours = repo.index_state();

    repo.restore_index(&before);
    let (ok, output) = repo.try_git_stdin(&["apply", "--cached", "-"], &text);
    assert!(ok, "git refused an autocrlf patch:\n{output}");
    assert_eq!(ours, repo.index_state());
}

#[test]
fn a_file_with_no_trailing_newline_keeps_its_marker() {
    let repo = TempRepo::new("nonl");
    repo.write("f.txt", b"one\ntwo\nthree");
    repo.commit_all("base");
    repo.write("f.txt", b"one\nTWO\nTHREE");

    let file = raw(&repo, "f.txt", DiffSide::Unstaged);
    assert!(
        file.hunks[0].lines.iter().any(|l| l.eofnl.is_some()),
        "no `\\ No newline` marker survived the diff"
    );

    let text = patch::synthesize(&file, &patch::every_change(&file))
        .unwrap()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&text).ends_with("\\ No newline at end of file\n"),
        "{}",
        String::from_utf8_lossy(&text)
    );
    let (ok, output) = repo.try_git_stdin(&["apply", "--cached", "-"], &text);
    assert!(ok, "git refused it:\n{output}");
    assert_eq!(
        repo.git(&["show", ":f.txt"]),
        "one\nTWO\nTHREE",
        "the staged blob grew or lost a newline"
    );
}

/// Deselecting the deletion of an unterminated last line while selecting something after it
/// describes a file that both does and does not end in a newline. Refused, not guessed at.
#[test]
fn a_stranded_no_newline_marker_is_refused() {
    let repo = TempRepo::new("stranded");
    repo.write("f.txt", b"one\ntwo");
    repo.commit_all("base");
    repo.write("f.txt", b"one\ntwo\nthree\n");

    let file = raw(&repo, "f.txt", DiffSide::Unstaged);
    let deletion = file.hunks[0]
        .lines
        .iter()
        .position(|l| l.origin == b'-')
        .expect("a deletion");
    let last_addition = file.hunks[0]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.origin == b'+')
        .map(|(i, _)| i)
        .next_back()
        .expect("an addition");
    assert!(deletion < last_addition);

    let error = patch::synthesize(&file, &BTreeSet::from([(0, last_addition)])).unwrap_err();
    assert_eq!(
        error,
        GitError::PartialRefused {
            path: "f.txt".into(),
            reason: PartialRefusal::NoNewlineOrdering
        }
    );
}

#[test]
fn a_mode_change_with_edits_stages_the_mode_too() {
    let repo = TempRepo::new("mode");
    repo.write("s.sh", b"one\ntwo\nthree\n");
    repo.commit_all("base");
    repo.write("s.sh", b"one\nTWO\nthree\n");
    // Only the working tree changes: `git update-index --chmod` would stage the content
    // along with the bit, and there would be nothing left to select.
    std::fs::set_permissions(
        repo.root.join("s.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let file = raw(&repo, "s.sh", DiffSide::Unstaged);
    assert_ne!(
        file.old_mode, file.new_mode,
        "libgit2 did not see the mode change"
    );

    let text = patch::synthesize(&file, &patch::every_change(&file))
        .unwrap()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&text).contains("new mode 100755"),
        "the mode lines were dropped from the header:\n{}",
        String::from_utf8_lossy(&text)
    );

    let before = repo.save_index();
    let git_repo = git2::Repository::open(&repo.root).unwrap();
    let parsed = git2::Diff::from_buffer(&text).unwrap();
    git_repo
        .apply(&parsed, git2::ApplyLocation::Index, None)
        .expect("libgit2 apply");
    drop(git_repo);
    let ours = repo.index_state();
    assert!(
        ours.starts_with("100755"),
        "the mode did not reach the index: {ours}"
    );

    repo.restore_index(&before);
    let (ok, output) = repo.try_git_stdin(&["apply", "--cached", "-"], &text);
    assert!(ok, "git refused a mode-change patch:\n{output}");
    assert_eq!(ours, repo.index_state());
}

#[test]
fn a_binary_file_refuses_partial_staging_and_stages_whole() {
    let repo = TempRepo::new("binary");
    let mut rng = Rng::new(7);
    let mut blob = binary_blob(&mut rng, 512);
    blob[3] = 0;
    repo.write("b.bin", &blob);
    repo.commit_all("base");
    let mut changed = binary_blob(&mut rng, 512);
    changed[9] = 0;
    repo.write("b.bin", &changed);

    let file = raw(&repo, "b.bin", DiffSide::Unstaged);
    assert!(file.binary, "libgit2 did not call it binary");
    assert_eq!(file.partial_refusal(), Some(PartialRefusal::Binary));
    assert_eq!(
        patch::synthesize(&file, &BTreeSet::from([(0, 0)])).unwrap_err(),
        GitError::PartialRefused {
            path: "b.bin".into(),
            reason: PartialRefusal::Binary
        }
    );

    assert_matches_git_add(&repo, &["b.bin"]);
}

#[test]
fn a_submodule_refuses_partial_staging_and_stages_whole() {
    let inner = TempRepo::new("sub-inner");
    inner.write("a.txt", b"one\n");
    inner.commit_all("inner base");

    let outer = TempRepo::new("sub-outer");
    outer.write("top.txt", b"top\n");
    outer.commit_all("outer base");
    outer.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        inner.root.to_str().unwrap(),
        "sub",
    ]);
    outer.commit_all("add submodule");

    // Move the submodule's HEAD, which is what makes the gitlink dirty.
    inner.write("a.txt", b"two\n");
    inner.commit_all("inner moves");
    outer.git(&["-C", "sub", "fetch", "-q", "origin"]);
    outer.git(&["-C", "sub", "checkout", "-q", "main"]);
    outer.git(&["-C", "sub", "pull", "-q", "origin", "main"]);

    let file = raw(&outer, "sub", DiffSide::Unstaged);
    assert_eq!(file.partial_refusal(), Some(PartialRefusal::Submodule));
    assert_matches_git_add(&outer, &["sub"]);

    // And the submodule is discovered as a repository of its own, not an opaque entry.
    let repos = repo_mod::discover(std::slice::from_ref(&outer.root));
    assert_eq!(repos.len(), 2, "{repos:#?}");
    assert!(repos[1].is_submodule);
    assert_eq!(repos[1].parent, Some(repos[0].id));
    assert_eq!(repos[1].name, "sub");
}

/// `git add -N` puts a zero-oid entry in the index. The diff against the working tree is a
/// whole-file addition, and partial staging of it has to reach the same index git does.
#[test]
fn an_intent_to_add_entry_stages_by_hunk() {
    let repo = TempRepo::new("ita");
    repo.write("seed.txt", b"seed\n");
    repo.commit_all("base");
    repo.write("n.txt", b"one\ntwo\nthree\nfour\n");
    repo.git(&["add", "-N", "n.txt"]);

    let file = raw(&repo, "n.txt", DiffSide::Unstaged);
    let all = patch::every_change(&file);
    assert_eq!(
        all.len(),
        4,
        "an intent-to-add file should diff as four additions"
    );

    // Take the first two lines only.
    let chosen: BTreeSet<_> = all.into_iter().take(2).collect();
    let text = patch::synthesize(&file, &chosen).unwrap().unwrap();

    let before = repo.save_index();
    let git_repo = git2::Repository::open(&repo.root).unwrap();
    let parsed = git2::Diff::from_buffer(&text).unwrap();
    let ours = git_repo.apply(&parsed, git2::ApplyLocation::Index, None);
    drop(git_repo);
    let ours = ours.map(|()| repo.index_state());

    repo.restore_index(&before);
    let (ok, output) = repo.try_git_stdin(&["apply", "--cached", "-"], &text);

    match (ours, ok) {
        (Ok(ours), true) => assert_eq!(ours, repo.index_state()),
        (Err(e), false) => panic!(
            "neither libgit2 nor git could apply an intent-to-add patch: {e} / {output}\n{}",
            String::from_utf8_lossy(&text)
        ),
        (ours, ok) => panic!(
            "libgit2 and git disagree about an intent-to-add patch: libgit2={ours:?} git={ok} {output}\n{}",
            String::from_utf8_lossy(&text)
        ),
    }
}

/// A rename with edits is one indivisible delta: the `rename from`/`rename to` headers and
/// the content hunks cannot be separated. Partial staging is refused; whole staging goes
/// through the index and matches `git add`.
#[test]
fn a_rename_with_edits_refuses_partial_staging() {
    let repo = TempRepo::new("rename");
    repo.write("a.txt", b"one\ntwo\nthree\nfour\nfive\nsix\n");
    repo.commit_all("base");
    repo.git(&["mv", "a.txt", "b.txt"]);
    repo.write("b.txt", b"one\nTWO\nthree\nfour\nfive\nSIX\n");

    let git_repo = git2::Repository::open(&repo.root).unwrap();
    let request = DiffRequest::new(DiffSide::Combined).renames(true);
    let diff = diff::build(&git_repo, request, None).unwrap();
    let files = diff::raw_files(&diff).unwrap();
    let renamed = files
        .iter()
        .find(|f| f.status == git2::Delta::Renamed)
        .unwrap_or_else(|| panic!("no rename found in {files:#?}"));
    assert_eq!(renamed.old_path.as_deref(), Some("a.txt"));
    assert_eq!(renamed.partial_refusal(), Some(PartialRefusal::Rename));
    drop(diff);
    drop(git_repo);

    // `git mv` already staged the rename; the working-tree edit is what is left to stage.
    assert_matches_git_add(&repo, &["b.txt"]);
}

// --- the milestone's own acceptance checks -------------------------------------------------

#[test]
fn three_of_seven_lines_reach_the_commit() {
    let repo = TempRepo::new("three-of-seven");
    repo.write("f.txt", b"head\n");
    repo.commit_all("base");
    repo.write("f.txt", b"head\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n");

    let file = raw(&repo, "f.txt", DiffSide::Combined);
    let additions: Vec<(usize, usize)> = file
        .hunks
        .iter()
        .enumerate()
        .flat_map(|(h, hunk)| {
            hunk.lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.origin == b'+')
                .map(move |(i, _)| (h, i))
        })
        .collect();
    assert_eq!(additions.len(), 7);

    let picked: Vec<LineRef> = [1usize, 3, 5]
        .iter()
        .map(|i| LineRef {
            hunk: additions[*i].0 as u32,
            line: additions[*i].1 as u32,
        })
        .collect();

    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "three of seven".into(),
            amend: false,
            changelist: None,
            selections: Some(vec![selection("f.txt", Selection::Lines { lines: picked })]),
            force: false,
        },
    )
    .expect("commit");

    let shown = repo.git(&["show", "--format=", "HEAD"]);
    let added: Vec<&str> = shown
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .collect();
    assert_eq!(added, vec!["+l2", "+l4", "+l6"], "{shown}");
    // And the other four are still sitting in the working tree.
    assert_eq!(
        repo.read("f.txt"),
        b"head\nl1\nl2\nl3\nl4\nl5\nl6\nl7\n".to_vec()
    );
}

#[test]
fn an_external_git_add_trips_the_guard() {
    let repo = TempRepo::new("guard");
    repo.write("a.txt", b"one\n");
    repo.write("b.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a.txt", b"two\n");
    repo.write("b.txt", b"two\n");

    // cide stages something, which is what records the fingerprint.
    stage::stage(&repo.root, &[selection("a.txt", Selection::Whole)]).expect("stage");

    // …and then the user runs `git add` in a bash pane.
    repo.git(&["add", "b.txt"]);

    let request = CommitRequest {
        message: "guarded".into(),
        amend: false,
        changelist: None,
        selections: None,
        force: false,
    };
    let error = commit::commit(&repo.root, &request).unwrap_err();
    assert!(
        matches!(error, GitError::IndexChangedExternally { .. }),
        "{error:?}"
    );

    // The status tree says so too, so the panel can draw the bar without trying to commit.
    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    let changes = status::repo_changes(&info, status::StatusRequest::default()).unwrap();
    assert!(changes.index_changed_externally);

    // Overwriting is a deliberate second gesture, never a default.
    let forced = CommitRequest {
        force: true,
        ..request
    };
    commit::commit(&repo.root, &forced).expect("forced commit");
    assert!(
        !status::repo_changes(&info, status::StatusRequest::default())
            .unwrap()
            .index_changed_externally
    );
}

#[test]
fn committing_one_changelist_leaves_the_other_untouched() {
    let repo = TempRepo::new("changelists");
    repo.write("a.txt", b"one\n");
    repo.write("b.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a.txt", b"changed a\n");
    repo.write("b.txt", b"changed b\n");

    let fixes = changelist::update(&repo.root, |data| data.create("Fixes", "")).unwrap();
    changelist::update(&repo.root, |data| {
        data.move_paths(&fixes, &["b.txt".to_string()])
    })
    .unwrap();

    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "only the default list".into(),
            amend: false,
            changelist: None,
            selections: None,
            force: false,
        },
    )
    .expect("commit");

    assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "changed a\n");
    assert_eq!(repo.git(&["show", "HEAD:b.txt"]), "one\n");
    // `b.txt` is still dirty and still filed under `Fixes`.
    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    let changes = status::repo_changes(&info, status::StatusRequest::default()).unwrap();
    let fixes_view = changes
        .changelists
        .iter()
        .find(|l| l.id == fixes)
        .expect("the Fixes list");
    assert_eq!(fixes_view.changes.len(), 1);
    assert_eq!(fixes_view.changes[0].path, "b.txt");
}

/// `rebuild_index` clears the index to HEAD before it knows whether there is anything to
/// commit, so the emptiness has to be answered first — otherwise clicking Commit on a
/// changelist that holds nothing silently drops whatever the index was holding.
#[test]
fn committing_an_empty_changelist_leaves_the_index_alone() {
    let repo = TempRepo::new("empty-changelist");
    repo.write("a.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a.txt", b"two\n");

    stage::stage(&repo.root, &[selection("a.txt", Selection::Whole)]).expect("stage");
    let fixes = changelist::update(&repo.root, |data| data.create("Fixes", "")).unwrap();
    changelist::update(&repo.root, |data| {
        data.move_paths(&fixes, &["a.txt".to_string()])
    })
    .unwrap();

    let error = commit::commit(
        &repo.root,
        &CommitRequest {
            message: "nothing here".into(),
            amend: false,
            changelist: None,
            selections: None,
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(error, GitError::NothingToCommit);
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]).trim(),
        "a.txt",
        "a refused commit unstaged a file it never named"
    );
    // …and the guard must not now accuse the user of staging behind cide's back.
    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    assert!(
        !status::repo_changes(&info, status::StatusRequest::default())
            .unwrap()
            .index_changed_externally
    );
}

#[test]
fn unstaging_selected_lines_puts_back_exactly_those() {
    let repo = TempRepo::new("unstage");
    // Long enough that the two edits are three context lines apart and stay separate hunks.
    repo.write("f.txt", BASE_15);
    repo.commit_all("base");
    repo.write("f.txt", EDITED_15);
    stage::stage(&repo.root, &[selection("f.txt", Selection::Whole)]).expect("stage");
    assert_eq!(repo.git(&["show", ":f.txt"]).as_bytes(), EDITED_15);

    // Take the first hunk back out; the second must survive.
    let staged = raw(&repo, "f.txt", DiffSide::Staged);
    assert_eq!(staged.hunks.len(), 2, "{staged:#?}");
    stage::unstage(
        &repo.root,
        &[selection("f.txt", Selection::Hunks { hunks: vec![0] })],
    )
    .expect("unstage");

    assert_eq!(
        repo.git(&["show", ":f.txt"]).as_bytes(),
        FIRST_HUNK_UNSTAGED
    );
    // The working tree is untouched by unstaging — that is the whole point of it.
    assert_eq!(repo.read("f.txt"), EDITED_15.to_vec());
}

#[test]
fn rollback_discards_only_the_selected_lines() {
    let repo = TempRepo::new("rollback");
    repo.write("f.txt", BASE_15);
    repo.commit_all("base");
    repo.write("f.txt", EDITED_15);

    let file = raw(&repo, "f.txt", DiffSide::Combined);
    assert_eq!(file.hunks.len(), 2);
    stage::rollback(
        &repo.root,
        &[selection("f.txt", Selection::Hunks { hunks: vec![1] })],
    )
    .expect("rollback");

    assert_eq!(repo.read("f.txt"), SECOND_HUNK_ROLLED_BACK.to_vec());
}

#[test]
fn rolling_an_untracked_file_back_removes_it() {
    let repo = TempRepo::new("rollback-untracked");
    repo.write("seed.txt", b"seed\n");
    repo.commit_all("base");
    repo.write("junk.txt", b"one\ntwo\n");

    stage::rollback(&repo.root, &[selection("junk.txt", Selection::Whole)]).expect("rollback");
    assert!(!repo.root.join("junk.txt").exists());
}

/// libgit2 takes *pathspecs*, not paths, in `checkout_head` and `reset_default`. A file whose
/// name contains an fnmatch metacharacter would otherwise drag its neighbours along — and on
/// the rollback path that means overwriting a file the user never named with its HEAD content.
#[test]
fn a_glob_shaped_filename_only_rolls_back_itself() {
    let repo = TempRepo::new("glob-rollback");
    repo.write("a[1].txt", b"one\n");
    repo.write("a1.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a[1].txt", b"bracket edit\n");
    repo.write("a1.txt", b"plain edit\n");

    stage::rollback(&repo.root, &[selection("a[1].txt", Selection::Whole)]).expect("rollback");

    assert_eq!(repo.read("a[1].txt"), b"one\n".to_vec());
    assert_eq!(
        repo.read("a1.txt"),
        b"plain edit\n".to_vec(),
        "rolling back `a[1].txt` destroyed uncommitted work in `a1.txt`"
    );
}

#[test]
fn a_glob_shaped_filename_only_unstages_itself() {
    let repo = TempRepo::new("glob-unstage");
    repo.write("a[1].txt", b"one\n");
    repo.write("a1.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a[1].txt", b"bracket edit\n");
    repo.write("a1.txt", b"plain edit\n");
    stage::stage(
        &repo.root,
        &[
            selection("a[1].txt", Selection::Whole),
            selection("a1.txt", Selection::Whole),
        ],
    )
    .expect("stage");

    stage::unstage(&repo.root, &[selection("a[1].txt", Selection::Whole)]).expect("unstage");

    let staged = repo.git(&["diff", "--cached", "--name-only"]);
    assert_eq!(
        staged.lines().collect::<Vec<_>>(),
        vec!["a1.txt"],
        "unstaging `a[1].txt` also unstaged its neighbour"
    );
}

/// The corrupting case behind [`PartialRefusal::NoNewlineOrdering`], end to end.
///
/// Old file `b` with no terminator, new file `b\nc` also with none, so libgit2 prints two
/// markers in one hunk. Staging only the added line turns the deletion into context and
/// strands the first marker; `git apply --cached` *accepts* that patch and stages `bc`.
#[test]
fn staging_past_a_stranded_marker_is_refused_rather_than_corrupting_the_blob() {
    let repo = TempRepo::new("two-markers");
    repo.write("f.txt", b"b");
    repo.commit_all("base");
    repo.write("f.txt", b"b\nc");

    let file = raw(&repo, "f.txt", DiffSide::Unstaged);
    let additions: Vec<(usize, usize)> = file.hunks[0]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.origin == b'+')
        .map(|(i, _)| (0usize, i))
        .collect();
    assert_eq!(additions.len(), 2, "{file:#?}");
    let last = *additions.last().unwrap();

    for chosen in [BTreeSet::from([last]), additions.iter().copied().collect()] {
        assert_eq!(
            patch::synthesize(&file, &chosen).unwrap_err(),
            GitError::PartialRefused {
                path: "f.txt".into(),
                reason: PartialRefusal::NoNewlineOrdering
            }
        );
    }

    // Selecting everything is the whole file, and that still round-trips byte for byte.
    let all = patch::every_change(&file);
    let text = patch::synthesize(&file, &all).unwrap().unwrap();
    let (ok, output) = repo.try_git_stdin(&["apply", "--cached", "-"], &text);
    assert!(ok, "git refused the full patch:\n{output}");
    assert_eq!(repo.git(&["show", ":f.txt"]), "b\nc");
}

#[test]
fn a_stale_selection_is_refused_rather_than_applied_to_moved_lines() {
    let repo = TempRepo::new("stale");
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("base");
    repo.write("f.txt", b"A\nb\nc\n");

    let file = raw(&repo, "f.txt", DiffSide::Unstaged);
    let stale = PathSelection {
        path: "f.txt".into(),
        selection: Selection::Hunks { hunks: vec![0] },
        rev: Some(file.rev()),
    };

    // Something else edits the file between the panel drawing it and the click landing.
    repo.write("f.txt", b"A\nB\nc\n");
    let error = stage::stage(&repo.root, &[stale]).unwrap_err();
    assert!(
        matches!(error, GitError::StaleSelection { .. }),
        "{error:?}"
    );
}

// --- shelf, stash, push ---------------------------------------------------------------------

#[test]
fn shelving_takes_the_change_away_and_unshelving_brings_it_back() {
    let repo = TempRepo::new("shelf");
    repo.write("f.txt", b"a\nb\nc\n");
    repo.commit_all("base");
    repo.write("f.txt", b"A\nb\nC\n");

    let entry = shelf::shelve(
        &repo.root,
        "work in progress",
        &[selection("f.txt", Selection::Whole)],
    )
    .expect("shelve");
    assert_eq!(repo.read("f.txt"), b"a\nb\nc\n".to_vec());
    assert_eq!(shelf::list(&repo.root).len(), 1);

    shelf::unshelve(&repo.root, &entry.id, false).expect("unshelve");
    assert_eq!(repo.read("f.txt"), b"A\nb\nC\n".to_vec());
    assert!(shelf::list(&repo.root).is_empty());
}

#[test]
fn the_shelf_and_the_stash_are_separate() {
    let repo = TempRepo::new("shelf-vs-stash");
    repo.write("f.txt", b"a\n");
    repo.commit_all("base");
    repo.write("f.txt", b"A\n");

    stash::save(&repo.root, "stashed", false).expect("stash");
    assert_eq!(repo.read("f.txt"), b"a\n".to_vec());
    assert_eq!(stash::list(&repo.root).unwrap().len(), 1);
    // The shelf knows nothing about it, and vice versa.
    assert!(shelf::list(&repo.root).is_empty());

    stash::pop(&repo.root, 0).expect("pop");
    assert_eq!(repo.read("f.txt"), b"A\n".to_vec());
    assert!(stash::list(&repo.root).unwrap().is_empty());
    assert!(matches!(
        stash::pop(&repo.root, 0),
        Err(GitError::NoSuchStash { index: 0 })
    ));
}

#[test]
fn a_local_remote_is_pushed_by_libgit2_and_https_is_shelled_out() {
    let repo = TempRepo::new("push");
    repo.write("f.txt", b"a\n");
    repo.commit_all("base");

    let bare = std::env::temp_dir().join(format!("cide-git-bare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bare);
    let status = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&bare)
        .status()
        .unwrap();
    assert!(status.success());
    repo.git(&["remote", "add", "origin", bare.to_str().unwrap()]);

    let git_repo = git2::Repository::open(&repo.root).unwrap();
    assert_eq!(push::route(&git_repo, "origin"), push::Route::Libgit2);
    drop(git_repo);

    let outcome = push::push(&repo.root, None, None, true).expect("push");
    assert!(!outcome.shelled_out);
    let remote_log = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&bare)
        .args(["log", "--format=%s", "-1", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&remote_log.stdout).trim(), "base");
    // `-u` has to write the two config keys itself; libgit2's push does not.
    assert_eq!(repo.git(&["config", "branch.main.remote"]).trim(), "origin");

    // An https remote cannot be authenticated from inside libgit2, so it takes the binary.
    repo.git(&["remote", "add", "web", "https://example.invalid/x.git"]);
    let git_repo = git2::Repository::open(&repo.root).unwrap();
    assert_eq!(push::route(&git_repo, "web"), push::Route::Binary);
    drop(git_repo);

    let _ = std::fs::remove_dir_all(&bare);
}

#[test]
fn a_credential_helper_forces_the_binary_even_for_a_local_remote() {
    let repo = TempRepo::new("push-helper");
    repo.write("f.txt", b"a\n");
    repo.commit_all("base");
    repo.git(&["remote", "add", "origin", "/tmp/does-not-matter"]);
    repo.git(&["config", "credential.helper", "store"]);

    let git_repo = git2::Repository::open(&repo.root).unwrap();
    assert_eq!(push::route(&git_repo, "origin"), push::Route::Binary);
}

// --- multi-root ------------------------------------------------------------------------------

#[test]
fn several_roots_inside_one_repository_produce_one_entry() {
    let repo = TempRepo::new("multiroot");
    repo.write("a/one.txt", b"one\n");
    repo.write("b/two.txt", b"two\n");
    repo.commit_all("base");

    let repos = repo_mod::discover(&[repo.root.join("a"), repo.root.join("b")]);
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].root, repo_mod::canonical(&repo.root));
}

#[test]
fn a_root_outside_any_repository_is_skipped_rather_than_failing_the_tree() {
    let repo = TempRepo::new("mixed");
    repo.write("f.txt", b"a\n");
    repo.commit_all("base");

    let loose = std::env::temp_dir().join(format!("cide-git-loose-{}", std::process::id()));
    std::fs::create_dir_all(&loose).unwrap();
    let tree = status::changes_tree(
        &[repo.root.clone(), loose.clone()],
        status::StatusRequest::default(),
    )
    .unwrap();
    assert_eq!(tree.repos.len(), 1);
    let _ = std::fs::remove_dir_all(&loose);
}

#[test]
fn the_sidecar_lives_under_the_state_directory_keyed_by_the_root() {
    let repo = TempRepo::new("sidecar-path");
    repo.write("f.txt", b"a\n");
    repo.commit_all("base");
    changelist::update(&repo.root, |data| data.create("Fixes", "")).unwrap();

    let expected = cide_core::persist::state_dir()
        .join("repos")
        .join(repo_mod::repo_key(&repo.root))
        .join("changelists.json");
    assert!(expected.is_file(), "{} was not written", expected.display());
    assert!(
        changelist::load(&repo.root)
            .lists
            .iter()
            .any(|l| l.name == "Fixes")
    );
}
