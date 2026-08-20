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

/// The proxy environment these tests spawn `git` with: none at all.
///
/// `ProxyTarget::Untouched` resolved, which is cide's default for its own `git` children and
/// the behaviour every one of these tests had before `ProxyScope` existed — the child gets
/// this process's environment unaltered. Named rather than inlined as `Default::default()` so
/// the reader of a `pull` call can see *which* of the three answers is being exercised.
fn untouched() -> cide_core::proxy::ProxyEnv {
    cide_core::proxy::ProxyEnv::default()
}

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
            amend_of: None,
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
        amend_of: None,
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
            amend_of: None,
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

/// The same guarantee for a file whose only existence as a change *is* its index entry.
///
/// > *"on commit changes that was moved from Uncommited to custom list goes into Uncommited
/// > again, but should stay in custom list."*
///
/// `commit::rebuild_index` resets the index to HEAD, which is what makes committing one
/// changelist leave the other's *tree* alone. It used to leave the other's **index** in
/// pieces, and for a `git add`ed new file the index is the whole of it: the reset dropped
/// `keep.txt`'s entry, the next status walk reported it `Untracked`, `repo_changes` leaves
/// untracked paths out of its `live` set, and `Sidecar::reconcile` then deleted the
/// assignment. A file the user had deliberately filed away came back as an unversioned row
/// after a commit that never named it.
///
/// Both halves are asserted because either alone would pass with the bug half-fixed: the
/// index entry is what keeps the file tracked, and the sidecar entry is what keeps it in the
/// list the user put it in.
#[test]
fn committing_one_changelist_leaves_the_others_staged_files_staged() {
    let repo = TempRepo::new("changelists-index");
    repo.write("a.txt", b"one\n");
    repo.write("gone.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a.txt", b"changed a\n");

    // A new file added to git but never committed — what `git status` prints as `A `, and
    // what cide's own `.cide/` files look like in the repository this was reported from.
    repo.write("keep.txt", b"keep\n");
    stage::stage(&repo.root, &[selection("keep.txt", Selection::Whole)]).expect("stage");
    // …and a staged *deletion*, which is the same erasure from the other direction: the
    // reset to HEAD puts the file back into the index, so restoring it means removing it.
    std::fs::remove_file(repo.root.join("gone.txt")).expect("remove");
    stage::stage(&repo.root, &[selection("gone.txt", Selection::Whole)]).expect("stage");

    let held = changelist::update(&repo.root, |data| data.create("Held", "")).unwrap();
    changelist::update(&repo.root, |data| {
        data.move_paths(&held, &["keep.txt".to_string(), "gone.txt".to_string()])
    })
    .unwrap();

    // The panel sends the ticked paths explicitly; `a.txt` is the only one in `Changes`.
    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "only a.txt".into(),
            amend: false,
            changelist: Some("default".into()),
            selections: Some(vec![selection("a.txt", Selection::Whole)]),
            force: false,
            amend_of: None,
        },
    )
    .expect("commit");

    assert_eq!(
        repo.git(&["show", "--name-only", "--format=", "HEAD"])
            .trim(),
        "a.txt",
        "the commit took exactly the changelist that was named"
    );
    let cached = repo.git(&["diff", "--cached", "--name-only"]);
    let mut staged: Vec<&str> = cached
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    staged.sort_unstable();
    assert_eq!(
        staged,
        ["gone.txt", "keep.txt"],
        "a commit of one changelist unstaged files belonging to another"
    );

    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    let changes = status::repo_changes(&info, status::StatusRequest::default()).expect("status");
    assert!(
        changes.unversioned.is_empty(),
        "`keep.txt` fell back to an unversioned row: {:?}",
        changes.unversioned
    );
    let list = changes
        .changelists
        .iter()
        .find(|l| l.id == held)
        .expect("the Held list");
    let mut still: Vec<&str> = list.changes.iter().map(|c| c.path.as_str()).collect();
    still.sort_unstable();
    assert_eq!(
        still,
        ["gone.txt", "keep.txt"],
        "the files stayed in the changelist the user filed them into"
    );
    // And the guard must not now accuse the user of staging behind cide's back: the restore
    // is cide's own write, so the fingerprint recorded after it has to describe it.
    assert!(!changes.index_changed_externally);
}

