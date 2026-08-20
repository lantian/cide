//! The guarded commit actions, against real repositories and the real `git` binary. (M18)
//!
//! # Why the oracle is the binary
//!
//! This is `tests/patch_props.rs`'s discipline applied to a second dangerous surface. A reset,
//! a revert and a cherry-pick are each a handful of libgit2 calls, and every one of them is
//! *silently* wrong in a way that only shows up as somebody's lost afternoon: a mixed reset
//! that leaves one stat-dirty entry, a cherry-pick whose author is the person who pressed the
//! button, a hard reset that removes an untracked file. Hand-written examples cover the cases
//! the author thought of, which are exactly the cases that already work. So the core of this
//! file is differential: build a random repository, copy it byte for byte, run cide's version
//! in one copy and `git`'s in the other, and require `rev-parse HEAD`, `ls-files --stage` and a
//! hash of the whole working tree to be identical.
//!
//! Cases come from a seeded [`Rng`] and the seed is printed on every failure, so a bad case is
//! reproduced by re-running rather than by re-rolling until it happens again. `CIDE_GIT_CASES`
//! overrides the count.
//!
//! # What the hand-written half is for
//!
//! Everything the generator cannot reach, and every claim the design makes that is a claim
//! about a *refusal*: that a conflicting cherry-pick changes nothing at all and leaves no
//! `CHERRY_PICK_HEAD`; that reverting a merge without a mainline names both parents rather than
//! guessing; that `-m 1` and `-m 2` really do give different answers, which is the whole reason
//! for refusing to guess; that a tag can be created during a rebase while a reset cannot. Those
//! are the parts a differential test cannot express, because `git` and cide deliberately answer
//! them differently — cide refuses where git half-applies.

mod support;

use std::collections::BTreeSet;
use std::path::Path;

use cide_git::{branch, changelist, commit, replay, reset, shelf, sidecar, tag};
use cide_ipc::git::{CheckoutMode, CommitRequest, GitError};
use cide_ipc::history::{ReplayMode, ReplayOp, ReplayRequest, ResetKind, ResetRequest, TagRequest};
use support::{Eol, Rng, TempRepo, mutate, text};

// --- helpers ----------------------------------------------------------------------------------

/// A byte-for-byte copy of a repository, working tree, index and all.
///
/// `cp -a` rather than a `git clone`: a clone has neither the uncommitted changes nor the index
/// that half of these cases are *about*, and a differential test whose two sides start from
/// different states proves nothing. [`TempRepo::new`] is still used for the scratch path and
/// the `Drop` that leaves it behind on failure, and the repository it makes is thrown away.
fn twin(source: &TempRepo, tag: &str) -> TempRepo {
    let clone = TempRepo::new(tag);
    std::fs::remove_dir_all(&clone.root).expect("clearing the scratch repo");
    let status = std::process::Command::new("cp")
        .arg("-a")
        .arg(&source.root)
        .arg(&clone.root)
        .status()
        .expect("cp -a");
    assert!(status.success(), "copying {}", source.root.display());
    clone
}

/// Everything about the working tree that a reset or a replay can change, as one string.
///
/// Path, executable bit and content for every file outside `.git`, sorted. Directories are not
/// hashed: an empty directory left behind by one implementation and removed by the other is a
/// difference nobody can observe through git, which tracks files.
fn worktree_hash(root: &Path) -> String {
    let mut entries: Vec<(String, bool, Vec<u8>)> = Vec::new();
    walk(root, root, &mut entries);
    entries.sort();
    let mut hasher = blake3::Hasher::new();
    for (path, executable, bytes) in entries {
        hasher.update(path.as_bytes());
        hasher.update(&[0, u8::from(executable)]);
        hasher.update(&bytes);
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().to_string()
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, bool, Vec<u8>)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            walk(root, &path, out);
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("inside the root")
            .to_string_lossy()
            .into_owned();
        let bytes = if meta.file_type().is_symlink() {
            std::fs::read_link(&path)
                .map(|target| target.to_string_lossy().into_owned().into_bytes())
                .unwrap_or_default()
        } else {
            std::fs::read(&path).unwrap_or_default()
        };
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        out.push((relative, executable, bytes));
    }
}

fn head_oid(repo: &TempRepo) -> String {
    repo.git(&["rev-parse", "HEAD"]).trim().to_string()
}

fn rev(repo: &TempRepo, spec: &str) -> String {
    repo.git(&["rev-parse", spec]).trim().to_string()
}

fn porcelain(repo: &TempRepo) -> String {
    repo.git(&["status", "--porcelain"])
}

/// The claim the whole "composed by hand" design rests on: nothing is left half-finished.
///
/// Both halves are checked. [`git2::Repository::state`] is what
/// `cide_git::repo::operation_in_progress` reads, so it is what would start refusing every
/// other action; the file list is what a user (or a `git` in a bash pane) would trip over.
fn assert_no_sequencer_state(repo: &TempRepo, after: &str) {
    let handle = git2::Repository::open(&repo.root).expect("open");
    assert_eq!(
        handle.state(),
        git2::RepositoryState::Clean,
        "repository state after {after}"
    );
    for name in [
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "MERGE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        assert!(
            !repo.root.join(".git").join(name).exists(),
            ".git/{name} was left behind after {after}"
        );
    }
}

fn record_fingerprint(repo: &TempRepo) {
    let handle = git2::Repository::open(&repo.root).expect("open");
    changelist::record_index(&repo.root, &handle).expect("recording the index fingerprint");
}

fn stored_fingerprint(repo: &TempRepo) -> Option<String> {
    changelist::load(&repo.root).index_fingerprint
}

fn live_fingerprint(repo: &TempRepo) -> String {
    let handle = git2::Repository::open(&repo.root).expect("open");
    let mut index = handle.index().expect("index");
    index.read(true).expect("re-read the index");
    changelist::fingerprint(&index)
}

fn replay_request(commit: &str, mode: ReplayMode, mainline: Option<u32>) -> ReplayRequest {
    ReplayRequest {
        commit: commit.to_string(),
        mode,
        mainline,
    }
}

fn reset_request(target: &str, kind: ResetKind) -> ResetRequest {
    ResetRequest {
        target: target.to_string(),
        kind,
        shelve_first: None,
        force: false,
    }
}

// --- generation -------------------------------------------------------------------------------

/// Rewrite `path` so that it really is different.
///
/// [`mutate`] can insert a line and delete the same line, or rewrite a line to what it already
/// said, and the result is a file identical to the one it started from. `git commit` then fails
/// with *nothing to commit* and the whole case dies for a reason that has nothing to do with
/// what is being tested — which is exactly what happened at seed 349. `salt` makes the fallback
/// line unique so two forced edits cannot collide either.
fn edit(repo: &TempRepo, rng: &mut Rng, path: &str, salt: usize) {
    let old = repo.read(path);
    let mut fresh = mutate(rng, &old, Eol::Lf, true);
    for _ in 0..8 {
        if fresh != old {
            break;
        }
        fresh = mutate(rng, &old, Eol::Lf, true);
    }
    if fresh == old {
        fresh.extend_from_slice(format!("{salt:03} forced\n").as_bytes());
    }
    repo.write(path, &fresh);
}

