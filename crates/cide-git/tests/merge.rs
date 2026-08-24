//! `cide_git::merge`, against the real `git` binary wherever the answer is git's to define.
//!
//! # The oracle is `git merge`, never `git pull`
//!
//! The mirror image of `tests/pull.rs`'s rule, and the reason this is a separate file with a
//! separate fixture: a pull's merge message comes from `fmt-merge-msg` reading `FETCH_HEAD`,
//! so it names the *remote-side* branch and the URL it came from, while `git merge topic`
//! writes `Merge branch 'topic'` and `git merge origin/topic` writes `Merge remote-tracking
//! branch 'origin/topic'`. Comparing against the wrong binary command would pin the wrong
//! sentence and pass.
//!
//! `--no-edit` throughout, so the twin commits the message `fmt-merge-msg` produced rather
//! than opening an editor the test does not have.
//!
//! # What is deliberately not compared
//!
//! **Reflogs**, for `tests/pull.rs`'s reason: cide writes `cide: merge` rather than
//! `merge topic: Fast-forward`, because a person reading `git reflog` after something
//! surprising is better served by a line that says which tool moved the ref. **Committer
//! dates** are not compared either — the two commits are made milliseconds apart.

mod support;

use cide_git::{conflict, merge};
use cide_ipc::git::GitError;
use support::{TempRepo, assert_no_sequencer_state, clone_of, worktree_hash};

fn head(repo: &TempRepo) -> String {
    repo.git(&["rev-parse", "HEAD"]).trim().to_string()
}

fn rev(repo: &TempRepo, name: &str) -> String {
    repo.git(&["rev-parse", name]).trim().to_string()
}

/// A twin holding exactly `work`'s branches, at the same oids, with the last one checked out.
///
/// `tests/pull.rs::twin_of`'s trick without the origin in the middle: the history is
/// *transferred* by fetch rather than rebuilt by replaying the same commands, so the two
/// repositories agree about every oid and the `git merge` the twin runs starts from the exact
/// commits `merge_into_head` starts from.
fn twin_of(work: &TempRepo, tag: &str, branches: &[&str]) -> TempRepo {
    let twin = TempRepo::new(tag);
    twin.git(&[
        "remote",
        "add",
        "work",
        work.root.to_str().expect("utf-8 path"),
    ]);
    twin.git(&["fetch", "-q", "work"]);
    for branch in branches {
        twin.git(&["checkout", "-q", "-B", branch, &format!("work/{branch}")]);
    }
    twin
}

/// One repository on `main` with a diverged local `topic`, and its twin.
///
/// The sides touch different files, so the merge is clean; the conflicted cases build their
/// own history where both sides edit `shared.txt`.
fn diverged(tag: &str) -> (TempRepo, TempRepo) {
    let work = TempRepo::new(&format!("{tag}-work"));
    work.write("a.txt", b"one\n");
    work.write("shared.txt", b"base\n");
    work.commit_all("first");
    work.git(&["checkout", "-q", "-b", "topic"]);
    work.write("theirs.txt", b"theirs\n");
    work.commit_all("theirs");
    work.git(&["checkout", "-q", "main"]);
    work.write("mine.txt", b"mine\n");
    work.commit_all("mine");
    let twin = twin_of(&work, &format!("{tag}-twin"), &["topic", "main"]);
    (work, twin)
}

/// Both sides edit the same line of `shared.txt`, so the merge must stop.
fn conflicting(tag: &str) -> TempRepo {
    let work = TempRepo::new(&format!("{tag}-work"));
    work.write("shared.txt", b"base\n");
    work.write("calm.txt", b"calm\n");
    work.commit_all("first");
    work.git(&["checkout", "-q", "-b", "topic"]);
    work.write("shared.txt", b"theirs\n");
    work.write("theirs.txt", b"theirs\n");
    work.commit_all("theirs");
    work.git(&["checkout", "-q", "main"]);
    work.write("shared.txt", b"mine\n");
    work.commit_all("mine");
    work
}

// --- differential -------------------------------------------------------------------------