/// A commit that is refused after the rebuild has already run puts the index back.
///
/// `committing_an_empty_changelist_leaves_the_index_alone` covers the emptiness that is
/// answered *before* `rebuild_index`. This is the other half: `write_commit` refuses a commit
/// whose tree equals HEAD's, and by then the index has been reset. Without the restore, the
/// user's answer to "nothing to commit" would be a silent `git reset` of everything they had
/// staged in every other changelist.
#[test]
fn a_commit_refused_after_the_rebuild_puts_the_index_back() {
    let repo = TempRepo::new("refused-rebuild");
    repo.write("a.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a.txt", b"two\n");
    repo.write("keep.txt", b"keep\n");
    stage::stage(&repo.root, &[selection("keep.txt", Selection::Whole)]).expect("stage");

    // An identity git cannot resolve, which is what makes `write_commit` fail *after*
    // `rebuild_index` has already reset the index. Any refusal from there on would do; this
    // is the one a test can arrange without reaching inside the function.
    repo.git(&["config", "--unset", "user.email"]);
    repo.git(&["config", "--unset", "user.name"]);

    let error = commit::commit(
        &repo.root,
        &CommitRequest {
            message: "who am i".into(),
            amend: false,
            changelist: Some("default".into()),
            selections: Some(vec![selection("a.txt", Selection::Whole)]),
            force: false,
            amend_of: None,
        },
    )
    .unwrap_err();
    assert!(
        matches!(error, GitError::Git { .. }),
        "expected the signature to be what failed, got {error:?}"
    );
    repo.git(&["config", "user.name", "cide tests"]);
    repo.git(&["config", "user.email", "tests@cide.invalid"]);
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]).trim(),
        "keep.txt",
        "a refused commit unstaged a file it never named"
    );
    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    assert!(
        !status::repo_changes(&info, status::StatusRequest::default())
            .unwrap()
            .index_changed_externally
    );
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
            amend_of: None,
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

/// A changelist is a **sidecar**, not git state: `changelists.json` names paths, and nothing
/// stops a bash pane inside cide from committing one of them. This pins the answer to what
/// happens to the entry left behind — it converges on the next status walk rather than
/// accumulating, which is the whole reason `Sidecar::reconcile` runs from `repo_changes` and
/// not from the mutations.
///
/// The second walk is half the test. `reconcile` returns whether it dropped anything so the
/// caller can skip the write, and a version that always reported "changed" would rewrite the
/// file on every status refresh — several a second while an agent edits.
#[test]
fn a_changelist_entry_whose_file_was_committed_elsewhere_converges() {
    let repo = TempRepo::new("changelist-ghosts");
    repo.write("a.txt", b"one\n");
    repo.write("b.txt", b"one\n");
    repo.commit_all("base");
    repo.write("a.txt", b"changed a\n");
    repo.write("b.txt", b"changed b\n");

    let fixes = changelist::update(&repo.root, |data| data.create("Fixes", "")).unwrap();
    changelist::update(&repo.root, |data| {
        data.move_paths(&fixes, &["a.txt".to_string(), "b.txt".to_string()])
    })
    .unwrap();

    // A bash pane commits one of the two. Nothing tells the sidecar.
    repo.git(&["add", "b.txt"]);
    repo.git(&["commit", "-q", "-m", "committed outside cide"]);

    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    let only_a = BTreeSet::from(["a.txt".to_string()]);
    for pass in 1..=2 {
        status::repo_changes(&info, status::StatusRequest::default()).expect("status");
        assert_eq!(
            changelist::load(&repo.root).get(&fixes).unwrap().paths,
            only_a,
            "pass {pass}: the committed path is still filed under Fixes"
        );
    }
}