/// A linear history of `commits` commits over `files` text files.
fn history(tag: &str, rng: &mut Rng, commits: usize, files: usize) -> TempRepo {
    let repo = TempRepo::new(tag);
    // `git commit` starts a **detached** `git gc --auto`, which packs loose objects and removes
    // their fanout directories. That is invisible in a normal test and fatal to `twin`: the
    // `cp -a` lists `.git/objects/` and then finds `d8/` gone underneath it, which showed up as
    // a copy failing one run in a few hundred. Turning both off is the fix, and it is only
    // needed for the repositories that get copied.
    repo.git(&["config", "gc.auto", "0"]);
    repo.git(&["config", "maintenance.auto", "false"]);
    for index in 0..files {
        // Long enough that two edits to one file usually land in different hunks. A six-line
        // file makes every second replay a conflict for reasons that have nothing to do with
        // the code under test.
        let lines = 24 + rng.below(24);
        repo.write(&format!("f{index}.txt"), &text(rng, lines, Eol::Lf, true));
    }
    repo.commit_all("commit 0");
    for n in 1..commits {
        // One file per commit, round-robin rather than at random. Two commits that rewrite the
        // same lines make *every* replay across them a conflict, and a generator that produced
        // nothing but conflicts would leave the differential half — the part that compares
        // results rather than refusals — never actually running. Overlap still happens
        // whenever `files` is small, which is why the caller varies it.
        edit(&repo, rng, &format!("f{}.txt", n % files), n);
        // A file that appears partway through, so a reset back past it has something to remove
        // and a replay has an add to carry.
        if rng.chance(1, 4) {
            repo.write(&format!("added-{n}.txt"), &text(rng, 4, Eol::Lf, true));
        }
        repo.commit_all(&format!("commit {n}"));
    }
    repo
}

/// Unstaged edits, staged edits and untracked files, in a random mixture.
fn dirty(repo: &TempRepo, rng: &mut Rng, files: usize) {
    for index in 0..files {
        let path = format!("f{index}.txt");
        match rng.below(4) {
            0 => {
                let old = repo.read(&path);
                repo.write(&path, &mutate(rng, &old, Eol::Lf, true));
            }
            1 => {
                let old = repo.read(&path);
                repo.write(&path, &mutate(rng, &old, Eol::Lf, true));
                repo.git(&["add", &path]);
            }
            2 => repo.write(&format!("untracked-{index}.txt"), b"scratch\n"),
            _ => {}
        }
    }
}

fn case_count() -> u64 {
    std::env::var("CIDE_GIT_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30)
}

// --- the differential core ----------------------------------------------------------------------

#[test]
fn reset_matches_the_git_binary() {
    for seed in 0..case_count() {
        let mut rng = Rng::new(seed ^ 0x5e5e_7000);
        let files = 2 + rng.below(3);
        let commits = 3 + rng.below(4);
        let ours = history(&format!("reset-diff-{seed}"), &mut rng, commits, files);
        dirty(&ours, &mut rng, files);

        let depth = rng.below(commits);
        let target = rev(&ours, &format!("HEAD~{depth}"));
        let (kind, flag) = match rng.below(3) {
            0 => (ResetKind::Soft, "--soft"),
            1 => (ResetKind::Mixed, "--mixed"),
            _ => (ResetKind::Hard, "--hard"),
        };

        let theirs = twin(&ours, &format!("reset-diff-{seed}-git"));
        let outcome = reset::reset(&ours.root, &reset_request(&target, kind))
            .unwrap_or_else(|error| panic!("seed {seed} [{kind:?} ~{depth}]: {error:?}"));
        theirs.git(&["reset", flag, &target]);

        let what = format!("seed {seed} [{kind:?} HEAD~{depth}]");
        assert_eq!(head_oid(&ours), head_oid(&theirs), "{what}: HEAD");
        assert_eq!(
            ours.index_state(),
            theirs.index_state(),
            "{what}: the index"
        );
        assert_eq!(
            worktree_hash(&ours.root),
            worktree_hash(&theirs.root),
            "{what}: the working tree"
        );
        assert_eq!(
            outcome.commits_dropped, depth as u32,
            "{what}: commits_dropped"
        );
        assert_eq!(outcome.head_after, target, "{what}: head_after");
        assert_no_sequencer_state(&ours, &what);
    }
}

#[test]
fn revert_matches_the_git_binary() {
    let mut compared = 0u32;
    for seed in 0..case_count() {
        let mut rng = Rng::new(seed ^ 0x4e_1e47);
        let files = 2 + rng.below(3);
        let ours = history(&format!("revert-diff-{seed}"), &mut rng, 4, files);
        let depth = rng.below(3);
        let victim = rev(&ours, &format!("HEAD~{depth}"));
        let theirs = twin(&ours, &format!("revert-diff-{seed}-git"));

        let what = format!("seed {seed} [revert HEAD~{depth}]");
        let outcome = replay::revert(
            &ours.root,
            &replay_request(&victim, ReplayMode::Commit, None),
        );
        let (their_ok, their_output) = theirs.try_git(&["revert", "--no-edit", &victim]);

        match outcome {
            Ok(outcome) => {
                assert!(
                    their_ok,
                    "{what}: cide reverted and git refused:\n{their_output}"
                );
                assert_same_commit(&ours, &theirs, false, &what);
                assert_eq!(outcome.op, ReplayOp::Revert);
                assert_eq!(outcome.source, victim, "{what}: source");
                assert_eq!(outcome.created, head_oid(&ours), "{what}: created");
                assert_no_sequencer_state(&ours, &what);
                compared += 1;
            }
            Err(GitError::ReplayWouldConflict { paths, .. }) => {
                assert!(
                    !their_ok,
                    "{what}: cide saw a conflict in {paths:?} and git did not"
                );
                assert_refused_cleanly(&ours, &what);
            }
            Err(GitError::EmptyReplay { .. }) => {
                // git calls this one out too, and refuses to make the empty commit.
                assert!(
                    !their_ok,
                    "{what}: git made a commit out of an empty revert"
                );
                assert_refused_cleanly(&ours, &what);
            }
            Err(other) => panic!("{what}: {other:?}"),
        }
    }
    assert!(
        compared >= 5,
        "the generator produced almost nothing but refusals ({compared} real comparisons); \
         the differential half is not being exercised"
    );
}

#[test]
fn cherry_pick_matches_the_git_binary() {
    let mut compared = 0u32;
    for seed in 0..case_count() {
        let mut rng = Rng::new(seed ^ 0xc4e_7900);
        let files = 2 + rng.below(3);
        let ours = history(&format!("pick-diff-{seed}"), &mut rng, 4, files);
        let picked = head_oid(&ours);

        // A sibling branch, so the pick has somewhere to land that does not already contain it.
        // Cherry-picking an ancestor of `HEAD` onto `HEAD` is the empty case, which is tested
        // on its own; here the point is a real three-way merge.
        let base = rev(&ours, "HEAD~2");
        ours.git(&["checkout", "-q", "-b", "side", &base]);
        let path = format!("f{}.txt", rng.below(files));
        edit(&ours, &mut rng, &path, 900);
        ours.commit_all("side work");

        let theirs = twin(&ours, &format!("pick-diff-{seed}-git"));
        let what = format!("seed {seed} [cherry-pick]");
        let outcome = replay::cherry_pick(
            &ours.root,
            &replay_request(&picked, ReplayMode::Commit, None),
        );
        let (their_ok, their_output) = theirs.try_git(&["cherry-pick", &picked]);

        match outcome {
            Ok(outcome) => {
                assert!(
                    their_ok,
                    "{what}: cide picked and git refused:\n{their_output}"
                );
                assert_same_commit(&ours, &theirs, true, &what);
                assert_eq!(outcome.op, ReplayOp::CherryPick);
                assert_no_sequencer_state(&ours, &what);
                compared += 1;
            }
            Err(GitError::ReplayWouldConflict { paths, .. }) => {
                assert!(
                    !their_ok,
                    "{what}: cide saw a conflict in {paths:?} and git did not"
                );
                assert_refused_cleanly(&ours, &what);
            }
            Err(GitError::EmptyReplay { .. }) => {
                assert!(!their_ok, "{what}: git committed an empty cherry-pick");
                assert_refused_cleanly(&ours, &what);
            }
            Err(other) => panic!("{what}: {other:?}"),
        }
    }
    assert!(
        compared >= 5,
        "the generator produced almost nothing but refusals ({compared} real comparisons)"
    );
}

/// The two repositories now hold the same commit in every way that is not a clock.
///
/// The oid itself cannot be compared: the committer timestamp is a second apart and that is
/// enough to change it. Everything the oid is made of *except* that is compared instead — the
/// tree, the message byte for byte, and the author triple, which is where "a cherry-pick keeps
/// the original author" and "the revert body is git's own" actually live.
fn assert_same_commit(ours: &TempRepo, theirs: &TempRepo, author_date: bool, what: &str) {
    assert_eq!(
        rev(ours, "HEAD^{tree}"),
        rev(theirs, "HEAD^{tree}"),
        "{what}: the committed tree"
    );
    assert_eq!(
        ours.index_state(),
        theirs.index_state(),
        "{what}: the index"
    );
    assert_eq!(
        worktree_hash(&ours.root),
        worktree_hash(&theirs.root),
        "{what}: the working tree"
    );
    assert_eq!(
        ours.git(&["log", "-1", "--format=%B"]),
        theirs.git(&["log", "-1", "--format=%B"]),
        "{what}: the commit message, byte for byte"
    );
    assert_eq!(
        ours.git(&["log", "-1", "--format=%an <%ae>"]),
        theirs.git(&["log", "-1", "--format=%an <%ae>"]),
        "{what}: the author"
    );
    if author_date {
        // Only for a cherry-pick, where the author date is the *original* one and carrying it
        // is the whole claim. A revert's author is the person reverting and its date is "now",
        // which is a clock reading and differs between the two runs whenever they straddle a
        // second — a real failure once every few hundred cases, and never a real bug.
        assert_eq!(
            ours.git(&["log", "-1", "--format=%at"]),
            theirs.git(&["log", "-1", "--format=%at"]),
            "{what}: the original author date, carried across the pick"
        );
    }
    assert_eq!(
        ours.git(&["log", "-1", "--format=%cn <%ce>"]),
        theirs.git(&["log", "-1", "--format=%cn <%ce>"]),
        "{what}: the committer"
    );
}

/// A refusal changed nothing: no commit, no sequencer file, no state.
fn assert_refused_cleanly(repo: &TempRepo, what: &str) {
    assert_no_sequencer_state(repo, what);
}

// --- reset: the preview, and what it promises -----------------------------------------------------

#[test]
fn a_hard_reset_names_every_file_before_it_discards_one() {
    let repo = TempRepo::new("reset-preview");
    repo.write("a.txt", b"a1\n");
    repo.write("b.txt", b"b1\n");
    repo.write("c.txt", b"c1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");

    // One unstaged, one staged, one merely present and untracked.
    repo.write("a.txt", b"a3\n");
    repo.write("b.txt", b"b2\n");
    repo.git(&["add", "b.txt"]);
    repo.write("scratch.txt", b"scratch\n");

    let preview = reset::preview(&repo.root, &base).expect("preview");
    assert_eq!(preview.head, "main");
    assert!(!preview.detached);
    assert_eq!(preview.head_oid, head_oid(&repo));
    assert_eq!(preview.target_oid, base);
    assert_eq!(preview.target_summary, "first");
    assert_eq!(preview.commits_dropped, 1);
    assert_eq!(preview.commits_gained, 0, "a reset back gains nothing");
    assert_eq!(preview.more_dropped, 0);
    assert_eq!(
        preview
            .dropped
            .iter()
            .map(|c| c.summary.as_str())
            .collect::<Vec<_>>(),
        vec!["second"],
    );

    // The rule the dialog's red button depends on: *name* them, do not count them.
    assert_eq!(preview.staged, vec!["b.txt".to_string()]);
    assert_eq!(
        preview.dirty,
        vec!["a.txt".to_string(), "b.txt".to_string()],
        "a staged file has working-tree content HEAD does not, so a --hard takes it too"
    );
    assert_eq!(
        preview
            .shelvable
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "b.txt"],
        "the shelf captures exactly the tracked half of `dirty`"
    );
    assert_eq!(
        preview.untracked_kept, 1,
        "counted, not listed: `git reset --hard` leaves untracked files alone"
    );
    assert!(!preview.use_staging_area);

    // And the reset really does discard what the preview named.
    let outcome =
        reset::reset(&repo.root, &reset_request(&base, ResetKind::Hard)).expect("hard reset");
    assert_eq!(outcome.files_discarded, 2);
    assert_eq!(repo.read("a.txt"), b"a1\n");
    assert_eq!(repo.read("b.txt"), b"b1\n");
    assert_eq!(
        repo.read("scratch.txt"),
        b"scratch\n",
        "the untracked file the preview promised to leave alone is still here"
    );
    assert_no_sequencer_state(&repo, "a hard reset");
}

