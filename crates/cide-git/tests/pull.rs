//! `cide_git::pull`, against the real `git` binary wherever the answer is git's to define.
//!
//! # The oracle is `git pull`, never `git merge`
//!
//! A pull's merge message comes from `fmt-merge-msg` reading `FETCH_HEAD`, so it names the
//! *remote-side* branch and the URL it came from: `git merge origin/main` writes *"Merge
//! remote-tracking branch 'origin/main'"* and a pull writes *"Merge branch 'main' of /tmp/…"*.
//! Comparing against `git merge` would pin the wrong sentence and pass.
//!
//! `--no-edit` throughout, and the strategy comes from `-c pull.rebase=…` rather than from
//! config precedence, so the twin cannot be influenced by whatever the config test left behind.
//!
//! # What is deliberately not compared
//!
//! **Reflogs.** cide's messages are its own — `cide: pull` rather than `pull: Fast-forward` —
//! because a person reading `git reflog` after something surprising is better served by a line
//! that says which tool moved the ref than by one impersonating git. That is a decision, so the
//! tests state it rather than discovering it.
//!
//! **Committer dates.** A rebase preserves the *author* date and moves the committer date, so
//! `%at` is comparable between cide and git and `%ct` is not.

mod support;

use cide_git::pull;
use cide_ipc::git::{GitError, PullDefault, PullRequest, PullStrategy};
use cide_ipc::history::{LogCursor, LogQuery, LogRefs, LogScope, Simplify};
use support::{TempRepo, assert_no_sequencer_state, clone_of, worktree_hash};

/// `cide-git` takes no configuration, and a pull touches no proxy environment on the libgit2
/// route these tests use. The default is "touch nothing".
fn untouched() -> cide_core::proxy::ProxyEnv {
    cide_core::proxy::ProxyEnv::default()
}

fn request(strategy: PullStrategy) -> PullRequest {
    PullRequest {
        strategy: Some(strategy),
        ..PullRequest::default()
    }
}

/// An origin with one commit, and a clone of it.
fn pair(tag: &str) -> (TempRepo, TempRepo) {
    let origin = TempRepo::new(&format!("{tag}-origin"));
    origin.write("a.txt", b"one\n");
    origin.write("shared.txt", b"base\n");
    origin.commit_all("first");
    let work = clone_of(&origin, &format!("{tag}-work"));
    (origin, work)
}

/// Diverge the pair: a commit on each side that does not touch the other's files.
fn diverge(origin: &TempRepo, work: &TempRepo) {
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");
    work.write("mine.txt", b"mine\n");
    work.commit_all("mine");
}

/// A second clone of `origin` at the same point as `work`, for the twin to act on.
fn twin_of(origin: &TempRepo, work: &TempRepo, tag: &str) -> TempRepo {
    let twin = clone_of(origin, tag);
    // Replay `work`'s local-only commits onto the twin by pulling from `work` itself, so both
    // start from identical histories rather than merely similar ones.
    twin.git(&[
        "remote",
        "add",
        "work",
        work.root.to_str().expect("utf-8 path"),
    ]);
    twin.git(&["fetch", "-q", "work"]);
    twin.git(&["reset", "-q", "--hard", "work/main"]);
    twin
}

fn head(repo: &TempRepo) -> String {
    repo.git(&["rev-parse", "HEAD"]).trim().to_string()
}

fn log_format(repo: &TempRepo, format: &str, range: &str) -> String {
    repo.git(&["log", "--reverse", &format!("--format={format}"), range])
}

// --- differential -------------------------------------------------------------------------

#[test]
fn a_merge_pull_matches_the_git_binary() {
    let (origin, work) = pair("merge-diff");
    diverge(&origin, &work);
    let twin = twin_of(&origin, &work, "merge-diff-twin");

    let outcome = pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("merge pull");
    assert!(outcome.conflicts.is_empty(), "this merge is clean");
    assert_eq!(outcome.strategy, Some(PullStrategy::Merge));

    twin.git(&[
        "-c",
        "pull.rebase=false",
        "pull",
        "--no-edit",
        "-q",
        "origin",
        "main",
    ]);

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
    // The message, byte for byte. This is what pins `merge_message`, including the
    // `master`-versus-`main` suffix rule, which is a hardcoded string comparison in git.
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
        "first parent"
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
    assert_no_sequencer_state(&work, "after a clean merge pull");
}