/// Filing an *untracked* path into a changelist does not stick: `repo_changes` computes its
/// `live` set from paths that are neither `Untracked` nor `Ignored`, so an assignment for one
/// is dropped by the very next status walk.
///
/// Pinned as a test rather than left as a comment because the whole git panel is built around
/// it. It used to be the reason *Move to Changelist…* was **disabled** on unversioned rows —
/// the alternative then being a menu item that appeared to work and silently undid itself a
/// moment later. It is now the reason that gesture *stages first*: see
/// `filing_an_untracked_path_after_adding_it_sticks` below, which is the same sequence with
/// the one missing step, and `ui/src/sidebar/GitPanel/dragDrop.ts`'s `track` outcome for the
/// gesture that runs it. The refusal survives for the ignored list, which has no such verb.
#[test]
fn filing_an_untracked_path_into_a_changelist_does_not_stick() {
    let repo = TempRepo::new("changelist-untracked");
    repo.write("a.txt", b"one\n");
    repo.commit_all("base");
    repo.write("new.txt", b"fresh\n");

    let fixes = changelist::update(&repo.root, |data| data.create("Fixes", "")).unwrap();
    changelist::update(&repo.root, |data| {
        data.move_paths(&fixes, &["new.txt".to_string()])
    })
    .unwrap();

    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    let changes = status::repo_changes(&info, status::StatusRequest::default()).expect("status");
    assert_eq!(
        changes.unversioned.len(),
        1,
        "it is still an unversioned row"
    );
    assert!(
        changelist::load(&repo.root)
            .get(&fixes)
            .unwrap()
            .paths
            .is_empty()
    );
}

/// The gesture the git panel calls *track*: add an unversioned file to git, **then** file it.
///
/// > *"i should be able to move files from Unversioned Files to any of change list via drag
/// > and drop and via context ... but when dragging it should stage them automatically."*
///
/// This is the whole claim of that feature, in the order it makes it. `stage::stage` over a
/// whole untracked path does what `git add` does, which takes the path out of `Untracked` and
/// therefore into `repo_changes`'s `live` set; only then does the assignment survive
/// `Sidecar::reconcile`. The reverse order is the test above — a write that undoes itself
/// roughly 150 ms later, which is what the panel used to refuse rather than perform.
///
/// The last assertion is the point of the exercise: committing that changelist commits the
/// file. A path filed into a list that a commit of the list would skip is a changelist entry
/// that lies, and `commit::default_selections` skips anything still `Untracked`.
#[test]
fn filing_an_untracked_path_after_adding_it_sticks() {
    let repo = TempRepo::new("changelist-track");
    repo.write("a.txt", b"one\n");
    repo.commit_all("base");
    repo.write("new.txt", b"fresh\n");

    let fixes = changelist::update(&repo.root, |data| data.create("Fixes", "")).unwrap();
    // Step one, exactly what `useGitPanel::trackPaths` sends first.
    stage::stage(&repo.root, &[selection("new.txt", Selection::Whole)]).expect("stage");
    // Step two, unchanged from the ordinary move.
    changelist::update(&repo.root, |data| {
        data.move_paths(&fixes, &["new.txt".to_string()])
    })
    .unwrap();

    let info = repo_mod::discover(std::slice::from_ref(&repo.root)).remove(0);
    let changes = status::repo_changes(&info, status::StatusRequest::default()).expect("status");
    assert!(
        changes.unversioned.is_empty(),
        "the file has left the Unversioned Files list: it is in the index now"
    );
    let list = changes
        .changelists
        .iter()
        .find(|l| l.id == fixes)
        .expect("the Fixes list");
    assert_eq!(
        list.changes
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        ["new.txt"],
        "and it is drawn in the changelist it was dropped on, after a full status walk — the \
         reconcile that drops an untracked path's assignment has already run here"
    );
    assert!(
        changelist::load(&repo.root)
            .get(&fixes)
            .unwrap()
            .paths
            .contains("new.txt"),
        "the sidecar kept it too"
    );

    // And the assignment means what it says: committing that list commits the new file.
    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "add the new file".into(),
            amend: false,
            changelist: Some(fixes.clone()),
            selections: None,
            force: false,
            amend_of: None,
        },
    )
    .expect("commit");
    assert!(
        repo.git(&["show", "--name-only", "--format=", "HEAD"])
            .contains("new.txt"),
        "an untracked path filed this way is a commit candidate, which is the only reason \
         filing it was worth doing"
    );
}