#[test]
fn a_forward_reset_reports_what_it_gains() {
    let repo = TempRepo::new("reset-forward");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    let tip = head_oid(&repo);
    repo.git(&["reset", "--hard", "-q", &base]);

    // A dialog that could only say "0 commits would be undone" would be describing the wrong
    // half of a reset that is actually moving forward.
    let preview = reset::preview(&repo.root, &tip).expect("preview");
    assert_eq!(preview.commits_dropped, 0);
    assert_eq!(preview.commits_gained, 1);
}

#[test]
fn shelve_first_puts_the_working_tree_on_the_shelf() {
    let repo = TempRepo::new("reset-shelve");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    let tip = head_oid(&repo);
    repo.write("a.txt", b"work in progress\n");
    repo.write("untracked.txt", b"scratch\n");

    let outcome = reset::reset(
        &repo.root,
        &ResetRequest {
            target: base.clone(),
            kind: ResetKind::Hard,
            shelve_first: Some("before the reset".into()),
            force: false,
        },
    )
    .expect("reset with a shelve");

    let entry = outcome
        .shelved
        .expect("the whole entry, so a toast can offer Unshelve");
    assert_eq!(entry.name, "before the reset");
    assert_eq!(entry.files, vec!["a.txt".to_string()]);
    assert_eq!(shelf::list(&repo.root).len(), 1);
    assert_eq!(head_oid(&repo), base);
    assert_eq!(repo.read("a.txt"), b"a1\n");
    assert_eq!(
        repo.read("untracked.txt"),
        b"scratch\n",
        "shelving rolls a path back to HEAD, which for an untracked file would mean deleting \
         it — so untracked files are not shelvable and are left where the reset would have \
         left them anyway"
    );
    // Nothing was destroyed, so nothing is reported as destroyed.
    assert_eq!(
        outcome.files_discarded, 0,
        "the shelf took them; calling that `discarded` would tell the user their work is gone"
    );

    // And it really does come back. Back onto the commit it was taken from first: a shelf
    // entry is a *patch*, so unshelving it onto a tree the reset moved somewhere else is a
    // three-way apply that can legitimately fail — which is a fact about the shelf and not
    // about the reset, and asserting it here would be asserting the wrong thing.
    let patch = shelf::read_patch(&repo.root, &entry.id).expect("the patch text");
    assert!(patch.contains("work in progress"), "{patch}");
    repo.git(&["reset", "--hard", "-q", &tip]);
    shelf::unshelve(&repo.root, &entry.id, false).expect("unshelve");
    assert_eq!(repo.read("a.txt"), b"work in progress\n");
}