#[test]
fn the_merge_message_names_the_branch_except_on_master_and_main() {
    // `master` and `main` are a hardcoded pair in `fmt-merge-msg.c`. The obvious guess — that
    // git looks up `init.defaultBranch` — is wrong, and `trunk` here is what proves it: this
    // repository's `init.defaultBranch` is untouched and `trunk` still gets the suffix.
    for branch in ["master", "main", "trunk"] {
        let origin = TempRepo::new(&format!("msg-{branch}-origin"));
        origin.git(&["checkout", "-q", "-b", branch]);
        origin.write("a.txt", b"one\n");
        origin.commit_all("first");

        let work = TempRepo::new(&format!("msg-{branch}-work"));
        work.git(&[
            "remote",
            "add",
            "origin",
            origin.root.to_str().expect("utf-8"),
        ]);
        work.git(&["fetch", "-q", "origin"]);
        work.git(&["checkout", "-q", "-B", branch, &format!("origin/{branch}")]);

        origin.write("theirs.txt", b"theirs\n");
        origin.commit_all("theirs");
        work.write("mine.txt", b"mine\n");
        work.commit_all("mine");

        let twin = TempRepo::new(&format!("msg-{branch}-twin"));
        twin.git(&[
            "remote",
            "add",
            "origin",
            origin.root.to_str().expect("utf-8"),
        ]);
        twin.git(&["remote", "add", "work", work.root.to_str().expect("utf-8")]);
        twin.git(&["fetch", "-q", "--all"]);
        twin.git(&["checkout", "-q", "-B", branch, &format!("work/{branch}")]);
        twin.git(&[
            "branch",
            "-q",
            "--set-upstream-to",
            &format!("origin/{branch}"),
            branch,
        ]);

        pull::pull_with(
            &work.root,
            &request(PullStrategy::Merge),
            PullDefault::Ask,
            &untouched(),
        )
        .expect("merge pull");
        twin.git(&[
            "-c",
            "pull.rebase=false",
            "pull",
            "--no-edit",
            "-q",
            "origin",
            branch,
        ]);

        let ours = work.git(&["log", "-1", "--format=%B"]);
        assert_eq!(ours, twin.git(&["log", "-1", "--format=%B"]), "on {branch}");
        // And the rule itself, stated, so a future reader does not have to run git to see it.
        assert_eq!(
            ours.contains(" into "),
            branch != "master" && branch != "main",
            "the `into` clause on {branch}"
        );
    }
}

#[test]
fn a_rebase_pull_matches_the_git_binary() {
    let (origin, work) = pair("rebase-diff");
    // Two local commits, so the test covers a chain rather than a single replay.
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");
    work.write("mine.txt", b"mine\n");
    work.commit_all("mine one");
    work.write("mine2.txt", b"mine too\n");
    work.commit_all("mine two");
    let twin = twin_of(&origin, &work, "rebase-diff-twin");

    let outcome = pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("rebase pull");
    assert!(outcome.conflicts.is_empty());
    assert_eq!(outcome.rewritten, 2, "both local commits were replayed");
    assert_eq!(outcome.skipped, 0);

    twin.git(&[
        "-c",
        "pull.rebase=true",
        "pull",
        "--no-edit",
        "-q",
        "origin",
        "main",
    ]);

    assert_eq!(
        head(&work),
        head(&twin),
        "the rebased tip is the same commit"
    );
    assert_eq!(worktree_hash(&work), worktree_hash(&twin));
    assert_eq!(
        work.git(&["ls-files", "--stage"]),
        twin.git(&["ls-files", "--stage"])
    );
    assert_eq!(
        work.git(&["rev-list", "--count", "HEAD"]),
        twin.git(&["rev-list", "--count", "HEAD"]),
        "no commit was invented or lost"
    );
    // The whole replayed range: author identity, author date, message. `%at` and not `%ct` —
    // a rebase preserves the author date and moves the committer date.
    let range = "origin/main..HEAD";
    assert_eq!(
        log_format(&work, "%an <%ae>%n%at%n%B", range),
        log_format(&twin, "%an <%ae>%n%at%n%B", range),
        "authors, author dates and messages survive the replay"
    );
    assert_eq!(
        work.git(&["rev-parse", "ORIG_HEAD"]),
        twin.git(&["rev-parse", "ORIG_HEAD"])
    );
    assert_no_sequencer_state(&work, "after a clean rebase pull");
}