/// The composition the git panel's *Revert Changelist* performs: `git_rollback` over exactly
/// the paths in one list. The point is the second assertion — the other changelist is not
/// touched, which is the same guarantee `committing_one_changelist_leaves_the_other_untouched`
/// makes for commit and the reason changelists exist here at all.
#[test]
fn reverting_one_changelist_leaves_the_other_alone() {
    let repo = TempRepo::new("revert-changelist");
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

    // The paths come *out of the changelist*, exactly as `useGitPanel::revertGroup` takes them
    // out of the group it was right-clicked on. Naming `b.txt` literally here would have left
    // the test green with the sidecar removed entirely — it would no longer have been about
    // changelists at all.
    let filed: Vec<_> = changelist::load(&repo.root)
        .get(&fixes)
        .unwrap()
        .paths
        .iter()
        .map(|p| selection(p, Selection::Whole))
        .collect();
    assert_eq!(filed.len(), 1, "the group menu reverts what the list holds");
    stage::rollback(&repo.root, &filed).expect("rollback");

    assert_eq!(repo.read("b.txt"), b"one\n", "Fixes went back to HEAD");
    assert_eq!(
        repo.read("a.txt"),
        b"changed a\n",
        "the default changelist was not in the gesture and must not have moved"
    );
}

/// Why the group menu says **Delete** rather than **Revert** over the unversioned list.
///
/// HEAD has no pre-image for an untracked file, so `stage::rollback`'s "restore" is a delete.
/// The panel's confirmation is worded from this fact; a dialog promising to "revert" a file
/// that is about to be erased would be the most expensive kind of wrong.
#[test]
fn reverting_an_untracked_file_deletes_it() {
    let repo = TempRepo::new("revert-untracked");
    repo.write("a.txt", b"one\n");
    repo.commit_all("base");
    repo.write("new.txt", b"never committed\n");

    stage::rollback(&repo.root, &[selection("new.txt", Selection::Whole)]).expect("rollback");
    assert!(!repo.root.join("new.txt").exists());
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

    let outcome = push::push(&repo.root, None, None, true, &untouched()).expect("push");
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

/// **libgit2 never carries network traffic in cide, and this is the assertion behind that.**
///
/// It is load-bearing for the proxy scope, not a curiosity. `ProxyTarget` promises that
/// cide's influence over git's proxying is entirely a matter of the child's *environment* —
/// and that promise is only true because every remote that could reach a network forks the
/// `git` binary. libgit2 in this process reads no proxy environment (its `lookup_proxy`
/// returns early for `GIT_PROXY_NONE`, which is the default this crate never moves off; see
/// `push.rs`'s module docs for the file and line), so a URL that reached the git2 route
/// *would* be a URL cide could not scope — silently, and only for the user whose remote
/// happened to be spelled that way.
///
/// So the shapes are enumerated rather than the two obvious ones being spot-checked.
#[test]
fn every_remote_that_could_reach_a_network_forks_the_binary() {
    let repo = TempRepo::new("route-shapes");
    repo.write("f.txt", b"a\n");
    repo.commit_all("base");

    // Each of these has a transport behind it that a proxy could sit in front of, or that
    // needs an interactive credential libgit2 cannot supply.
    let networked = [
        ("https", "https://example.invalid/x.git"),
        ("http", "http://example.invalid/x.git"),
        ("ssh-url", "ssh://git@example.invalid/x.git"),
        ("scp-like", "git@example.invalid:x.git"),
        ("git-proto", "git://example.invalid/x.git"),
        // A host that merely looks local is still not a path.
        ("host-colon", "example.invalid:x.git"),
    ];
    for (name, url) in networked {
        repo.git(&["remote", "add", name, url]);
    }
    // And the shapes that genuinely are a directory on this machine, which is the whole set
    // libgit2 is allowed to keep.
    for (name, url) in [
        ("local-abs", "/srv/git/x.git"),
        ("local-file", "file:///srv/git/x.git"),
        ("local-rel", "../x.git"),
    ] {
        repo.git(&["remote", "add", name, url]);
    }

    let git_repo = git2::Repository::open(&repo.root).unwrap();
    for (name, url) in networked {
        assert_eq!(
            push::route(&git_repo, name),
            push::Route::Binary,
            "{url} would have gone through libgit2, which reads no proxy environment — so \
             `ProxyScope::git` would silently not apply to it"
        );
    }
    for name in ["local-abs", "local-file", "local-rel"] {
        assert_eq!(
            push::route(&git_repo, name),
            push::Route::Libgit2,
            "{name} is a path on this machine; forking a process for it buys nothing"
        );
    }

    // The fallthrough, which is what makes the list above safe to be incomplete: a URL shape
    // nobody anticipated is *not* handed to libgit2.
    repo.git(&["remote", "add", "novel", "quic+git://example.invalid/x.git"]);
    let git_repo2 = git2::Repository::open(&repo.root).unwrap();
    assert_eq!(
        push::route(&git_repo2, "novel"),
        push::Route::Binary,
        "an unrecognised scheme has to fall through to the binary, or the next transport git \
         gains is one cide cannot scope"
    );
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

// --- the frontend's indices are the indices `choose` resolves --------------------------------

/// The one bridge between what the diff pane paints and what staging acts on.
///
/// The pane renders `FileDiff` rows and sends `LineRef`s that index *those* rows; `choose`
/// resolves them against a `RawFile` it re-derives itself, and `diff::view` is the only thing
/// that makes the two the same indices. Nothing pinned that. A change on either side — folding
/// the `\ No newline` marker back out into a row of its own is the one the type's own doc
/// comment warns about — would stage a line the user never ticked, with every existing test
/// still green because every existing test picks its positions out of the `RawFile`.
///
/// So this one picks them the way the pane does: out of the rendered view, through the exact
/// request `cmd/git.rs::git_diff_file` makes, carrying the `rev` the pane would send back.
#[test]
fn a_line_ref_taken_from_the_rendered_view_stages_the_row_the_user_ticked() {
    let repo = TempRepo::new("view-indices");
    repo.write("f.txt", BASE_15);
    repo.commit_all("base");
    repo.write("f.txt", EDITED_15);

    // Exactly what the pane fetches: `git_diff_file` asks for renames, staging does not.
    let git_repo = git2::Repository::open(&repo.root).expect("open");
    let raw_file = diff::file_diff(
        &git_repo,
        "f.txt",
        DiffRequest::new(DiffSide::Unstaged).renames(true),
    )
    .expect("diff")
    .expect("f.txt is modified");
    let view = diff::view(&raw_file, DiffSide::Unstaged);

    // The view is the raw file, row for row. Stated as an assertion and not as a comment
    // because it is the whole reason a `LineRef` means the same thing at both ends.
    assert_eq!(view.hunks.len(), raw_file.hunks.len());
    for (shown, raw_hunk) in view.hunks.iter().zip(&raw_file.hunks) {
        assert_eq!(shown.lines.len(), raw_hunk.lines.len(), "row counts differ");
        for (row, raw_line) in shown.lines.iter().zip(&raw_hunk.lines) {
            let expected = match raw_line.origin {
                b'+' => cide_ipc::git::LineOrigin::Addition,
                b'-' => cide_ipc::git::LineOrigin::Deletion,
                _ => cide_ipc::git::LineOrigin::Context,
            };
            assert_eq!(row.origin, expected, "row origins differ");
        }
    }

    // Two hunks, and the second one's change rows sit *after* three context rows — so a
    // position counted in change lines rather than in view rows would name a different row
    // and this test would catch it.
    assert_eq!(view.hunks.len(), 2, "{:?}", view.hunks);
    let second = &view.hunks[1];
    let picked: Vec<LineRef> = second
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.origin != cide_ipc::git::LineOrigin::Context)
        .map(|(line, _)| LineRef {
            hunk: 1,
            line: line as u32,
        })
        .collect();
    assert_eq!(picked.len(), 2, "one deletion and one addition");
    assert!(
        picked.iter().all(|r| r.line >= 3),
        "the change rows must not be at the front, or the test proves nothing: {picked:?}"
    );

    // And the rev the pane holds is the rev staging re-derives, `renames(true)` or not. If it
    // were not, every partial stage from the pane would come back `StaleSelection`.
    stage::stage(
        &repo.root,
        &[PathSelection {
            path: "f.txt".into(),
            selection: Selection::Lines { lines: picked },
            rev: Some(view.rev.clone()),
        }],
    )
    .expect("stage the second hunk by view indices");

    assert_eq!(
        repo.git(&["show", ":f.txt"]).into_bytes(),
        FIRST_HUNK_UNSTAGED.to_vec(),
        "the row the user ticked is the row that was staged"
    );
}

/// A shelve whose selection has gone stale must leave nothing behind.
///
/// `shelve` writes the patch file and the catalogue entry and *then* calls `rollback`, which
/// is the only thing that used to check `rev`. So a held per-line selection whose file moved
/// underneath produced: a patch synthesized from the *new* diff at the *old* positions, on
/// disk; a shelf entry naming it; and only then a `StaleSelection` error. The user saw a
/// failure and got a shelf entry full of lines they never picked — which unshelving would
/// later apply on top of the change that is still in the working tree.
///
/// Reachable since the diff pane started handing stored selections to `shelve`; before that
/// every selection the panel sent carried `rev: None`.
#[test]
fn a_stale_selection_is_refused_before_the_shelf_is_written() {
    let repo = TempRepo::new("shelve-stale");
    repo.write("f.txt", BASE_15);
    repo.commit_all("base");
    repo.write("f.txt", EDITED_15);

    let file = raw(&repo, "f.txt", DiffSide::Combined);
    let stale = file.rev();
    let picked: Vec<LineRef> = file.hunks[1]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change())
        .map(|(line, _)| LineRef {
            hunk: 1,
            line: line as u32,
        })
        .collect();
    assert_eq!(picked.len(), 2);

    // The file moves under the held selection.
    let moved = b"A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nO\nEXTRA\n";
    repo.write("f.txt", moved);

    let outcome = shelf::shelve(
        &repo.root,
        "held",
        &[PathSelection {
            path: "f.txt".into(),
            selection: Selection::Lines { lines: picked },
            rev: Some(stale),
        }],
    );
    assert!(
        matches!(outcome, Err(GitError::StaleSelection { .. })),
        "expected a refusal, got {outcome:?}"
    );
    assert!(
        shelf::list(&repo.root).is_empty(),
        "a refused shelve must not leave an entry behind"
    );
    assert_eq!(
        repo.read("f.txt"),
        moved.to_vec(),
        "and must not touch the tree"
    );
}