#[test]
fn a_clean_merge_matches_the_git_binary() {
    let (work, twin) = diverged("merge-clean");

    let outcome = merge::merge_into_head(&work.root, "topic").expect("clean merge");
    assert!(outcome.conflicts.is_empty(), "this merge is clean");
    assert!(
        !outcome.fast_forward,
        "both sides moved, so this is a real merge commit"
    );
    assert_eq!(outcome.source, "topic");
    assert_eq!(outcome.branch, "main");
    assert_eq!(outcome.advanced, 1);
    assert_eq!(outcome.commits.len(), 1);
    assert_eq!(outcome.commits[0].summary, "theirs");
    assert_eq!(outcome.files_changed, 1);
    assert_eq!(outcome.insertions, 1);

    twin.git(&["merge", "--no-edit", "-q", "topic"]);

    assert_eq!(
        work.git(&["rev-parse", "HEAD^{tree}"]),
        twin.git(&["rev-parse", "HEAD^{tree}"]),
        "the merged tree"
    );
    assert_eq!(
        worktree_hash(&work),
        worktree_hash(&twin),
        "the working tree"
    );
    assert_eq!(
        work.git(&["ls-files", "--stage"]),
        twin.git(&["ls-files", "--stage"]),
        "the index"
    );
    // The message, byte for byte. This is what pins `merge_message` — including that `main`
    // takes no ` into ` clause, which the dedicated case below walks across three names.
    assert_eq!(
        work.git(&["log", "-1", "--format=%B"]),
        twin.git(&["log", "-1", "--format=%B"]),
        "the merge message"
    );
    // Parent order. `[theirs, ours]` is a perfectly valid commit that is wrong in a way only
    // `git log --first-parent` reveals, months later.
    assert_eq!(
        work.git(&["log", "-1", "--format=%P"])
            .split_whitespace()
            .count(),
        2
    );
    assert_eq!(
        work.git(&["rev-parse", "HEAD^1^{tree}"]),
        twin.git(&["rev-parse", "HEAD^1^{tree}"]),
        "first parent is ours"
    );
    assert_eq!(
        work.git(&["log", "-1", "--format=%an <%ae>"]),
        twin.git(&["log", "-1", "--format=%an <%ae>"]),
        "the author is the user, not cide"
    );
    assert_eq!(
        work.git(&["rev-parse", "ORIG_HEAD"]),
        twin.git(&["rev-parse", "ORIG_HEAD"]),
        "ORIG_HEAD names where you were"
    );
    assert_no_sequencer_state(&work, "after a clean merge");
}

#[test]
fn the_merge_message_names_the_branch_except_on_master_and_main() {
    // The same hardcoded `fmt-merge-msg.c` pair `tests/pull.rs` pins for pulls, re-proven for
    // `git merge`'s sentence — `trunk` is again what shows the rule is the pair, not
    // `init.defaultBranch`.
    for dest in ["master", "main", "trunk"] {
        let work = TempRepo::new(&format!("merge-msg-{dest}-work"));
        work.git(&["checkout", "-q", "-b", dest]);
        work.write("a.txt", b"one\n");
        work.commit_all("first");
        work.git(&["checkout", "-q", "-b", "topic"]);
        work.write("theirs.txt", b"theirs\n");
        work.commit_all("theirs");
        work.git(&["checkout", "-q", dest]);
        work.write("mine.txt", b"mine\n");
        work.commit_all("mine");
        let twin = twin_of(&work, &format!("merge-msg-{dest}-twin"), &["topic", dest]);

        merge::merge_into_head(&work.root, "topic").expect("merge");
        twin.git(&["merge", "--no-edit", "-q", "topic"]);

        let ours = work.git(&["log", "-1", "--format=%B"]);
        assert_eq!(ours, twin.git(&["log", "-1", "--format=%B"]), "on {dest}");
        // And the rule itself, stated, so a future reader does not have to run git to see it.
        assert_eq!(
            ours.contains(" into "),
            dest != "master" && dest != "main",
            "the `into` clause on {dest}"
        );
    }
}