#[test]
fn a_failed_shelve_stops_the_reset() {
    let repo = TempRepo::new("reset-shelve-fails");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    repo.write("a.txt", b"work in progress\n");

    // A regular file where the shelf directory has to go. Chosen over a permission bit because
    // it fails the same way for root, and CI is not always somebody's laptop.
    let shelf_dir = sidecar::shelf_dir(&repo.root);
    std::fs::create_dir_all(shelf_dir.parent().expect("a parent")).expect("repo dir");
    std::fs::write(&shelf_dir, b"not a directory").expect("blocking file");

    let error = reset::reset(
        &repo.root,
        &ResetRequest {
            target: base,
            kind: ResetKind::Hard,
            shelve_first: Some("doomed".into()),
            force: false,
        },
    )
    .expect_err("a shelve that cannot be written must stop the reset");
    assert!(
        matches!(error, GitError::Sidecar { .. }),
        "unexpected error: {error:?}"
    );

    // The whole point: the copy the shelf failed to make is still the only copy, and it is
    // still where the user left it.
    assert_eq!(repo.read("a.txt"), b"work in progress\n");
    assert_eq!(repo.git(&["log", "--oneline"]).lines().count(), 2);
}

// --- reset: the guards ---------------------------------------------------------------------------

#[test]
fn a_mixed_reset_fires_the_external_staging_guard_and_a_soft_one_does_not() {
    let repo = TempRepo::new("reset-guard");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");

    // cide has written the index and remembers what it wrote.
    record_fingerprint(&repo);
    // …and then a `git add` in a bash pane, which is the case ADR 0004's first mitigation
    // exists for and is expected rather than hypothetical.
    repo.write("staged-by-hand.txt", b"hand\n");
    repo.git(&["add", "staged-by-hand.txt"]);

    let before = head_oid(&repo);
    let error = reset::reset(&repo.root, &reset_request(&base, ResetKind::Mixed))
        .expect_err("a mixed reset rewrites the index and must not clobber a hand-built one");
    assert!(
        matches!(error, GitError::IndexChangedExternally { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(head_oid(&repo), before, "the refusal moved nothing");

    // Soft does not fire: nothing staged is lost, the entries are merely staged against a
    // different HEAD. A guard that fires with nothing at risk is a guard the user turns off.
    reset::reset(&repo.root, &reset_request(&base, ResetKind::Soft))
        .expect("a soft reset never touches the index, so there is nothing to guard");
    assert_eq!(head_oid(&repo), base);
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"])
            .lines()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["a.txt", "staged-by-hand.txt"]),
        "the hand-built staging survived the soft reset intact"
    );
}

#[test]
fn force_is_the_second_click_past_the_staging_guard() {
    let repo = TempRepo::new("reset-guard-force");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    record_fingerprint(&repo);
    repo.write("hand.txt", b"hand\n");
    repo.git(&["add", "hand.txt"]);

    reset::reset(
        &repo.root,
        &ResetRequest {
            target: base.clone(),
            kind: ResetKind::Mixed,
            shelve_first: None,
            force: true,
        },
    )
    .expect("force proceeds past the guard, having listed what is at stake");
    assert_eq!(head_oid(&repo), base);
}

#[test]
fn a_reset_re_records_the_fingerprint_so_the_next_commit_is_not_refused() {
    let repo = TempRepo::new("reset-fingerprint");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    record_fingerprint(&repo);

    reset::reset(&repo.root, &reset_request(&base, ResetKind::Mixed)).expect("mixed reset");
    assert_eq!(
        stored_fingerprint(&repo).as_deref(),
        Some(live_fingerprint(&repo).as_str()),
        "the reset wrote .git/index, so the fingerprint has to describe the index it wrote"
    );

    // The proof that matters: the very next commit is not refused with a bar pointing at
    // cide's own act.
    let outcome = commit::commit(
        &repo.root,
        &CommitRequest {
            message: "after the reset".into(),
            amend: false,
            changelist: None,
            selections: None,
            force: false,
            amend_of: None,
        },
    )
    .expect("the commit after a reset must not hit the external-staging guard");
    assert_eq!(outcome.summary, "after the reset");
}

#[test]
fn a_failed_reset_re_records_the_fingerprint_too() {
    let repo = TempRepo::new("reset-fingerprint-error");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");

    // A fingerprint that is deliberately wrong, so "the code re-recorded it" and "the code did
    // nothing and it happened to match" are distinguishable — which they are not if the stored
    // value already describes the live index.
    changelist::update(&repo.root, |data| {
        data.index_fingerprint = Some("deliberately-stale".into());
        Ok(())
    })
    .expect("poisoning the fingerprint");

    // A stale `index.lock` is how a crashed `git` leaves a repository, and it makes
    // `git_index_write` fail *after* libgit2 has already moved the ref — the genuinely
    // half-done case the error path exists for.
    std::fs::write(repo.root.join(".git/index.lock"), b"").expect("lock");
    let error = reset::reset(
        &repo.root,
        &ResetRequest {
            target: base,
            kind: ResetKind::Mixed,
            shelve_first: None,
            // Past the guard, which the poisoned fingerprint would otherwise trip first.
            force: true,
        },
    )
    .expect_err("a locked index cannot be written");
    assert!(matches!(error, GitError::Git { .. }), "{error:?}");

    assert_eq!(
        stored_fingerprint(&repo).as_deref(),
        Some(live_fingerprint(&repo).as_str()),
        "a failed reset must still leave the fingerprint describing the live index, or the \
         next commit refuses and blames the user for cide's write"
    );
    let _ = std::fs::remove_file(repo.root.join(".git/index.lock"));
}

#[test]
fn changelist_assignments_survive_a_soft_reset_and_dead_ones_are_reconciled() {
    let repo = TempRepo::new("reset-changelists");
    repo.write("a.txt", b"a1\n");
    repo.write("ghost.txt", b"g1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");

    let fixes = changelist::update(&repo.root, |data| {
        let id = data.create("Fixes", "")?;
        data.move_paths(&id, &["a.txt".to_string(), "ghost.txt".to_string()])?;
        Ok(id)
    })
    .expect("a named list with two paths filed into it");

    reset::reset(&repo.root, &reset_request(&base, ResetKind::Soft)).expect("soft reset");

    let sidecar = changelist::load(&repo.root);
    assert_eq!(
        sidecar.owner_of("a.txt"),
        fixes,
        "the reset gave a.txt a change again, and an explicit assignment is not something a \
         reset may quietly discard"
    );
    assert_eq!(
        sidecar.owner_of("ghost.txt"),
        changelist::DEFAULT_ID,
        "ghost.txt has no change, so its assignment is dead and the reconcile trailer swept it"
    );
}

#[test]
fn a_reset_on_a_detached_head_stays_detached() {
    let repo = TempRepo::new("reset-detached");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let base = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    let tip = head_oid(&repo);

    branch::checkout_detached(&repo.root, &tip, CheckoutMode::Refuse).expect("detach");
    reset::reset(&repo.root, &reset_request(&base, ResetKind::Hard)).expect("hard reset");

    assert_eq!(head_oid(&repo), base);
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "HEAD",
        "a reset moves whatever HEAD names, and on a detached HEAD that is the commit — it \
         must not silently reattach to a branch"
    );
    assert_eq!(
        repo.git(&["rev-parse", "main"]).trim(),
        tip,
        "and main did not move"
    );
}

// --- replay: the refusals --------------------------------------------------------------------