/// `Selection::Whole` carrying a rev is checked, not waved through.
///
/// The diff pane needs this. Ticking *every* row of a diff encodes as `Whole` — the index API
/// is exact where a synthesized patch is not — but the user still pointed at rows they could
/// see, so the pane sends the rev and expects a moved file to be refused. Whole-file staging
/// from the panel's checkbox keeps sending `rev: None` and is unaffected.
#[test]
fn a_whole_selection_with_a_rev_is_still_refused_when_the_file_moved() {
    let repo = TempRepo::new("whole-rev");
    repo.write("f.txt", BASE_15);
    repo.commit_all("base");
    repo.write("f.txt", EDITED_15);

    let stale = raw(&repo, "f.txt", DiffSide::Unstaged).rev();
    repo.write(
        "f.txt",
        b"A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\nO\nLATER\n",
    );

    let outcome = stage::stage(
        &repo.root,
        &[PathSelection {
            path: "f.txt".into(),
            selection: Selection::Whole,
            rev: Some(stale),
        }],
    );
    assert!(
        matches!(outcome, Err(GitError::StaleSelection { .. })),
        "expected a refusal, got {outcome:?}"
    );
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]).trim(),
        "",
        "nothing may reach the index"
    );

    // And the same selection without a rev is the panel's checkbox, which still works.
    stage::stage(
        &repo.root,
        &[PathSelection {
            path: "f.txt".into(),
            selection: Selection::Whole,
            rev: None,
        }],
    )
    .expect("a checkbox stage takes the working tree as it is");
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]).trim(),
        "f.txt"
    );
}