#[test]
fn a_rebase_pull_keeps_the_author_and_moves_the_committer() {
    let (origin, work) = pair("rebase-author");
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");

    work.write("mine.txt", b"mine\n");
    work.git(&["add", "-A"]);
    work.git(&[
        "-c",
        "user.name=Someone Else",
        "-c",
        "user.email=else@example.invalid",
        "commit",
        "-q",
        "-m",
        "theirs originally",
    ]);

    pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("rebase pull");

    assert_eq!(
        work.git(&["log", "-1", "--format=%an <%ae>"]).trim(),
        "Someone Else <else@example.invalid>",
        "the author is kept — a replayed commit must not claim somebody else's work"
    );
    assert_eq!(
        work.git(&["log", "-1", "--format=%cn <%ce>"]).trim(),
        "cide tests <tests@cide.invalid>",
        "the committer is whoever ran the rebase"
    );
}

#[test]
fn a_rebase_pull_skips_a_commit_that_is_already_upstream() {
    let (origin, work) = pair("rebase-skip");

    // A commit made locally and then landed upstream — what happens when somebody else merges
    // your patch.
    work.write("shared.txt", b"changed\n");
    work.commit_all("the same change");
    let carried = head(&work);

    // `theirs` **before** the cherry-pick, and that ordering is what makes this a real test.
    // Cherry-picking straight onto the shared parent reproduces the original commit exactly —
    // same tree, same parent, same author, same message, same second — so git hands back a
    // byte-identical oid, `origin/main` genuinely contains it, and the revwalk hides it before
    // the rebase ever sees it. That version passed while testing nothing.
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");
    origin.git(&["remote", "add", "work", work.root.to_str().expect("utf-8")]);
    origin.git(&["fetch", "-q", "work"]);
    origin.git(&["cherry-pick", &carried]);
    assert_ne!(
        origin.git(&["rev-parse", "HEAD"]).trim(),
        carried,
        "upstream's copy must be a different commit object carrying the same patch"
    );

    // …plus one that is genuinely new.
    work.write("mine.txt", b"mine\n");
    work.commit_all("mine");

    let outcome = pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("rebase pull");
    assert_eq!(
        outcome.rewritten, 1,
        "only the genuinely new commit replayed"
    );
    assert_eq!(
        outcome.skipped, 1,
        "and the dropped one is reported, not swallowed"
    );
    assert_eq!(
        work.git(&["rev-list", "--count", "origin/main..HEAD"])
            .trim(),
        "1"
    );
}

#[test]
fn a_rebase_pull_drops_an_originally_empty_commit_and_says_so() {
    /*
     * A documented divergence from `git rebase`, pinned so it stays documented.
     *
     * git keeps a `--allow-empty` commit — `--keep-empty` is its default, and only commits
     * that *become* empty are dropped. libgit2 answers `GIT_EAPPLIED` for both and offers no
     * way to force one through mid-sequence, so cide follows libgit2.
     *
     * What makes that acceptable is the second assertion: it is **counted**. `skipped` is on
     * the wire precisely so a marker commit that vanished shows up in the notice rather than
     * being discovered by somebody reading their own log a week later.
     */
    let (origin, work) = pair("rebase-empty");
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");
    work.git(&["commit", "-q", "--allow-empty", "-m", "a marker"]);

    let outcome = pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("rebase pull");
    assert_eq!(outcome.rewritten, 0);
    assert_eq!(outcome.skipped, 1, "dropped, and reported");
    assert_eq!(
        work.git(&["rev-parse", "HEAD"]),
        work.git(&["rev-parse", "origin/main"]),
        "the branch is now exactly the upstream"
    );
}