/// `base` → `side` and `main` both rewrite the same line, so a pick across them must conflict.
fn conflicting_branches(tag: &str) -> (TempRepo, String) {
    let repo = TempRepo::new(tag);
    repo.write("f.txt", b"base\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("f.txt", b"side\n");
    repo.commit_all("side");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("f.txt", b"main\n");
    repo.commit_all("main");
    let main_tip = head_oid(&repo);
    repo.git(&["checkout", "-q", "side"]);
    (repo, main_tip)
}

#[test]
fn a_conflicting_cherry_pick_is_refused_and_changes_nothing() {
    let (repo, main_tip) = conflicting_branches("pick-conflict");
    repo.write("unrelated.txt", b"work in progress\n");
    let before_status = porcelain(&repo);
    let before_head = head_oid(&repo);
    let before_worktree = worktree_hash(&repo.root);
    let before_index = repo.index_state();

    let error = replay::cherry_pick(
        &repo.root,
        &replay_request(&main_tip, ReplayMode::Commit, None),
    )
    .expect_err("a cherry-pick that would conflict is refused, not forced");

    match error {
        GitError::ReplayWouldConflict { op, oid, paths } => {
            assert_eq!(op, ReplayOp::CherryPick);
            assert_eq!(oid, main_tip);
            // `paths` is the whole point of the variant: "cherry-pick failed" is unactionable.
            assert_eq!(paths, vec!["f.txt".to_string()]);
        }
        other => panic!("unexpected error: {other:?}"),
    }

    assert_eq!(
        porcelain(&repo),
        before_status,
        "`git status --porcelain` must be byte-identical across a refusal"
    );
    assert_eq!(head_oid(&repo), before_head);
    assert_eq!(worktree_hash(&repo.root), before_worktree);
    assert_eq!(repo.index_state(), before_index);
    // The claim the whole hand-composed design is for: git's own cherry-pick would have left
    // `CHERRY_PICK_HEAD` and a conflicted index here, and the panel has no *Continue* button.
    assert_no_sequencer_state(&repo, "a refused cherry-pick");
}

#[test]
fn a_conflicting_revert_is_refused_the_same_way() {
    let (repo, main_tip) = conflicting_branches("revert-conflict");
    let error = replay::revert(
        &repo.root,
        &replay_request(&main_tip, ReplayMode::Commit, None),
    )
    .expect_err("reverting a commit whose lines this branch has rewritten conflicts");
    assert!(
        matches!(
            error,
            GitError::ReplayWouldConflict {
                op: ReplayOp::Revert,
                ..
            }
        ),
        "{error:?}"
    );
    assert_no_sequencer_state(&repo, "a refused revert");
}

/// `main` and `side` each add a file, then merge. `M` has two parents with different content.
fn merged_history(tag: &str) -> (TempRepo, String) {
    let repo = TempRepo::new(tag);
    repo.write("shared.txt", b"base\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("side.txt", b"from side\n");
    repo.commit_all("side work");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("main.txt", b"from main\n");
    repo.commit_all("main work");
    repo.git(&[
        "merge",
        "-q",
        "--no-ff",
        "side",
        "-m",
        "Merge branch 'side'",
    ]);
    let merge = head_oid(&repo);
    (repo, merge)
}

#[test]
fn reverting_a_merge_without_a_mainline_names_both_parents() {
    let (repo, merge) = merged_history("merge-mainline");
    let error = replay::revert(
        &repo.root,
        &replay_request(&merge, ReplayMode::Commit, None),
    )
    .expect_err("\"revert this merge\" has no correct default");

    match error {
        GitError::MergeNeedsMainline { oid, parents } => {
            assert_eq!(oid, merge);
            // The parents themselves, not a count: *which side do you want to keep* is not a
            // question anybody can answer from the number 2.
            assert_eq!(parents.len(), 2);
            assert_eq!(parents[0].summary, "main work");
            assert_eq!(parents[1].summary, "side work");
            assert_eq!(parents[0].short_oid, rev(&repo, "HEAD^1")[..8].to_string());
            assert_eq!(parents[1].short_oid, rev(&repo, "HEAD^2")[..8].to_string());
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_no_sequencer_state(&repo, "a refused merge revert");
}

#[test]
fn mainline_one_and_mainline_two_give_different_answers() {
    // This is the entire justification for refusing to guess: the two answers are not close,
    // they are opposites — one undoes the feature branch, the other undoes the trunk.
    let (first, merge) = merged_history("merge-m1");
    replay::revert(
        &first.root,
        &replay_request(&merge, ReplayMode::Commit, Some(1)),
    )
    .expect("revert -m 1");

    let (second, merge_two) = merged_history("merge-m2");
    replay::revert(
        &second.root,
        &replay_request(&merge_two, ReplayMode::Commit, Some(2)),
    )
    .expect("revert -m 2");

    assert!(
        !first.root.join("side.txt").exists(),
        "-m 1 keeps the trunk and undoes what side brought"
    );
    assert!(first.root.join("main.txt").exists());
    assert!(
        !second.root.join("main.txt").exists(),
        "-m 2 keeps side and undoes what the trunk brought"
    );
    assert!(second.root.join("side.txt").exists());
    assert_ne!(rev(&first, "HEAD^{tree}"), rev(&second, "HEAD^{tree}"));

    // And the body is git's, including the merge clause `sequencer.c` adds.
    let body = first.git(&["log", "-1", "--format=%B"]);
    assert!(
        body.starts_with("Revert \"Merge branch 'side'\"\n\nThis reverts commit "),
        "{body}"
    );
    assert!(body.contains(", reversing\nchanges made to "), "{body}");
}

#[test]
fn a_mainline_on_a_non_merge_is_refused() {
    let (repo, _) = merged_history("merge-not");
    let ordinary = rev(&repo, "HEAD^1");
    let error = replay::revert(
        &repo.root,
        &replay_request(&ordinary, ReplayMode::Commit, Some(1)),
    )
    .expect_err(
        "git refuses this too, and silently ignoring the argument would make a \
                 mis-wired button look like it worked",
    );
    assert_eq!(error, GitError::NotAMerge { oid: ordinary });
}

#[test]
fn a_mainline_out_of_range_is_refused_rather_than_handed_to_libgit2() {
    let (repo, merge) = merged_history("merge-range");
    for bad in [0, 3, 99] {
        let error = replay::revert(
            &repo.root,
            &replay_request(&merge, ReplayMode::Commit, Some(bad)),
        )
        .expect_err("out of range");
        // Reported as the same question, because the dialog's answer is the same: here are the
        // parents, pick one.
        assert!(
            matches!(error, GitError::MergeNeedsMainline { ref parents, .. } if parents.len() == 2),
            "mainline {bad}: {error:?}"
        );
    }
}

#[test]
fn an_empty_replay_is_refused_rather_than_committed() {
    let repo = TempRepo::new("empty-replay");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    let second = head_oid(&repo);

    replay::revert(
        &repo.root,
        &replay_request(&second, ReplayMode::Commit, None),
    )
    .expect("the first revert has work to do");

    let error = replay::revert(
        &repo.root,
        &replay_request(&second, ReplayMode::Commit, None),
    )
    .expect_err("the second has none");
    assert_eq!(
        error,
        GitError::EmptyReplay {
            op: ReplayOp::Revert,
            oid: second
        },
        "an empty commit here is a commit the user has to explain in review, and it looks \
         exactly like a successful revert in the log"
    );
    assert_eq!(repo.git(&["log", "--oneline"]).lines().count(), 3);
}

#[test]
fn a_working_tree_replay_does_not_move_head() {
    let repo = TempRepo::new("replay-working-tree");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    let head_before = head_oid(&repo);
    let second = head_before.clone();

    let outcome = replay::revert(
        &repo.root,
        &replay_request(&second, ReplayMode::WorkingTree, None),
    )
    .expect("IDEA's *do not commit*");

    assert_eq!(head_oid(&repo), head_before, "HEAD did not move");
    assert_eq!(
        outcome.created, "",
        "empty rather than an Option, the same convention FetchOutcome's vacant fields use"
    );
    assert_eq!(repo.read("a.txt"), b"a1\n", "the change is in the tree");
    assert!(!porcelain(&repo).is_empty(), "and it is pending");
    // And no `REVERT_HEAD`, which git's own `-n` would have written — cide has no `git commit`
    // handoff to feed it to, and the file would lock every other action in the crate.
    assert_no_sequencer_state(&repo, "a working-tree revert");
}

// --- in-progress operations: the asymmetry --------------------------------------------------------

/// A rebase stopped on a conflict. The repository is left mid-operation, as a user's would be.
fn wedged_rebase(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("f.txt", b"base\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("f.txt", b"side\n");
    repo.commit_all("side");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("f.txt", b"main\n");
    repo.commit_all("main");
    repo.git(&["checkout", "-q", "side"]);
    let (ok, _) = repo.try_git(&["rebase", "main"]);
    assert!(!ok, "the rebase is supposed to stop on a conflict");
    repo
}

fn wedged_cherry_pick(tag: &str) -> TempRepo {
    let (repo, main_tip) = conflicting_branches(tag);
    let (ok, _) = repo.try_git(&["cherry-pick", &main_tip]);
    assert!(!ok, "the cherry-pick is supposed to stop on a conflict");
    repo
}

fn bisecting(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("f.txt", b"one\n");
    repo.commit_all("one");
    repo.write("f.txt", b"two\n");
    repo.commit_all("two");
    repo.git(&["bisect", "start"]);
    repo
}

#[test]
fn moving_head_is_refused_mid_operation_while_a_tag_is_not() {
    for (make, expected) in [
        (wedged_rebase as fn(&str) -> TempRepo, "rebase"),
        (wedged_cherry_pick as fn(&str) -> TempRepo, "cherry-pick"),
        (bisecting as fn(&str) -> TempRepo, "bisect"),
    ] {
        let repo = make(&format!("in-progress-{expected}"));
        let handle = git2::Repository::open(&repo.root).expect("open");
        assert_ne!(
            handle.state(),
            git2::RepositoryState::Clean,
            "the fixture for {expected} did not actually wedge the repository"
        );
        drop(handle);
        let head = head_oid(&repo);

        // Everything that moves HEAD, the index or the working tree is refused. Note there is
        // deliberately no `force` past this: a `--hard` during a rebase does not abort the
        // rebase, it leaves the todo pointing at unreachable commits.
        let refusals: Vec<GitError> = vec![
            reset::reset(&repo.root, &reset_request(&head, ResetKind::Hard)).unwrap_err(),
            reset::reset(
                &repo.root,
                &ResetRequest {
                    target: head.clone(),
                    kind: ResetKind::Hard,
                    shelve_first: None,
                    force: true,
                },
            )
            .unwrap_err(),
            replay::revert(&repo.root, &replay_request(&head, ReplayMode::Commit, None))
                .unwrap_err(),
            replay::cherry_pick(&repo.root, &replay_request(&head, ReplayMode::Commit, None))
                .unwrap_err(),
            branch::checkout_detached(&repo.root, &head, CheckoutMode::Refuse).unwrap_err(),
        ];
        for error in refusals {
            assert_eq!(
                error,
                GitError::OperationInProgress {
                    operation: expected.to_string()
                },
                "during a {expected}"
            );
        }

        // And the deliberate exception, pinned so a later tidy-up cannot make the guards
        // uniform: a tag writes one ref, and `git tag` works perfectly well mid-rebase.
        let outcome = tag::create(
            &repo.root,
            &TagRequest {
                name: "wip".into(),
                target: head.clone(),
                message: None,
                force: false,
            },
        )
        .unwrap_or_else(|error| panic!("a tag during a {expected} must be allowed: {error:?}"));
        assert_eq!(outcome.oid, head);
        assert_eq!(rev(&repo, "wip"), head);
    }
}

// --- unborn HEAD -------------------------------------------------------------------------------

#[test]
fn every_action_refuses_an_unborn_head() {
    let repo = TempRepo::new("unborn");
    let nothing = "0000000000000000000000000000000000000000";

    assert_eq!(reset::preview(&repo.root, nothing), Err(GitError::Unborn));
    assert_eq!(
        reset::reset(&repo.root, &reset_request(nothing, ResetKind::Soft)),
        Err(GitError::Unborn)
    );
    assert_eq!(
        replay::revert(
            &repo.root,
            &replay_request(nothing, ReplayMode::Commit, None)
        ),
        Err(GitError::Unborn)
    );
    assert_eq!(
        replay::cherry_pick(
            &repo.root,
            &replay_request(nothing, ReplayMode::Commit, None)
        ),
        Err(GitError::Unborn)
    );
    assert_eq!(
        branch::checkout_detached(&repo.root, nothing, CheckoutMode::Refuse),
        Err(GitError::Unborn)
    );
    // A tag is the one that refuses for a different reason, and the different reason is the
    // honest one: there is no `HEAD` to be unborn *about* here, there is simply no such commit
    // to point a ref at.
    assert_eq!(
        tag::create(
            &repo.root,
            &TagRequest {
                name: "v0".into(),
                target: nothing.into(),
                message: None,
                force: false,
            }
        ),
        Err(GitError::NoSuchCommit {
            rev: nothing.to_string()
        })
    );
}

// --- amend -----------------------------------------------------------------------------------

#[test]
fn amending_head_keeps_the_original_author() {
    let repo = TempRepo::new("amend-author");
    repo.write("a.txt", b"a1\n");
    repo.git(&["add", "."]);
    repo.git(&[
        "commit",
        "-q",
        "--author=Someone Else <else@cide.invalid>",
        "-m",
        "their commit",
    ]);
    repo.write("a.txt", b"a2\n");

    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "their commit, tidied".into(),
            amend: true,
            changelist: None,
            selections: None,
            force: false,
            amend_of: None,
        },
    )
    .expect("amend");

    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>"]).trim(),
        "Someone Else <else@cide.invalid>",
        "`git commit --amend` keeps the author, and so must this — otherwise amending \
         somebody else's commit silently claims it"
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%cn"]).trim(),
        "cide tests",
        "the committer is whoever pressed the button"
    );
    assert_eq!(repo.git(&["log", "--oneline"]).lines().count(), 1);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]).trim(),
        "their commit, tidied"
    );
}