#[test]
fn a_remote_tracking_source_says_remote_tracking_branch() {
    let origin = TempRepo::new("merge-rt-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    origin.git(&["checkout", "-q", "-b", "topic"]);
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");
    origin.git(&["checkout", "-q", "main"]);

    let work = clone_of(&origin, "merge-rt-work");
    work.write("mine.txt", b"mine\n");
    work.commit_all("mine");
    /*
     * The trap this test exists for: a *local* branch with the source's short name, pointing
     * somewhere else entirely. `git merge origin/topic` merges the remote-tracking ref, and a
     * resolver that strips the prefix — `branch::find`'s checkout semantics, which map
     * `origin/topic` onto a local `topic` — would merge this decoy instead, silently, exactly
     * when the two have drifted apart.
     */
    work.git(&["branch", "-q", "topic", "HEAD"]);

    let twin = twin_of(&work, "merge-rt-twin", &["main"]);
    twin.git(&[
        "remote",
        "add",
        "origin",
        origin.root.to_str().expect("utf-8 path"),
    ]);
    twin.git(&["fetch", "-q", "origin"]);
    twin.git(&["branch", "-q", "topic", "HEAD"]);

    let outcome = merge::merge_into_head(&work.root, "origin/topic").expect("merge");
    assert_eq!(outcome.source, "origin/topic");
    twin.git(&["merge", "--no-edit", "-q", "origin/topic"]);

    assert_eq!(
        work.git(&["log", "-1", "--format=%B"]),
        twin.git(&["log", "-1", "--format=%B"]),
        "the message says 'remote-tracking branch', byte for byte"
    );
    assert_eq!(
        rev(&work, "HEAD^2"),
        rev(&work, "origin/topic"),
        "the second parent is the remote-tracking ref, not the same-named local decoy"
    );
    assert_eq!(
        work.git(&["rev-parse", "HEAD^{tree}"]),
        twin.git(&["rev-parse", "HEAD^{tree}"]),
        "the merged tree"
    );
    assert_no_sequencer_state(&work, "after merging a remote-tracking ref");
}

#[test]
fn a_fast_forward_moves_the_branch_and_makes_no_commit() {
    let work = TempRepo::new("merge-ff-work");
    work.write("a.txt", b"one\n");
    work.commit_all("first");
    let old = head(&work);
    work.git(&["checkout", "-q", "-b", "topic"]);
    work.write("theirs.txt", b"theirs\n");
    work.commit_all("theirs");
    work.git(&["checkout", "-q", "main"]);
    let twin = twin_of(&work, "merge-ff-twin", &["topic", "main"]);

    let outcome = merge::merge_into_head(&work.root, "topic").expect("fast-forward");
    assert!(outcome.fast_forward);
    assert_eq!(outcome.advanced, 1);
    assert!(outcome.conflicts.is_empty());

    twin.git(&["merge", "--no-edit", "-q", "topic"]);

    assert_eq!(
        head(&work),
        rev(&work, "topic"),
        "the branch moved to the tip"
    );
    assert_eq!(head(&work), head(&twin), "to the same commit git moved to");
    assert_eq!(
        worktree_hash(&work),
        worktree_hash(&twin),
        "the working tree"
    );
    assert_eq!(
        rev(&work, "ORIG_HEAD"),
        old,
        "ORIG_HEAD names where you were"
    );
    assert_no_sequencer_state(&work, "after a fast-forward merge");
}

#[test]
fn an_ancestor_is_already_up_to_date() {
    let work = TempRepo::new("merge-utd-work");
    work.write("a.txt", b"one\n");
    work.commit_all("first");
    work.git(&["branch", "-q", "old"]);
    work.write("b.txt", b"two\n");
    work.commit_all("second");
    let before = head(&work);

    let outcome = merge::merge_into_head(&work.root, "old").expect("already up to date");
    assert_eq!(outcome.advanced, 0);
    assert_eq!(
        outcome.old_oid, outcome.new_oid,
        "nothing happened, and the report says so"
    );
    assert!(outcome.conflicts.is_empty());
    assert_eq!(head(&work), before, "HEAD did not move");

    // Merging the branch you are standing on is the same nothing — the popup hides the item,
    // but the popup's list can be stale, so the entry point must answer for itself.
    let own = merge::merge_into_head(&work.root, "main").expect("self merge");
    assert_eq!(own.advanced, 0);
    assert_eq!(head(&work), before);

    // And a name that resolves to nothing is a refusal, not a panic.
    let missing = merge::merge_into_head(&work.root, "nope").expect_err("no such branch");
    assert!(matches!(missing, GitError::NoSuchBranch { .. }));
    assert_no_sequencer_state(&work, "after the merges that did nothing");
}

#[test]
fn a_conflicted_merge_leaves_real_state_and_the_surface_finishes_it() {
    let work = conflicting("merge-conf");
    let twin = twin_of(&work, "merge-conf-twin", &["topic", "main"]);

    let outcome = merge::merge_into_head(&work.root, "topic").expect("conflicts are Ok, not Err");
    assert_eq!(outcome.conflicts, vec!["shared.txt".to_string()]);
    assert_eq!(outcome.old_oid, outcome.new_oid, "HEAD has not moved yet");
    assert_eq!(
        outcome.advanced, 1,
        "the commits the merge takes are still counted"
    );

    // Real git state, which is the whole of ADR 0009: resumable after a restart, visible to
    // `git status` in a pane, abandonable with `git merge --abort`.
    assert_eq!(
        rev(&work, "MERGE_HEAD"),
        rev(&work, "topic"),
        "MERGE_HEAD is the source tip"
    );
    assert!(
        work.git(&["status", "--porcelain"])
            .contains("UU shared.txt"),
        "git's own status sees the conflict"
    );
    assert!(
        String::from_utf8(work.read("shared.txt"))
            .expect("utf-8")
            .contains("<<<<<<<"),
        "git's markers are in the working tree"
    );
    assert_eq!(
        work.read("theirs.txt"),
        b"theirs\n",
        "the half with nothing to decide arrived"
    );

    // A second merge over the first is one of the refusals ADR 0009 deliberately kept.
    let second = merge::merge_into_head(&work.root, "topic").expect_err("second merge refused");
    assert!(matches!(second, GitError::OperationInProgress { .. }));

    // The surface can see it and finish it.
    let state = conflict::state(&work.root)
        .expect("state")
        .expect("a merge is in progress");
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.entries[0].path, "shared.txt");
    assert!(!state.entries[0].resolved);

    /*
     * The twin resolves the same way with the plain commands a person would type — and the
     * concluding commit runs with an *editor* (one that changes nothing), not `--no-edit`.
     * That is not a convenience: `commit.cleanup`'s default strips `#` lines only when the
     * message was opened for editing, so `git commit --no-edit` after a conflicted merge
     * keeps the `# Conflicts:` block verbatim, which no interactive user ever sees. The
     * person at a terminal types `git commit`, the editor opens, and the block is stripped —
     * that is the message cide has to match.
     */
    let (clean, _) = twin.try_git(&["merge", "--no-edit", "-q", "topic"]);
    assert!(!clean, "the twin conflicts too");
    twin.write("shared.txt", b"resolved\n");
    twin.git(&["add", "shared.txt"]);
    twin.git(&["-c", "core.editor=true", "commit", "-q"]);

    conflict::resolve(&work.root, "shared.txt", b"resolved\n").expect("resolve");
    conflict::cont(&work.root, None).expect("continue");

    assert_eq!(
        work.git(&["log", "-1", "--format=%B"]),
        twin.git(&["log", "-1", "--format=%B"]),
        "the concluded message is the one a clean merge would have written — `merge_into_head` \
         rewrote MERGE_MSG before anything could read libgit2's"
    );
    assert_eq!(
        work.git(&["rev-parse", "HEAD^{tree}"]),
        twin.git(&["rev-parse", "HEAD^{tree}"]),
        "the concluded tree"
    );
    assert_no_sequencer_state(&work, "after concluding the merge");
}

