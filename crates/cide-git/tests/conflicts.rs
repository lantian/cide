//! `cide_git::conflict`: landing a conflict, resolving it, and getting back out.
//!
//! The promise these tests exist to pin is the one the whole feature rests on: **a conflicted
//! pull leaves real git state**, so the same conflict is visible to `git status` in a terminal
//! pane, survives an app restart, and can be abandoned with `git merge --abort` by somebody who
//! would rather not use the resolver at all.

mod support;

use cide_git::{conflict, pull};
use cide_ipc::git::{ConflictSide, GitError, PullDefault, PullRequest, PullStrategy};
use support::{TempRepo, assert_no_sequencer_state, clone_of, worktree_hash};

fn untouched() -> cide_core::proxy::ProxyEnv {
    cide_core::proxy::ProxyEnv::default()
}

fn request(strategy: PullStrategy) -> PullRequest {
    PullRequest {
        strategy: Some(strategy),
        ..PullRequest::default()
    }
}

/// A clone whose branch and upstream have both edited `shared.txt` on the same line.
fn conflicting(tag: &str) -> (TempRepo, TempRepo) {
    let origin = TempRepo::new(&format!("{tag}-origin"));
    origin.write("shared.txt", b"base\n");
    origin.write("calm.txt", b"untouched\n");
    origin.commit_all("first");
    let work = clone_of(&origin, &format!("{tag}-work"));

    origin.write("shared.txt", b"theirs\n");
    origin.write("theirs-only.txt", b"theirs\n");
    origin.commit_all("theirs");

    work.write("shared.txt", b"ours\n");
    work.commit_all("ours");
    (origin, work)
}

fn state_of(work: &TempRepo) -> cide_ipc::git::MergeState {
    conflict::state(&work.root)
        .expect("state")
        .expect("an operation is in progress")
}

// --- landing --------------------------------------------------------------------------------

#[test]
fn a_conflicting_merge_pull_lands_real_git_state_and_reports_it() {
    let (_origin, work) = conflicting("merge-land");

    let outcome = pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("a landed conflict is not an error");
    assert_eq!(outcome.strategy, Some(PullStrategy::Merge));
    assert_eq!(outcome.conflicts, vec!["shared.txt".to_string()]);

    // Real state, which is the whole point. Any of these three missing would mean the conflict
    // could not be finished, abandoned or even seen from a terminal.
    assert!(work.root.join(".git/MERGE_HEAD").exists(), "MERGE_HEAD");
    assert!(
        work.git(&["status", "--porcelain"])
            .contains("UU shared.txt"),
        "git itself sees the conflict"
    );
    assert!(
        String::from_utf8_lossy(&work.read("shared.txt")).contains("<<<<<<<"),
        "and the working tree carries git's markers"
    );

    // The non-conflicting half of the merge came along.
    assert!(work.root.join("theirs-only.txt").exists());

    let state = state_of(&work);
    assert_eq!(state.operation, "merge");
    assert_eq!(state.ours, "main");
    assert_eq!(state.step, None, "a merge is one step by construction");
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.entries[0].path, "shared.txt");
    assert!(!state.entries[0].resolved);
}

#[test]
fn a_conflicting_rebase_pull_stops_with_the_sequencer_on_disk() {
    let (_origin, work) = conflicting("rebase-land");

    let outcome = pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("a landed conflict is not an error");
    assert_eq!(outcome.strategy, Some(PullStrategy::Rebase));
    assert_eq!(outcome.conflicts, vec!["shared.txt".to_string()]);

    assert!(
        work.root.join(".git/rebase-merge").exists(),
        "the sequencer's state, which is what makes Continue possible"
    );
    let state = state_of(&work);
    assert_eq!(state.operation, "rebase");
    let step = state.step.expect("a rebase counts its steps");
    assert_eq!((step.done, step.total), (1, 1));
}

// --- reading --------------------------------------------------------------------------------