// --- refusals, and the promise that nothing moved -------------------------------------------

#[test]
fn a_strategy_does_not_turn_a_fast_forward_into_a_merge() {
    // The case that would silently manufacture pointless merge commits if the strategy were
    // consulted before the ahead check.
    for strategy in [PullStrategy::Merge, PullStrategy::Rebase] {
        let (origin, work) = pair("ff-not-merge");
        origin.write("theirs.txt", b"theirs\n");
        origin.commit_all("theirs");

        let outcome = pull::pull_with(
            &work.root,
            &request(strategy),
            PullDefault::Ask,
            &untouched(),
        )
        .expect("pull");
        assert_eq!(
            outcome.strategy,
            Some(PullStrategy::FastForward),
            "{strategy:?}"
        );
        assert_eq!(outcome.advanced, 1);
        assert_eq!(
            work.git(&["rev-parse", "HEAD"]),
            work.git(&["rev-parse", "origin/main"]),
            "{strategy:?} fast-forwarded"
        );
        assert_eq!(
            work.git(&["log", "-1", "--format=%P"])
                .split_whitespace()
                .count(),
            1,
            "{strategy:?} created no merge commit"
        );
    }
}

#[test]
fn a_branch_merely_ahead_of_its_upstream_is_already_up_to_date() {
    // This used to report `NotFastForward { ahead: 1, behind: 0 }` — a branch with one unpushed
    // commit and a quiet remote, told it had diverged.
    let (_origin, work) = pair("ahead-only");
    work.write("mine.txt", b"mine\n");
    work.commit_all("mine");
    let before = head(&work);

    let outcome = pull::pull(&work.root, None, &untouched()).expect("already up to date");
    assert_eq!(outcome.advanced, 0);
    assert_eq!(outcome.branch, "main");
    assert_eq!(outcome.strategy, None, "nothing was integrated");
    assert_eq!(head(&work), before);
}

#[test]
fn a_rebase_pull_refuses_to_drop_a_merge_commit() {
    let (origin, work) = pair("rebase-merge-commit");
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");

    // A local merge: a side branch folded back into `main`.
    work.git(&["checkout", "-q", "-b", "side"]);
    work.write("side.txt", b"side\n");
    work.commit_all("on the side");
    work.git(&["checkout", "-q", "main"]);
    work.git(&["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);

    let before = head(&work);
    let err = pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect_err("refused");
    let GitError::RebaseWouldDropMerges { branch, commits } = err else {
        panic!("expected RebaseWouldDropMerges, got {err:?}");
    };
    assert_eq!(branch, "main");
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].summary, "Merge side");
    assert_eq!(head(&work), before, "nothing moved");
    assert_no_sequencer_state(&work, "after refusing to drop a merge");

    // And the refusal is a choice, not a wall: merging the same repository works.
    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("merging is still available");
}

#[test]
fn an_unrelated_history_is_refused_before_either_strategy() {
    let origin = TempRepo::new("unrelated-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");

    let work = TempRepo::new("unrelated-work");
    work.write("b.txt", b"other\n");
    work.commit_all("a different beginning");
    work.git(&[
        "remote",
        "add",
        "origin",
        origin.root.to_str().expect("utf-8"),
    ]);
    work.git(&["fetch", "-q", "origin"]);
    work.git(&["branch", "-q", "--set-upstream-to", "origin/main", "main"]);

    let before = head(&work);
    for strategy in [PullStrategy::Merge, PullStrategy::Rebase] {
        let err = pull::pull_with(
            &work.root,
            &request(strategy),
            PullDefault::Ask,
            &untouched(),
        )
        .expect_err("refused");
        assert!(
            matches!(err, GitError::UnrelatedHistories { .. }),
            "{strategy:?} gave {err:?}"
        );
    }
    assert_eq!(head(&work), before);
    assert_no_sequencer_state(&work, "after refusing unrelated histories");
}