#[test]
fn aborting_a_conflicted_merge_puts_the_tree_back_exactly() {
    let work = conflicting("merge-abort");
    let before_tree = worktree_hash(&work);
    let before_head = head(&work);

    let outcome = merge::merge_into_head(&work.root, "topic").expect("conflicted merge");
    assert!(!outcome.conflicts.is_empty());

    conflict::abort(&work.root).expect("abort");
    assert_eq!(
        worktree_hash(&work),
        before_tree,
        "the tree is byte-identical"
    );
    assert_eq!(head(&work), before_head, "HEAD is back");
    assert_no_sequencer_state(&work, "after aborting the merge");
}

#[test]
fn unrelated_histories_are_refused_before_anything_is_written() {
    let work = TempRepo::new("merge-unrel-work");
    work.write("a.txt", b"one\n");
    work.commit_all("first");
    work.git(&["checkout", "-q", "--orphan", "isle"]);
    work.git(&["rm", "-q", "--cached", "a.txt"]);
    work.remove("a.txt");
    work.write("island.txt", b"alone\n");
    work.commit_all("alone");
    work.git(&["checkout", "-q", "main"]);
    let before = worktree_hash(&work);

    let err = merge::merge_into_head(&work.root, "isle").expect_err("unrelated");
    assert!(matches!(err, GitError::UnrelatedHistories { .. }));
    assert_eq!(worktree_hash(&work), before, "the refusal wrote nothing");
    assert_no_sequencer_state(&work, "after an unrelated-histories refusal");
}