/// A **reword** — an amend whose only change is the message — with nothing selected.
///
/// This is the commonest amend there is and it was refused until the `NothingToCommit` guard
/// learned to admit an amend: nothing is ticked, so `selections` is empty, and the request never
/// reached the amend path. The oracle is `git commit --amend -m`, which is what a user reaching
/// for a terminal instead would have run.
#[test]
fn a_reword_changes_the_message_and_leaves_the_tree_alone() {
    let repo = TempRepo::new("amend-reword");
    repo.write("a.txt", b"a1\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-q", "-m", "typo in teh summary"]);
    let tree_before = repo.git(&["rev-parse", "HEAD^{tree}"]).trim().to_string();

    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "typo in the summary".into(),
            amend: true,
            changelist: None,
            selections: None,
            force: false,
            amend_of: None,
        },
    )
    .expect("a reword is an amend with nothing selected, and must not be NothingToCommit");

    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]).trim(),
        "typo in the summary"
    );
    assert_eq!(
        repo.git(&["rev-parse", "HEAD^{tree}"]).trim(),
        tree_before,
        "a reword changes the message and nothing else — a tree that moved would mean the index \
         was rebuilt from something other than HEAD"
    );
    assert_eq!(
        repo.git(&["log", "--oneline"]).lines().count(),
        1,
        "amended, not added on top of"
    );
    assert_eq!(
        repo.git(&["status", "--porcelain"]).trim(),
        "",
        "and nothing is left dirty behind it"
    );
}