#[test]
fn a_pull_will_not_start_during_a_rebase_and_does_not_reach_the_network() {
    let (origin, work) = pair("mid-rebase");
    diverge(&origin, &work);
    // Leave a rebase in progress by conflicting on purpose.
    work.write("shared.txt", b"ours\n");
    work.commit_all("ours");
    origin.write("shared.txt", b"theirs\n");
    origin.commit_all("theirs again");
    work.git(&["fetch", "-q", "origin"]);
    let (started, _) = work.try_git(&["rebase", "origin/main"]);
    assert!(!started, "the rebase should have stopped on a conflict");

    let remote_before = work.git(&["rev-parse", "refs/remotes/origin/main"]);
    // Move the origin so a fetch would be observable.
    origin.write("later.txt", b"later\n");
    origin.commit_all("after");

    let err = pull::pull(&work.root, None, &untouched()).expect_err("refused");
    assert!(
        matches!(err, GitError::OperationInProgress { ref operation } if operation == "rebase"),
        "got {err:?}"
    );
    // The whole point of checking before the fetch: a doomed pull costs no round trip.
    assert_eq!(
        work.git(&["rev-parse", "refs/remotes/origin/main"]),
        remote_before,
        "no fetch happened"
    );

    work.git(&["rebase", "--abort"]);
}

#[test]
fn a_pull_refuses_rather_than_overwriting_a_local_change_under_every_strategy() {
    for strategy in [
        PullStrategy::FastForward,
        PullStrategy::Merge,
        PullStrategy::Rebase,
    ] {
        let (origin, work) = pair("dirty");
        origin.write("shared.txt", b"theirs\n");
        origin.commit_all("theirs");
        if strategy != PullStrategy::FastForward {
            work.write("mine.txt", b"mine\n");
            work.commit_all("mine");
        }
        work.write("shared.txt", b"work in progress\n");

        let err = pull::pull_with(
            &work.root,
            &request(strategy),
            PullDefault::Ask,
            &untouched(),
        )
        .expect_err("refused");
        assert!(
            matches!(err, GitError::CheckoutWouldOverwrite { ref paths, .. } if paths == &["shared.txt".to_string()]),
            "{strategy:?} gave {err:?}"
        );
        assert_eq!(work.read("shared.txt"), b"work in progress\n");
        assert_no_sequencer_state(&work, "after refusing a dirty pull");
    }
}

#[test]
fn a_dirty_file_the_pull_does_not_touch_comes_along() {
    // `cide_git::replay`'s deliberate permissiveness, restated for pulls: refusing on *any*
    // dirty file would make Pull useless in a repository somebody is working in.
    let (origin, work) = pair("dirty-elsewhere");
    diverge(&origin, &work);
    work.write("a.txt", b"work in progress\n");

    pull::pull_with(
        &work.root,
        &request(PullStrategy::Merge),
        PullDefault::Ask,
        &untouched(),
    )
    .expect("the merge does not touch a.txt");
    assert_eq!(work.read("a.txt"), b"work in progress\n");
}

#[test]
fn the_answer_can_be_given_without_fetching_again() {
    let (origin, work) = pair("skip-fetch");
    diverge(&origin, &work);

    // The first pass fetches and asks.
    let err = pull::pull(&work.root, None, &untouched()).expect_err("diverged");
    assert!(matches!(err, GitError::PullNeedsStrategy(_)), "got {err:?}");

    // Take the origin away. A second fetch would now fail, which is the only way to prove one
    // did not happen.
    let moved = origin.root.with_extension("moved");
    std::fs::rename(&origin.root, &moved).expect("move the origin aside");

    let outcome = pull::pull_with(
        &work.root,
        &PullRequest {
            strategy: Some(PullStrategy::Merge),
            skip_fetch: true,
            ..PullRequest::default()
        },
        PullDefault::Ask,
        &untouched(),
    )
    .expect("answered from the refs already on disk");
    assert!(outcome.conflicts.is_empty());
    assert_eq!(outcome.strategy, Some(PullStrategy::Merge));
    assert!(work.root.join("theirs.txt").exists(), "the merge landed");

    std::fs::rename(&moved, &origin.root).expect("put it back");
}