#[test]
fn reading_a_conflict_gives_three_sides_and_labels_them() {
    let (_origin, work) = conflicting("read");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    let file = conflict::read(&work.root, "shared.txt").expect("read");
    assert_eq!(file.base.as_deref(), Some("base\n"));
    assert_eq!(file.ours.as_deref(), Some("ours\n"));
    assert_eq!(file.theirs.as_deref(), Some("theirs\n"));
    assert!(!file.binary);
    assert_eq!(file.too_large, None);
    assert_eq!(file.our_label, "HEAD (main)");
    assert!(!file.their_label.is_empty());
}

#[test]
fn a_delete_modify_conflict_has_a_missing_side() {
    // The case a three-pane view would otherwise render as an empty document, saying "they
    // deleted every line" where the truth is "they deleted the file".
    let origin = TempRepo::new("del-mod-origin");
    origin.write("gone.txt", b"base\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "del-mod-work");

    origin.remove("gone.txt");
    origin.commit_all("they deleted it");
    work.write("gone.txt", b"ours\n");
    work.commit_all("we edited it");

    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    let file = conflict::read(&work.root, "gone.txt").expect("read");
    assert_eq!(file.base.as_deref(), Some("base\n"));
    assert_eq!(file.ours.as_deref(), Some("ours\n"));
    assert_eq!(file.theirs, None, "their side deleted the file");
}

#[test]
fn a_binary_conflict_reports_itself_instead_of_carrying_text() {
    let origin = TempRepo::new("bin-origin");
    origin.write("blob.bin", &[0u8, 1, 2, 3, 0, 5]);
    origin.commit_all("first");
    let work = clone_of(&origin, "bin-work");

    origin.write("blob.bin", &[0u8, 9, 9, 9, 0, 9]);
    origin.commit_all("theirs");
    work.write("blob.bin", &[0u8, 7, 7, 7, 0, 7]);
    work.commit_all("ours");

    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    let file = conflict::read(&work.root, "blob.bin").expect("read");
    assert!(file.binary);
    // Refused **instead of** filled in, so a caller that ignores the flag renders nothing
    // rather than mojibake.
    assert_eq!(file.base, None);
    assert_eq!(file.ours, None);
    assert_eq!(file.theirs, None);
}

// --- resolving ------------------------------------------------------------------------------

#[test]
fn taking_a_side_resolves_the_path_and_stages_it() {
    let (_origin, work) = conflicting("take");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    conflict::take_side(&work.root, "shared.txt", ConflictSide::Theirs).expect("take theirs");

    assert_eq!(work.read("shared.txt"), b"theirs\n", "the working tree");
    assert!(
        !work.git(&["status", "--porcelain"]).contains("UU"),
        "git no longer sees a conflict"
    );
    let state = state_of(&work);
    assert!(state.entries[0].resolved, "and the panel says so");
}

#[test]
fn resolving_with_edited_text_writes_exactly_that() {
    let (_origin, work) = conflicting("resolve");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    conflict::resolve(&work.root, "shared.txt", b"a bit of both\n").expect("resolve");
    assert_eq!(work.read("shared.txt"), b"a bit of both\n");
    assert!(!work.git(&["status", "--porcelain"]).contains("UU"));
}

#[test]
fn resolving_a_path_that_is_not_conflicted_is_refused() {
    // Two windows can have the panel open, and the ordinary cause of this is one acting on a
    // row the other has already resolved.
    let (_origin, work) = conflicting("not-conflicted");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    let err = conflict::resolve(&work.root, "calm.txt", b"anything\n").expect_err("refused");
    assert!(matches!(err, GitError::NotConflicted { ref path } if path == "calm.txt"));
}

#[test]
fn a_resolved_path_can_be_put_back_into_conflict() {
    let (_origin, work) = conflicting("unresolve");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    conflict::take_side(&work.root, "shared.txt", ConflictSide::Ours).expect("take ours");
    assert!(state_of(&work).entries[0].resolved);

    conflict::unresolve(&work.root, "shared.txt").expect("unresolve");
    assert!(!state_of(&work).entries[0].resolved);
    // The stages are back, so the resolver can read all three again.
    let file = conflict::read(&work.root, "shared.txt").expect("read");
    assert_eq!(file.ours.as_deref(), Some("ours\n"));
    assert_eq!(file.theirs.as_deref(), Some("theirs\n"));
}

// --- finishing ------------------------------------------------------------------------------

#[test]
fn continuing_a_merge_writes_a_two_parent_commit_with_gits_message() {
    let (_origin, work) = conflicting("finish-merge");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");
    conflict::take_side(&work.root, "shared.txt", ConflictSide::Theirs).expect("take");

    let outcome = conflict::cont(&work.root, None).expect("continue");
    assert!(!outcome.oid.is_empty());
    assert_eq!(outcome.state, None, "the operation is over");

    assert_eq!(
        work.git(&["log", "-1", "--format=%P"])
            .split_whitespace()
            .count(),
        2,
        "a real merge commit"
    );
    // `MERGE_MSG` is what git prepared, so a merge concluded from the panel carries the same
    // message as one concluded from a terminal.
    assert!(
        work.git(&["log", "-1", "--format=%B"])
            .starts_with("Merge branch 'main'"),
        "got {:?}",
        work.git(&["log", "-1", "--format=%B"])
    );
    assert_eq!(work.read("shared.txt"), b"theirs\n");
    assert_no_sequencer_state(&work, "after concluding a merge");
    assert_eq!(work.git(&["status", "--porcelain"]).trim(), "");
}

#[test]
fn continuing_a_rebase_advances_and_then_finishes() {
    let origin = TempRepo::new("finish-rebase-origin");
    origin.write("shared.txt", b"base\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "finish-rebase-work");

    origin.write("shared.txt", b"theirs\n");
    origin.commit_all("theirs");
    // Two local commits, both touching the same line, so the rebase stops twice.
    work.write("shared.txt", b"ours one\n");
    work.commit_all("ours one");
    work.write("shared.txt", b"ours two\n");
    work.commit_all("ours two");

    pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    let first = state_of(&work);
    assert_eq!(first.step.expect("step").total, 2);

    /*
     * `Theirs`, not `Ours`, and that is git's own inversion rather than a slip.
     *
     * During a rebase `HEAD` is the *new base* while each of your commits is replayed on top,
     * so stage 2 — "ours" — is the upstream and stage 3 — "theirs" — is your own commit.
     * `conflict::side_labels` is what stops the resolver's panes lying about this; the test
     * takes stage 3 because that is where "ours one" and "ours two" actually are.
     */
    // Resolved to text that is neither side, which is the ordinary outcome of actually using a
    // merge tool — and it is what makes the second commit conflict too. Taking stage 3 whole
    // would leave the base at exactly "ours one", the second commit's patch would apply
    // cleanly, and the rebase would run to the end without a second stop.
    conflict::resolve(&work.root, "shared.txt", b"merged one\n").expect("resolve");
    let outcome = conflict::cont(&work.root, None).expect("continue");
    // Stopped again on the second commit rather than finishing.
    let second = outcome.state.expect("still rebasing");
    assert_eq!(second.operation, "rebase");
    assert_eq!(second.step.expect("step").done, 2);

    conflict::take_side(&work.root, "shared.txt", ConflictSide::Theirs).expect("take");
    let done = conflict::cont(&work.root, None).expect("continue");
    assert_eq!(done.state, None, "finished");
    assert_no_sequencer_state(&work, "after finishing a rebase");
    assert_eq!(work.read("shared.txt"), b"ours two\n");
    assert_eq!(
        work.git(&["rev-list", "--count", "origin/main..HEAD"])
            .trim(),
        "2",
        "both commits survived the rebase"
    );
}

#[test]
fn continuing_with_something_still_unresolved_is_refused() {
    let (_origin, work) = conflicting("continue-early");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    let err = conflict::cont(&work.root, None).expect_err("refused");
    assert!(
        matches!(err, GitError::Conflicted { ref paths } if paths == &["shared.txt".to_string()]),
        "got {err:?}"
    );
    assert!(
        work.root.join(".git/MERGE_HEAD").exists(),
        "and nothing was concluded"
    );
}

// --- getting back out -------------------------------------------------------------------------

#[test]
fn aborting_a_merge_puts_the_tree_back_exactly() {
    let (_origin, work) = conflicting("abort-merge");
    let before_head = work.git(&["rev-parse", "HEAD"]);
    let before_tree = worktree_hash(&work);

    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");
    // Resolve one thing first, so the abort has something to undo beyond the markers.
    conflict::take_side(&work.root, "shared.txt", ConflictSide::Theirs).expect("take");

    conflict::abort(&work.root).expect("abort");

    assert_eq!(work.git(&["rev-parse", "HEAD"]), before_head);
    assert_eq!(worktree_hash(&work), before_tree, "byte-identical");
    assert_eq!(work.git(&["status", "--porcelain"]).trim(), "");
    assert_no_sequencer_state(&work, "after aborting a merge");
    assert!(conflict::state(&work.root).expect("state").is_none());
}

#[test]
fn aborting_a_rebase_puts_the_branch_back() {
    let (_origin, work) = conflicting("abort-rebase");
    let before_head = work.git(&["rev-parse", "HEAD"]);
    let before_tree = worktree_hash(&work);

    pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");
    conflict::abort(&work.root).expect("abort");

    assert_eq!(work.git(&["rev-parse", "HEAD"]), before_head);
    assert_eq!(worktree_hash(&work), before_tree);
    assert_no_sequencer_state(&work, "after aborting a rebase");
}

#[test]
fn a_conflict_started_outside_cide_is_picked_up() {
    // The realistic case: `git merge` typed into a terminal pane. The roster is snapshotted
    // lazily precisely so this works — a roster written only by cide's own entry points would
    // leave this conflict with no resolved column at all.
    let (_origin, work) = conflicting("outside");
    work.git(&["fetch", "-q", "origin"]);
    let (ok, _) = work.try_git(&["merge", "origin/main"]);
    assert!(!ok, "the merge should have conflicted");

    let state = state_of(&work);
    assert_eq!(state.operation, "merge");
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.entries[0].path, "shared.txt");

    conflict::take_side(&work.root, "shared.txt", ConflictSide::Ours).expect("take");
    conflict::cont(&work.root, None).expect("continue");
    assert_no_sequencer_state(&work, "after finishing a terminal's merge");
}

#[test]
fn the_roster_does_not_survive_into_the_next_operation() {
    let (_origin, work) = conflicting("roster");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");
    assert_eq!(state_of(&work).entries.len(), 1);
    conflict::abort(&work.root).expect("abort");

    // A second, different conflict must not inherit the first's paths.
    let (_o2, w2) = conflicting("roster-two");
    pull::pull_with(
        &w2.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");
    assert!(conflict::state(&work.root).expect("state").is_none());
    assert_eq!(state_of(&w2).entries.len(), 1);
}

#[test]
fn a_rebases_panes_are_labelled_the_way_round_a_rebase_actually_works() {
    // git's own inversion: during a rebase, stage 2 is the branch being rebased *onto* and
    // stage 3 is your own commit. A resolver that said `HEAD (main)` over the left pane would
    // have *Accept Yours* take the other branch's work.
    let (_origin, work) = conflicting("labels");
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    let file = conflict::read(&work.root, "shared.txt").expect("read");
    assert!(
        file.our_label.starts_with("Rebasing onto"),
        "left pane: {:?}",
        file.our_label
    );
    assert!(
        file.their_label.starts_with("Your commit"),
        "right pane: {:?}",
        file.their_label
    );
    // And the content confirms it: stage 3 is the local commit.
    assert_eq!(file.theirs.as_deref(), Some("ours\n"));
    assert_eq!(file.ours.as_deref(), Some("theirs\n"));
}