#[test]
fn a_dirty_file_in_the_way_is_refused_under_both_arms() {
    // The merge arm: both branches have their own commits, and the file the merge would
    // rewrite carries an uncommitted edit.
    let work = conflicting("merge-dirty");
    work.write("shared.txt", b"uncommitted\n");
    let before = worktree_hash(&work);

    let err = merge::merge_into_head(&work.root, "topic").expect_err("blocked merge");
    match err {
        GitError::CheckoutWouldOverwrite { branch, paths } => {
            // The refusal names the *source* — the sentence built from it is "Merging topic
            // would overwrite…", and the branch that is not moving is not the news.
            assert_eq!(branch, "topic");
            assert_eq!(paths, vec!["shared.txt".to_string()]);
        }
        other => panic!("expected CheckoutWouldOverwrite, got {other:?}"),
    }
    assert_eq!(worktree_hash(&work), before, "nothing moved");
    assert_no_sequencer_state(&work, "after a blocked merge");

    // The fast-forward arm: the current branch has nothing of its own, and the same dirty
    // file blocks the checkout half of the fast-forward.
    let ff = TempRepo::new("merge-dirty-ff");
    ff.write("shared.txt", b"base\n");
    ff.commit_all("first");
    ff.git(&["checkout", "-q", "-b", "topic"]);
    ff.write("shared.txt", b"theirs\n");
    ff.commit_all("theirs");
    ff.git(&["checkout", "-q", "main"]);
    ff.write("shared.txt", b"uncommitted\n");
    let before = worktree_hash(&ff);

    let err = merge::merge_into_head(&ff.root, "topic").expect_err("blocked fast-forward");
    assert!(matches!(err, GitError::CheckoutWouldOverwrite { .. }));
    assert_eq!(worktree_hash(&ff), before, "nothing moved");
    assert_no_sequencer_state(&ff, "after a blocked fast-forward");
}

#[test]
fn a_detached_head_is_refused_by_the_entry_point() {
    // The popup hides the action on a detached HEAD, but the popup's list can be minutes
    // stale — the Rust refusal is the truth the UI's gate approximates.
    let work = TempRepo::new("merge-detached-work");
    work.write("a.txt", b"one\n");
    work.commit_all("first");
    work.git(&["branch", "-q", "topic"]);
    work.git(&["checkout", "-q", "--detach"]);

    let err = merge::merge_into_head(&work.root, "topic").expect_err("detached");
    assert!(matches!(err, GitError::DetachedHead { .. }));
    assert_no_sequencer_state(&work, "after a detached-HEAD refusal");
}