#[test]
fn a_rebase_over_the_cap_is_refused_before_anything_is_replayed() {
    let (origin, work) = pair("rebase-cap");
    origin.write("theirs.txt", b"theirs\n");
    origin.commit_all("theirs");
    for n in 0..=pull::REBASE_COMMIT_CAP {
        work.git(&["commit", "-q", "--allow-empty", "-m", &format!("local {n}")]);
    }

    let before = head(&work);
    let err = pull::pull_with(
        &work.root,
        &request(PullStrategy::Rebase),
        PullDefault::Ask,
        &untouched(),
    )
    .expect_err("refused");
    let GitError::RebaseTooLong { limit, .. } = err else {
        panic!("expected RebaseTooLong, got {err:?}");
    };
    assert_eq!(limit as usize, pull::REBASE_COMMIT_CAP);
    assert_eq!(head(&work), before);
    assert_no_sequencer_state(&work, "after refusing an over-long rebase");
}

// --- the config ladder ------------------------------------------------------------------------

#[test]
fn pull_rebase_follows_gits_own_value_grammar() {
    let cases: &[(&str, PullStrategy)] = &[
        ("true", PullStrategy::Rebase),
        ("yes", PullStrategy::Rebase),
        ("on", PullStrategy::Rebase),
        ("1", PullStrategy::Rebase),
        ("2", PullStrategy::Rebase),
        ("false", PullStrategy::Merge),
        ("no", PullStrategy::Merge),
        ("off", PullStrategy::Merge),
        ("0", PullStrategy::Merge),
        ("merges", PullStrategy::Rebase),
        ("m", PullStrategy::Rebase),
        ("interactive", PullStrategy::Rebase),
        ("i", PullStrategy::Rebase),
    ];
    for (value, expected) in cases {
        let (origin, work) = pair("cfg");
        diverge(&origin, &work);
        work.git(&["config", "pull.rebase", value]);

        let outcome = pull::pull(&work.root, None, &untouched())
            .unwrap_or_else(|e| panic!("pull.rebase={value}: {e:?}"));
        assert_eq!(outcome.strategy, Some(*expected), "pull.rebase={value}");
    }
}

#[test]
fn a_valueless_pull_rebase_key_is_true() {
    // `[pull]` followed by a bare `rebase` line. `Config::get_string` cannot tell this from a
    // missing key; `ConfigEntry::has_value` is the only way to ask, and git reads it as true.
    let (origin, work) = pair("cfg-valueless");
    diverge(&origin, &work);
    let config = work.root.join(".git/config");
    let mut text = std::fs::read_to_string(&config).expect("read config");
    text.push_str("[pull]\n\trebase\n");
    std::fs::write(&config, text).expect("write config");

    let outcome = pull::pull(&work.root, None, &untouched()).expect("pull");
    assert_eq!(outcome.strategy, Some(PullStrategy::Rebase));
}

#[test]
fn an_unrecognised_pull_rebase_value_leaves_the_branch_unconfigured() {
    // git dies on `preserve`; cide cannot die, and quietly choosing merge would be a lie about
    // the user's own file. Unconfigured means the dialog opens, which is where a person can see
    // that something is wrong.
    let (origin, work) = pair("cfg-bogus");
    diverge(&origin, &work);
    work.git(&["config", "pull.rebase", "preserve"]);

    let err = pull::pull(&work.root, None, &untouched()).expect_err("asks");
    assert!(matches!(err, GitError::PullNeedsStrategy(_)), "got {err:?}");
}