/// A reword must not touch work the user has not committed.
///
/// The reword path runs `rebuild_index` with no selections, which clears the index down to HEAD.
/// That is right for the index — in changelist mode it is derived — and would be a data loss if
/// it reached the worktree, which is the one thing here that is not.
#[test]
fn a_reword_leaves_unstaged_work_in_the_worktree() {
    let repo = TempRepo::new("amend-reword-dirty");
    repo.write("a.txt", b"a1\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-q", "-m", "first"]);
    repo.write("a.txt", b"a1\nwork in progress\n");
    repo.write("new.txt", b"untracked\n");

    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "first, reworded".into(),
            amend: true,
            changelist: None,
            // Explicitly nothing, which is what unticking every row sends. With `None` the
            // default would pick the dirty file up and this would be an ordinary amend.
            selections: Some(Vec::new()),
            force: false,
            amend_of: None,
        },
    )
    .expect("reword");

    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]).trim(),
        "first, reworded"
    );
    assert_eq!(
        std::fs::read(repo.root.join("a.txt")).unwrap(),
        b"a1\nwork in progress\n",
        "the uncommitted edit survives a reword"
    );
    assert!(
        repo.root.join("new.txt").exists(),
        "and so does an untracked file"
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "first, reworded",
        "the reworded commit holds only the message it was given"
    );
    assert!(
        !repo
            .git(&["show", "--stat", "--format=", "HEAD"])
            .contains("new.txt"),
        "the reword committed nothing new — the tree is still the one HEAD had"
    );
}

/// The guard the reword widened still does the job it was written for.
///
/// It is not there to refuse empty commits — the tree-identity check further down does that. It
/// is there so that an *ordinary* commit of an empty changelist is refused **before**
/// `rebuild_index` clears the index to HEAD, rather than after, having already thrown away
/// whatever the index held.
#[test]
fn an_ordinary_commit_with_nothing_selected_is_still_refused() {
    let repo = TempRepo::new("amend-guard-kept");
    repo.write("a.txt", b"a1\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-q", "-m", "first"]);
    repo.write("a.txt", b"a2\n");
    repo.git(&["add", "."]);
    let staged_before = repo.git(&["ls-files", "--stage"]);

    let error = commit::commit(
        &repo.root,
        &CommitRequest {
            message: "nothing ticked".into(),
            amend: false,
            changelist: None,
            selections: Some(Vec::new()),
            force: true,
            amend_of: None,
        },
    )
    .expect_err("an ordinary commit with nothing selected is still NothingToCommit");
    assert_eq!(error, GitError::NothingToCommit);
    assert_eq!(
        repo.git(&["ls-files", "--stage"]),
        staged_before,
        "and the index it would have cleared is untouched, which is the whole reason the guard \
         runs before `rebuild_index` rather than after it"
    );
}

/// The refusal half of amend: the log named a commit, and HEAD moved before the confirm landed.
///
/// This is a **race**, not a mis-click, and it is the reason the expected oid travels with the
/// request instead of being read from the row the menu was opened on. The log offers Amend only
/// on the HEAD row; between the menu opening and the user confirming, a `git commit` in a bash
/// pane — or an agent — can move HEAD. Without the guard the amend would land on a commit the
/// user never looked at, rewriting it and silently taking its author.
#[test]
fn amending_a_commit_that_is_no_longer_head_is_refused() {
    let repo = TempRepo::new("amend-not-head");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let drawn = head_oid(&repo);

    // What the bash pane did while the menu was open.
    repo.write("b.txt", b"b1\n");
    repo.commit_all("second");
    let moved = head_oid(&repo);
    assert_ne!(drawn, moved);

    let error = commit::commit(
        &repo.root,
        &CommitRequest {
            message: "tidied".into(),
            amend: true,
            changelist: None,
            selections: None,
            force: false,
            amend_of: Some(drawn.clone()),
        },
    )
    .expect_err("amending a commit that is no longer HEAD must be refused");

    match error {
        GitError::NotHead { oid, head } => {
            assert_eq!(oid, drawn, "the refusal names the commit the user aimed at");
            assert_eq!(head, moved, "and the one that is actually there now");
        }
        other => panic!("expected NotHead, got {other:?}"),
    }

    // Nothing was written: the second commit is still HEAD and still says what it said.
    assert_eq!(head_oid(&repo), moved);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]).trim(), "second");
    assert_eq!(repo.git(&["log", "--oneline"]).lines().count(), 2);
}

/// The same request against the commit that *is* HEAD goes through, so the guard is a check and
/// not a blanket refusal — the pair is what makes either half meaningful.
#[test]
fn amending_the_commit_that_is_still_head_is_allowed() {
    let repo = TempRepo::new("amend-is-head");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let head = head_oid(&repo);
    // An amend still has to have something to commit — `commit::commit` builds the tree from the
    // changelist, and an empty one is `NothingToCommit` whether or not `amend` is set.
    repo.write("a.txt", b"a2\n");

    commit::commit(
        &repo.root,
        &CommitRequest {
            message: "first, tidied".into(),
            amend: true,
            changelist: None,
            selections: None,
            force: false,
            amend_of: Some(head),
        },
    )
    .expect("amending HEAD with its own oid is the ordinary case");

    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]).trim(),
        "first, tidied"
    );
    assert_eq!(repo.git(&["log", "--oneline"]).lines().count(), 1);
}

// --- tags ------------------------------------------------------------------------------------

#[test]
fn lightweight_and_annotated_tags_are_what_git_says_they_are() {
    let repo = TempRepo::new("tag-kinds");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let head = head_oid(&repo);

    let light = tag::create(
        &repo.root,
        &TagRequest {
            name: "v1.0".into(),
            target: head.clone(),
            message: None,
            force: false,
        },
    )
    .expect("lightweight");
    assert!(!light.annotated);
    assert!(!light.moved);
    assert_eq!(light.oid, head);
    assert_eq!(
        repo.git(&["cat-file", "-t", "v1.0"]).trim(),
        "commit",
        "a lightweight tag is a ref and nothing else"
    );

    let annotated = tag::create(
        &repo.root,
        &TagRequest {
            name: "v1.1".into(),
            target: head.clone(),
            message: Some("the first release".into()),
            force: false,
        },
    )
    .expect("annotated");
    assert!(annotated.annotated);
    assert_eq!(
        repo.git(&["cat-file", "-t", "v1.1"]).trim(),
        "tag",
        "an annotated tag is a real object — the distinction `git describe` cares about"
    );
    assert_eq!(annotated.oid, head, "and the outcome names the commit");
    assert_eq!(
        repo.git(&["tag", "-l", "--format=%(contents:subject)", "v1.1"])
            .trim(),
        "the first release"
    );
}