#[test]
fn the_branch_key_outranks_the_repo_key_outranks_the_app_default() {
    let (origin, work) = pair("cfg-ladder");
    diverge(&origin, &work);

    // The app default alone.
    let outcome = pull::pull_with(
        &work.root,
        &PullRequest::default(),
        PullDefault::Merge,
        &untouched(),
    )
    .expect("pull");
    assert_eq!(
        outcome.strategy,
        Some(PullStrategy::Merge),
        "the app default answered"
    );

    // The repository outranks it.
    let (origin, work) = pair("cfg-ladder-repo");
    diverge(&origin, &work);
    work.git(&["config", "pull.rebase", "true"]);
    let outcome = pull::pull_with(
        &work.root,
        &PullRequest::default(),
        PullDefault::Merge,
        &untouched(),
    )
    .expect("pull");
    assert_eq!(
        outcome.strategy,
        Some(PullStrategy::Rebase),
        "pull.rebase won"
    );

    // The branch outranks the repository.
    let (origin, work) = pair("cfg-ladder-branch");
    diverge(&origin, &work);
    work.git(&["config", "pull.rebase", "true"]);
    work.git(&["config", "branch.main.rebase", "false"]);
    let outcome = pull::pull_with(
        &work.root,
        &PullRequest::default(),
        PullDefault::Merge,
        &untouched(),
    )
    .expect("pull");
    assert_eq!(
        outcome.strategy,
        Some(PullStrategy::Merge),
        "branch.main.rebase won"
    );
}

#[test]
fn remembering_writes_the_repository_config_and_not_the_global_one() {
    let (origin, work) = pair("remember");
    diverge(&origin, &work);

    pull::pull_with(
        &work.root,
        &PullRequest {
            strategy: Some(PullStrategy::Rebase),
            remember: true,
            ..PullRequest::default()
        },
        PullDefault::Ask,
        &untouched(),
    )
    .expect("pull");

    assert_eq!(
        work.git(&["config", "--local", "--get", "pull.rebase"])
            .trim(),
        "true"
    );
    let (found, _) = work.try_git(&["config", "--global", "--get", "pull.rebase"]);
    assert!(!found, "the user's own ~/.gitconfig is not edited");

    // And the next pull needs no dialog.
    origin.write("more.txt", b"more\n");
    origin.commit_all("more");
    work.write("mine2.txt", b"mine\n");
    work.commit_all("mine again");
    let outcome = pull::pull(&work.root, None, &untouched()).expect("pull");
    assert_eq!(outcome.strategy, Some(PullStrategy::Rebase));
}

#[test]
fn a_refused_pull_does_not_remember_the_answer() {
    let (origin, work) = pair("remember-refused");
    diverge(&origin, &work);
    work.write("shared.txt", b"work in progress\n");
    origin.write("shared.txt", b"theirs\n");
    origin.commit_all("touching the same file");

    let err = pull::pull_with(
        &work.root,
        &PullRequest {
            strategy: Some(PullStrategy::Rebase),
            remember: true,
            ..PullRequest::default()
        },
        PullDefault::Ask,
        &untouched(),
    )
    .expect_err("refused");
    assert!(
        matches!(err, GitError::CheckoutWouldOverwrite { .. }),
        "got {err:?}"
    );

    let (found, _) = work.try_git(&["config", "--local", "--get", "pull.rebase"]);
    assert!(
        !found,
        "an answer that never ran must not pin the user to a strategy"
    );
}

// --- what came down, as a range the log can walk ------------------------------------------

/// `git rev-list <spec>`, as full oids, sorted — the order is git's business, the set is ours.
fn rev_list(repo: &TempRepo, spec: &str) -> Vec<String> {
    let mut oids: Vec<String> = repo
        .git(&["rev-list", spec])
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    oids.sort();
    oids
}

/// What `cide_git::log` lists for `LogRefs::Rev { spec }` over this repository, sorted the same
/// way. This is the walk *View commits* runs, so the test asks the real one and not a revwalk of
/// its own.
fn log_rev(repo: &TempRepo, spec: &str) -> Vec<String> {
    let info = cide_git::repo::discover(std::slice::from_ref(&repo.root))
        .into_iter()
        .next()
        .expect("the temp repo is discoverable");
    let query = LogQuery {
        scope: LogScope::One { repo: info.id },
        refs: LogRefs::Rev { spec: spec.into() },
        cursor: LogCursor::Newest,
        path: None,
        follow: false,
        simplify: Simplify::Default,
        first_parent: false,
        author: None,
        text: None,
        limit: 100,
        scan_limit: 20_000,
        graph: false,
        graph_lanes: 12,
    };
    let page = cide_git::log::log(std::slice::from_ref(&info), &query).expect("a log page");
    let mut oids: Vec<String> = page.commits.iter().map(|c| c.oid.clone()).collect();
    oids.sort();
    oids
}

#[test]
fn received_is_the_range_the_commits_came_from_under_every_strategy() {
    for strategy in [
        PullStrategy::FastForward,
        PullStrategy::Merge,
        PullStrategy::Rebase,
    ] {
        let (origin, work) = pair("received");
        // Three upstream commits, so the range is more than one row; and for the two strategies
        // that integrate, one of ours, so there is something to merge or replay.
        //
        // A minute apart, and after `pair`'s root commit, because this test is about which
        // commits the range names and not about how the walk orders a tie: six commits inside one
        // second once made it emit the root — hidden behind `mine` — before it had popped `mine`
        // and learned so. `docs/journal.md` (M37) records that as the walk's, not this feature's.
        let base = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970")
            .as_secs() as i64
            + 3_600;
        for n in 1..=3 {
            origin.write(&format!("theirs{n}.txt"), b"theirs\n");
            origin.commit_all_at(&format!("theirs {n}"), base + 60 * n);
        }
        if strategy != PullStrategy::FastForward {
            work.write("mine.txt", b"mine\n");
            work.commit_all_at("mine", base + 600);
        }
        let before = head(&work);

        let outcome = pull::pull_with(
            &work.root,
            &request(strategy),
            PullDefault::Ask,
            &untouched(),
        )
        .expect("pull");
        assert_eq!(outcome.strategy, Some(strategy), "{strategy:?}");
        assert_eq!(outcome.advanced, 3, "{strategy:?}");

        let upstream = work.git(&["rev-parse", "origin/main"]).trim().to_string();
        assert_eq!(
            outcome.received,
            format!("{before}..{upstream}"),
            "{strategy:?}: full oids, from the pre-pull tip to the upstream tip"
        );

        let listed = rev_list(&work, &outcome.received);
        assert_eq!(
            listed.len() as u32,
            outcome.advanced,
            "{strategy:?}: the range is what came down and nothing else"
        );
        if strategy != PullStrategy::FastForward {
            // The merge commit, or the replayed `mine`: HEAD is new, and it was not received.
            assert!(
                !listed.contains(&head(&work)),
                "{strategy:?}: `old..new` would have listed HEAD; `received` must not"
            );
        }
        // Under the cap, so the toast's list and the range are the same commits.
        assert_eq!(outcome.commits.len(), listed.len(), "{strategy:?}");
        for commit in &outcome.commits {
            assert!(
                listed.iter().any(|oid| oid.starts_with(&commit.short_oid)),
                "{strategy:?}: {} is in the toast but not in the range",
                commit.short_oid
            );
        }
        assert_eq!(
            log_rev(&work, &outcome.received),
            listed,
            "{strategy:?}: the log walks the range to the same commits"
        );
    }
}

#[test]
fn received_is_empty_when_nothing_came_down() {
    let (_origin, work) = pair("received-empty");
    let pulled = pull::pull(&work.root, None, &untouched()).expect("already up to date");
    assert_eq!(pulled.advanced, 0);
    assert_eq!(
        pulled.received, "",
        "a pull that took nothing names no range"
    );

    let fetched = cide_git::branch::fetch(&work.root, None, &untouched()).expect("fetch");
    assert_eq!(
        fetched.received, "",
        "a fetch moves no branch, so it received nothing"
    );
}