#[test]
fn a_tag_collision_says_where_the_tag_points() {
    let repo = TempRepo::new("tag-collision");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let first = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    let second = head_oid(&repo);

    tag::create(
        &repo.root,
        &TagRequest {
            name: "v1.0".into(),
            target: first.clone(),
            message: None,
            force: false,
        },
    )
    .expect("the first tag");

    let error = tag::create(
        &repo.root,
        &TagRequest {
            name: "v1.0".into(),
            target: second.clone(),
            message: None,
            force: false,
        },
    )
    .expect_err("a tag is a published promise about which commit a release is");
    assert_eq!(
        error,
        GitError::TagExists {
            name: "v1.0".into(),
            // Where it points *now*, so the confirmation can say what would be lost rather
            // than only that the name is taken.
            oid: first.clone(),
        }
    );
    assert_eq!(rev(&repo, "v1.0^{commit}"), first, "nothing moved");

    let outcome = tag::create(
        &repo.root,
        &TagRequest {
            name: "v1.0".into(),
            target: second.clone(),
            message: None,
            force: true,
        },
    )
    .expect("force is the second, confirmed click");
    assert!(
        outcome.moved,
        "\"moved v1.0\" and \"created v1.0\" are things a user has to be able to tell apart \
         after the fact"
    );
    assert_eq!(rev(&repo, "v1.0^{commit}"), second);

    // The same applies to an annotated tag standing where a lightweight one is asked for.
    let error = tag::create(
        &repo.root,
        &TagRequest {
            name: "v1.0".into(),
            target: first,
            message: Some("annotated now".into()),
            force: false,
        },
    )
    .expect_err("still a collision");
    assert!(matches!(error, GitError::TagExists { .. }), "{error:?}");
}

#[test]
fn tag_names_git_would_reject_are_refused_before_libgit2_sees_them() {
    let repo = TempRepo::new("tag-names");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let head = head_oid(&repo);

    for bad in ["v 1.0", "a..b", "~x", "x.lock", "a@{b}", "f[o]o", "", "a^b"] {
        let error = tag::create(
            &repo.root,
            &TagRequest {
                name: bad.into(),
                target: head.clone(),
                message: None,
                force: false,
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            GitError::InvalidTagName { name: bad.into() },
            "{bad:?} is a name git would reject, and the dialog has to be able to say so \
             while the user is still typing rather than after a round trip"
        );
        assert!(
            repo.git(&["tag", "-l"]).trim().is_empty(),
            "{bad:?} left a ref behind"
        );
    }
}

// --- detached checkout ---------------------------------------------------------------------------

#[test]
fn a_detached_checkout_reports_the_way_back() {
    let repo = TempRepo::new("detach");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let first = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second commit");

    let outcome =
        branch::checkout_detached(&repo.root, &first, CheckoutMode::Refuse).expect("detach");
    assert_eq!(outcome.head, first[..8].to_string());
    assert_eq!(outcome.summary, "first");
    assert_eq!(
        outcome.previous, "main",
        "the single most important field: a detached HEAD is the state users most often reach \
         by accident and least often know how to leave"
    );
    assert!(outcome.stashed.is_none());
    assert_eq!(head_oid(&repo), first);
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "HEAD"
    );
    assert_eq!(repo.read("a.txt"), b"a1\n");

    // Detaching again from a detached HEAD names the commit you were on, because that is still
    // the way back.
    let outcome = branch::checkout_detached(&repo.root, "main", CheckoutMode::Refuse)
        .expect("detach onto main's tip");
    assert_eq!(outcome.previous, first[..8].to_string());
}

#[test]
fn a_detached_checkout_refuses_or_stashes_exactly_as_a_branch_switch_does() {
    let repo = TempRepo::new("detach-blockers");
    // Three lines, so the commits can disagree about the top while the working tree edits the
    // bottom: that is what makes the restore a *clean* three-way merge rather than the
    // conflicting one `branches.rs` already covers.
    repo.write("a.txt", b"top\nmiddle\nbottom\n");
    repo.write("untouched.txt", b"same on both\n");
    repo.commit_all("first");
    let first = head_oid(&repo);
    repo.write("a.txt", b"TOP\nmiddle\nbottom\n");
    repo.commit_all("second");

    repo.write("a.txt", b"TOP\nmiddle\nBOTTOM\n");
    repo.write("untouched.txt", b"edited but identical on both sides\n");

    let error = branch::checkout_detached(&repo.root, &first, CheckoutMode::Refuse)
        .expect_err("a.txt differs between here and there and is locally modified");
    match error {
        GitError::CheckoutWouldOverwrite { branch, paths } => {
            assert_eq!(branch, first[..8].to_string());
            assert_eq!(
                paths,
                vec!["a.txt".to_string()],
                "untouched.txt is byte-identical on both sides, so it comes along and is not a \
                 blocker — the same three-clause rule branch switching uses"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(head_oid(&repo), rev(&repo, "main"), "nothing moved");

    let outcome = branch::checkout_detached(&repo.root, &first, CheckoutMode::StashAndRestore)
        .expect("smart checkout");
    assert!(
        outcome.stashed.is_none() && outcome.restore_failed.is_none(),
        "applied and dropped, so there is no stash left to tell the user about: {outcome:?}"
    );
    assert_eq!(head_oid(&repo), first);
    assert_eq!(
        repo.read("a.txt"),
        b"top\nmiddle\nBOTTOM\n",
        "the commit's line came back and the working tree's line came along, which is the \
         whole of \"smart checkout\""
    );
    assert_eq!(
        repo.read("untouched.txt"),
        b"edited but identical on both sides\n"
    );
}

// --- the three actions that needed no new backend --------------------------------------------------

#[test]
fn a_branch_can_be_created_at_an_oid_with_no_new_backend() {
    let repo = TempRepo::new("branch-from-here");
    repo.write("a.txt", b"a1\n");
    repo.commit_all("first");
    let first = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");

    // `branch::create`'s `resolve()` falls through to `revparse_single`, so a full oid from a
    // log row is a legal start point today. This test exists to keep that true.
    branch::create(&repo.root, "from-here", Some(&first)).expect("branch from a commit");
    assert_eq!(rev(&repo, "from-here"), first);
    assert_eq!(
        head_oid(&repo),
        rev(&repo, "main"),
        "creating a branch does not switch to it"
    );
}

// --- the standing claim ---------------------------------------------------------------------------

#[test]
fn nothing_leaves_a_sequencer_state_behind() {
    let repo = TempRepo::new("no-sequencer");
    repo.write("a.txt", b"a1\n");
    repo.write("b.txt", b"b1\n");
    repo.commit_all("first");
    let first = head_oid(&repo);
    repo.write("a.txt", b"a2\n");
    repo.commit_all("second");
    let second = head_oid(&repo);
    repo.write("b.txt", b"b2\n");
    repo.commit_all("third");

    replay::revert(
        &repo.root,
        &replay_request(&second, ReplayMode::Commit, None),
    )
    .expect("revert");
    assert_no_sequencer_state(&repo, "a committed revert");

    // The root commit created both files and both have been rewritten since, so undoing it is
    // a delete/modify conflict on each — refused, and refused without a trace.
    replay::revert(
        &repo.root,
        &replay_request(&first, ReplayMode::WorkingTree, None),
    )
    .expect_err("reverting the root commit conflicts with everything written since");
    assert_no_sequencer_state(&repo, "a refused revert");

    repo.git(&["checkout", "-q", "-b", "side", &first]);
    replay::cherry_pick(
        &repo.root,
        &replay_request(&second, ReplayMode::Commit, None),
    )
    .expect("cherry-pick");
    assert_no_sequencer_state(&repo, "a committed cherry-pick");

    replay::cherry_pick(
        &repo.root,
        &replay_request(&second, ReplayMode::WorkingTree, None),
    )
    .expect_err("already applied");
    assert_no_sequencer_state(&repo, "an empty cherry-pick");

    reset::reset(&repo.root, &reset_request(&first, ResetKind::Hard)).expect("reset");
    assert_no_sequencer_state(&repo, "a hard reset");

    branch::checkout_detached(&repo.root, &second, CheckoutMode::Refuse).expect("detach");
    assert_no_sequencer_state(&repo, "a detached checkout");

    tag::create(
        &repo.root,
        &TagRequest {
            name: "v1".into(),
            target: second,
            message: Some("annotated".into()),
            force: false,
        },
    )
    .expect("tag");
    assert_no_sequencer_state(&repo, "a tag");
}
