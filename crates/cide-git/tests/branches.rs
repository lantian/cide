//! The branch selector's backend, against real repositories.
//!
//! The listing is easy to get right and is checked here mostly so a refactor cannot quietly
//! drop the current-branch marker or the upstream counts. **The refusal is what these tests
//! are for.** `cide_git::branch::checkout_blockers` implements git's own rule — a local change
//! blocks a switch only when the file differs between the two branches — and that rule has two
//! failure modes that both look reasonable from the outside:
//!
//! * refuse whenever the tree is dirty, and the selector is unusable in any repository
//!   somebody is working in;
//! * refuse never, and libgit2's `SAFE` checkout fails later with a message the popup never
//!   asked for and cannot turn into a choice.
//!
//! So every branch of that rule has a test with a real working tree behind it, and the two
//! stash modes are driven end to end because "your changes came with you" is a claim about
//! bytes on disk, not about a return value.

mod support;

use cide_git::branch;
use cide_git::repo as repo_mod;
use cide_ipc::git::{CheckoutMode, GitError};
use support::TempRepo;

/// `main` with `shared.txt` and `only-main.txt`, plus a `feature` branch that rewrites
/// `only-main.txt` and adds `only-feature.txt`. Left on `main`.
///
/// The shape is chosen so every clause of the blocker rule has a file to exercise:
/// `shared.txt` is identical on both branches, `only-main.txt` differs, and
/// `only-feature.txt` exists on one side only.
fn two_branches(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("shared.txt", b"shared\n");
    repo.write("only-main.txt", b"main\n");
    repo.commit_all("first");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("only-main.txt", b"feature\n");
    repo.write("only-feature.txt", b"feature only\n");
    repo.commit_all("second");
    repo.git(&["checkout", "-q", "main"]);
    repo
}

fn info(repo: &TempRepo) -> cide_ipc::git::RepoInfo {
    repo_mod::discover(std::slice::from_ref(&repo.root))
        .into_iter()
        .next()
        .expect("the temp repo is discoverable")
}

// --- listing --------------------------------------------------------------------------------

#[test]
fn the_list_marks_the_branch_that_is_checked_out() {
    let repo = two_branches("list-current");
    let list = branch::list(&info(&repo)).expect("list");

    let names: Vec<&str> = list.local.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"main"), "local branches: {names:?}");
    assert!(names.contains(&"feature"), "local branches: {names:?}");
    assert_eq!(list.head.head, "main");

    let current: Vec<&str> = list
        .local
        .iter()
        .filter(|b| b.current)
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(current, vec!["main"], "exactly one row is the current one");
    assert!(list.remote.is_empty(), "no remote is configured");
}

#[test]
fn branches_are_listed_most_recently_committed_first() {
    let repo = two_branches("list-order");
    // `feature`'s tip was committed after `main`'s, so it leads. Recency is the order because
    // a year-old repository has hundreds of branches and the wanted one is nearly always
    // among the handful last touched.
    let list = branch::list(&info(&repo)).expect("list");
    let names: Vec<&str> = list.local.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["feature", "main"]);

    let tip = &list.local[0];
    assert_eq!(tip.tip.len(), 8, "the tip oid is abbreviated");
    assert_eq!(tip.subject, "second", "the row carries its tip's summary");
}

#[test]
fn a_local_branch_reports_its_upstream_and_the_two_counts() {
    let origin = TempRepo::new("upstream-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");

    let work = clone_of(&origin, "upstream-work");
    work.write("a.txt", b"two\n");
    work.commit_all("local only");

    let list = branch::list(&info(&work)).expect("list");
    let main = list
        .local
        .iter()
        .find(|b| b.name == "main")
        .expect("main is listed");
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (1, 0));

    let remote: Vec<&str> = list.remote.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(remote, vec!["origin/main"], "origin/HEAD is not a row");
}

// --- the refusal ------------------------------------------------------------------------------

#[test]
fn a_clean_switch_moves_head_and_the_working_tree() {
    let repo = two_branches("switch-clean");
    assert_eq!(branch::checkout_blockers(&repo.root, "feature"), Ok(vec![]));

    let outcome = branch::checkout(&repo.root, "feature", CheckoutMode::Refuse).expect("checkout");
    assert_eq!(outcome.branch, "feature");
    assert_eq!(outcome.stashed, None);
    assert_eq!(branch::head(&repo.root).expect("head").head, "feature");
    assert_eq!(repo.read("only-main.txt"), b"feature\n");
    assert!(repo.root.join("only-feature.txt").exists());
}

#[test]
fn a_dirty_file_that_differs_between_the_branches_blocks_the_switch_and_is_named() {
    let repo = two_branches("switch-blocked");
    repo.write("only-main.txt", b"work in progress\n");

    assert_eq!(
        branch::checkout_blockers(&repo.root, "feature"),
        Ok(vec!["only-main.txt".to_string()])
    );

    let error = branch::checkout(&repo.root, "feature", CheckoutMode::Refuse).unwrap_err();
    // The paths are the whole point: "checkout failed" is unactionable, and this list is what
    // the popup turns into a choice.
    assert_eq!(
        error,
        GitError::CheckoutWouldOverwrite {
            branch: "feature".to_string(),
            paths: vec!["only-main.txt".to_string()],
        }
    );
    // And it really did not switch.
    assert_eq!(branch::head(&repo.root).expect("head").head, "main");
    assert_eq!(repo.read("only-main.txt"), b"work in progress\n");
}

#[test]
fn a_dirty_file_that_is_identical_on_both_branches_comes_along() {
    let repo = two_branches("switch-carries");
    repo.write("shared.txt", b"edited while on main\n");

    assert_eq!(
        branch::checkout_blockers(&repo.root, "feature"),
        Ok(vec![]),
        "shared.txt is byte-identical on both branches, so it is not in the way"
    );
    branch::checkout(&repo.root, "feature", CheckoutMode::Refuse).expect("checkout");
    assert_eq!(branch::head(&repo.root).expect("head").head, "feature");
    assert_eq!(
        repo.read("shared.txt"),
        b"edited while on main\n",
        "the edit came with the switch, exactly as `git switch` brings it"
    );
}

#[test]
fn an_untracked_file_the_target_branch_contains_blocks_the_switch() {
    let repo = two_branches("switch-untracked");
    // `feature` has this file committed; here it is untracked. Writing over it is the one
    // case that is genuinely unrecoverable — the content is in no object database.
    repo.write("only-feature.txt", b"mine, not git's\n");

    assert_eq!(
        branch::checkout_blockers(&repo.root, "feature"),
        Ok(vec!["only-feature.txt".to_string()])
    );
    assert!(matches!(
        branch::checkout(&repo.root, "feature", CheckoutMode::Refuse),
        Err(GitError::CheckoutWouldOverwrite { .. })
    ));
    assert_eq!(repo.read("only-feature.txt"), b"mine, not git's\n");
}

#[test]
fn an_untracked_file_the_target_branch_does_not_have_is_not_in_the_way() {
    let repo = two_branches("switch-untracked-ok");
    repo.write("scratch.md", b"notes\n");

    assert_eq!(branch::checkout_blockers(&repo.root, "feature"), Ok(vec![]));
    branch::checkout(&repo.root, "feature", CheckoutMode::Refuse).expect("checkout");
    assert_eq!(repo.read("scratch.md"), b"notes\n");
}

#[test]
fn a_merge_in_progress_refuses_before_anything_is_computed() {
    let repo = two_branches("switch-mid-merge");
    // Conflict `main` and `feature` on `only-main.txt`, so the merge stops half done.
    repo.write("only-main.txt", b"main, differently\n");
    repo.commit_all("diverge");
    let (ok, _) = repo.try_git(&["merge", "feature"]);
    assert!(!ok, "the merge is supposed to conflict");

    assert!(matches!(
        branch::checkout(&repo.root, "feature", CheckoutMode::Refuse),
        Err(GitError::OperationInProgress { .. })
    ));
}

// --- the stash modes --------------------------------------------------------------------------

#[test]
fn stash_switches_and_leaves_the_work_in_the_stash() {
    let repo = two_branches("switch-stash");
    repo.write("only-main.txt", b"work in progress\n");

    let outcome = branch::checkout(&repo.root, "feature", CheckoutMode::Stash).expect("checkout");
    assert_eq!(branch::head(&repo.root).expect("head").head, "feature");
    assert_eq!(
        outcome.stashed.as_deref(),
        Some("cide: switching to feature")
    );
    assert_eq!(outcome.restore_failed, None);
    // The switch happened, so the file is the target branch's.
    assert_eq!(repo.read("only-main.txt"), b"feature\n");
    // And the work is recoverable by a `git` the user runs themselves.
    assert_eq!(
        cide_git::stash::list(&repo.root).expect("stash list").len(),
        1
    );
}

/// `main` and `feature` differ in the **first** line of `divergent.txt` and nowhere else, so a
/// working-tree edit to the last line blocks the switch and then merges into it cleanly.
fn divergent(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("divergent.txt", b"top\nmiddle\nbottom\n");
    repo.commit_all("first");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("divergent.txt", b"TOP\nmiddle\nbottom\n");
    repo.commit_all("second");
    repo.git(&["checkout", "-q", "main"]);
    repo
}

#[test]
fn stash_and_restore_carries_the_changes_onto_the_new_branch() {
    let repo = divergent("switch-smart");
    // The edit has to be to the file that differs — an edit to anything else would not block
    // and there would be nothing to be smart about.
    repo.write("divergent.txt", b"top\nmiddle\nBOTTOM\n");
    assert_eq!(
        branch::checkout_blockers(&repo.root, "feature"),
        Ok(vec!["divergent.txt".to_string()])
    );

    let outcome =
        branch::checkout(&repo.root, "feature", CheckoutMode::StashAndRestore).expect("checkout");
    assert_eq!(branch::head(&repo.root).expect("head").head, "feature");
    assert_eq!(outcome.restore_failed, None);
    assert_eq!(
        outcome.stashed, None,
        "the entry was applied and dropped, so there is no stash to tell the user about"
    );
    assert_eq!(
        repo.read("divergent.txt"),
        b"TOP\nmiddle\nBOTTOM\n",
        "the branch's change and the working-tree change are both there"
    );
    assert_eq!(
        cide_git::stash::list(&repo.root).expect("stash list").len(),
        0
    );
}

#[test]
fn a_restore_that_conflicts_says_so_and_keeps_the_stash() {
    let repo = divergent("switch-smart-conflict");
    // Now the edit is to the *same* line the branches disagree about, so the three-way merge
    // has no clean answer. libgit2's own `stash_pop` would drop the entry here — it treats an
    // apply that wrote conflict markers as success — which is why this path uses apply+drop.
    repo.write("divergent.txt", b"mine\nmiddle\nbottom\n");

    let outcome =
        branch::checkout(&repo.root, "feature", CheckoutMode::StashAndRestore).expect("checkout");
    assert_eq!(branch::head(&repo.root).expect("head").head, "feature");
    let reason = outcome.restore_failed.expect("the conflict is reported");
    assert!(
        reason.contains("divergent.txt"),
        "the reason names the file: {reason}"
    );
    assert_eq!(
        cide_git::stash::list(&repo.root).expect("stash list").len(),
        1,
        "the work is still recoverable"
    );
    assert_eq!(
        outcome.stashed.as_deref(),
        Some("cide: switching to feature")
    );
}

// --- remote branches --------------------------------------------------------------------------

/// A second working repository with `origin` pointing at `from`, on a tracking `main`.
///
/// A path remote (not `file://`, not `https://`) is what keeps `push::route` on the libgit2
/// side, so these tests never fork a `git` that could block on a credential prompt.
fn clone_of(from: &TempRepo, tag: &str) -> TempRepo {
    let work = TempRepo::new(tag);
    work.git(&[
        "remote",
        "add",
        "origin",
        from.root.to_str().expect("utf-8 path"),
    ]);
    work.git(&["fetch", "-q", "origin"]);
    work.git(&["checkout", "-q", "-b", "main", "origin/main"]);
    work
}

#[test]
fn checking_out_a_remote_branch_creates_a_local_one_that_tracks_it() {
    let origin = TempRepo::new("remote-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    origin.git(&["checkout", "-q", "-b", "shipped"]);
    origin.write("a.txt", b"two\n");
    origin.commit_all("on shipped");
    origin.git(&["checkout", "-q", "main"]);

    let work = clone_of(&origin, "remote-work");
    let outcome =
        branch::checkout(&work.root, "origin/shipped", CheckoutMode::Refuse).expect("checkout");

    assert_eq!(outcome.branch, "shipped");
    assert_eq!(
        outcome.created_from_remote.as_deref(),
        Some("origin/shipped")
    );

    let list = branch::list(&info(&work)).expect("list");
    let created = list
        .local
        .iter()
        .find(|b| b.name == "shipped")
        .expect("the local branch exists");
    assert!(created.current);
    assert_eq!(
        created.upstream.as_deref(),
        Some("origin/shipped"),
        "without the upstream the next push would need --set-upstream and the row would show no counts"
    );
    assert_eq!(work.read("a.txt"), b"two\n");
}

// --- create, rename, delete -------------------------------------------------------------------

#[test]
fn a_name_git_would_reject_never_reaches_libgit2() {
    let repo = two_branches("create-invalid");
    assert_eq!(
        branch::create(&repo.root, "my branch", None),
        Err(GitError::InvalidBranchName {
            name: "my branch".to_string()
        })
    );
}

#[test]
fn creating_over_an_existing_branch_is_refused_rather_than_moving_it() {
    let repo = two_branches("create-duplicate");
    assert_eq!(
        branch::create(&repo.root, "feature", None),
        Err(GitError::BranchExists {
            name: "feature".to_string()
        })
    );
    // And `feature` still points where it did.
    let list = branch::list(&info(&repo)).expect("list");
    let feature = list
        .local
        .iter()
        .find(|b| b.name == "feature")
        .expect("feature");
    assert_eq!(feature.subject, "second");
}

#[test]
fn a_branch_can_be_created_from_another_branch_rather_than_from_head() {
    let repo = two_branches("create-from");
    branch::create(&repo.root, "hotfix", Some("feature")).expect("create");
    branch::checkout(&repo.root, "hotfix", CheckoutMode::Refuse).expect("checkout");
    assert_eq!(
        repo.read("only-main.txt"),
        b"feature\n",
        "hotfix starts at feature's tip, not main's"
    );
}

#[test]
fn renaming_a_branch_keeps_its_tip_and_refuses_a_name_in_use() {
    let repo = two_branches("rename");
    branch::rename(&repo.root, "feature", "feature/login").expect("rename");
    let list = branch::list(&info(&repo)).expect("list");
    let names: Vec<&str> = list.local.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["feature/login", "main"]);

    assert_eq!(
        branch::rename(&repo.root, "feature/login", "main"),
        Err(GitError::BranchExists {
            name: "main".to_string()
        })
    );
    assert_eq!(
        branch::rename(&repo.root, "gone", "whatever"),
        Err(GitError::NoSuchBranch {
            name: "gone".to_string()
        })
    );
}

#[test]
fn deleting_the_branch_you_are_on_is_refused() {
    let repo = two_branches("delete-current");
    assert_eq!(
        branch::delete(&repo.root, "main", false),
        Err(GitError::BranchIsCurrent {
            name: "main".to_string()
        })
    );
}

#[test]
fn deleting_an_unmerged_branch_needs_force_and_a_merged_one_does_not() {
    let repo = two_branches("delete-unmerged");
    // `feature` has a commit that `main` does not, so deleting it loses commits.
    assert_eq!(
        branch::delete(&repo.root, "feature", false),
        Err(GitError::BranchNotMerged {
            name: "feature".to_string()
        })
    );
    branch::delete(&repo.root, "feature", true).expect("forced delete");

    // A branch pointing at HEAD is merged by definition and goes without a force.
    branch::create(&repo.root, "scratch", None).expect("create");
    branch::delete(&repo.root, "scratch", false).expect("delete a merged branch");
    let list = branch::list(&info(&repo)).expect("list");
    assert_eq!(list.local.len(), 1);
}

/// `BranchNotMerged` means "HEAD cannot reach it", **not** "these commits are on no other
/// branch" — and the difference is a sentence under a red button.
///
/// `is_merged` is `git branch -d`'s test on purpose (see its doc comment), so a branch already
/// merged into `release` refuses while you are standing on `main`, with nothing whatsoever at
/// stake. This test exists so nobody re-derives the stronger claim from the variant's name and
/// writes it into a confirmation again: `explain` in `ui/src/chrome/branchModel.ts` and the
/// popup's delete panel both used to say "has commits that are on no other branch. Deleting it
/// loses them.", which this arrangement disproves.
#[test]
fn a_branch_merged_into_some_other_branch_is_still_refused_from_here() {
    let repo = TempRepo::new("delete-merged-elsewhere");
    repo.write("a.txt", b"one\n");
    repo.commit_all("first");
    repo.git(&["checkout", "-q", "-b", "release"]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("b.txt", b"work\n");
    repo.commit_all("the work");
    repo.git(&["checkout", "-q", "release"]);
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "feature"]);
    repo.git(&["checkout", "-q", "main"]);

    // Every commit on `feature` is reachable from `release`. Deleting the name loses nothing,
    // and cide still refuses — which is fine, and is exactly why the message may not claim it.
    assert_eq!(
        branch::delete(&repo.root, "feature", false),
        Err(GitError::BranchNotMerged {
            name: "feature".to_string()
        })
    );
    assert_eq!(
        GitError::BranchNotMerged {
            name: "feature".to_string()
        }
        .to_string(),
        "feature is not fully merged into the current branch",
        "the wording is the claim; it has to stay the one that was measured"
    );
}

// --- fetch and pull ---------------------------------------------------------------------------

#[test]
fn a_fast_forward_pull_advances_head_and_says_by_how_much() {
    let origin = TempRepo::new("pull-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "pull-work");

    origin.write("a.txt", b"two\n");
    origin.commit_all("second");

    let outcome = branch::pull(&work.root, None).expect("pull");
    assert_eq!(outcome.remote, "origin");
    assert!(
        !outcome.shelled_out,
        "a path remote needs no credential helper"
    );
    assert_eq!(outcome.advanced, 1);
    assert_eq!(work.read("a.txt"), b"two\n");

    // Running it again has nothing to take and says so with zero rather than an error.
    let again = branch::pull(&work.root, None).expect("second pull");
    assert_eq!(again.advanced, 0);
}

#[test]
fn a_divergent_pull_reports_both_counts_instead_of_merging() {
    let origin = TempRepo::new("pull-diverge-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "pull-diverge-work");

    origin.write("b.txt", b"theirs\n");
    origin.commit_all("theirs");
    work.write("c.txt", b"mine\n");
    work.commit_all("mine");

    // cide has no conflict-resolution surface, so it refuses with the numbers a user needs to
    // choose between merge and rebase rather than dropping them into a conflicted tree.
    assert_eq!(
        branch::pull(&work.root, None),
        Err(GitError::NotFastForward {
            branch: "main".to_string(),
            ahead: 1,
            behind: 1,
        })
    );
    assert!(!work.root.join("b.txt").exists(), "nothing was merged in");
}

#[test]
fn a_pull_refuses_rather_than_overwriting_a_local_change() {
    let origin = TempRepo::new("pull-dirty-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "pull-dirty-work");

    origin.write("a.txt", b"two\n");
    origin.commit_all("second");
    work.write("a.txt", b"work in progress\n");

    assert_eq!(
        branch::pull(&work.root, None),
        Err(GitError::CheckoutWouldOverwrite {
            branch: "main".to_string(),
            paths: vec!["a.txt".to_string()],
        })
    );
    assert_eq!(work.read("a.txt"), b"work in progress\n");
}

#[test]
fn a_branch_with_no_upstream_cannot_be_pulled() {
    let origin = TempRepo::new("pull-no-upstream-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "pull-no-upstream-work");
    work.git(&["checkout", "-q", "-b", "solo"]);

    assert_eq!(
        branch::pull(&work.root, None),
        Err(GitError::NoUpstream {
            branch: "solo".to_string()
        })
    );
}

#[test]
fn a_fetch_updates_the_remote_row_without_moving_the_working_tree() {
    let origin = TempRepo::new("fetch-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "fetch-work");

    origin.write("a.txt", b"two\n");
    origin.commit_all("second");

    let outcome = branch::fetch(&work.root, None).expect("fetch");
    assert_eq!(outcome.advanced, 0, "a fetch never moves HEAD");
    assert_eq!(work.read("a.txt"), b"one\n");

    let list = branch::list(&info(&work)).expect("list");
    let main = list.local.iter().find(|b| b.name == "main").expect("main");
    assert_eq!(
        (main.ahead, main.behind),
        (0, 1),
        "the row now knows it is a commit behind"
    );
}

/// `output` is what the popup prints, verbatim, as the single line a fetch earns. So it has to
/// be a sentence.
///
/// It was libgit2's sideband stream, and this test is the one that would have caught it: a
/// successful fetch of three objects handed the UI `"Counting objects 1\rCounting objects
/// 3\r\nCompressing objects: 0% (0/3)\rCompressing objects: 100% (3/3), done\n"`, carriage
/// returns and all. Both halves are pinned, because the empty case is load-bearing too —
/// `fetchNote` reads an empty `output` as "already up to date", so a fetch that really brought
/// something down must not produce one.
#[test]
fn a_fetch_reports_a_sentence_and_not_a_progress_meter() {
    let origin = TempRepo::new("fetch-says-origin");
    origin.write("a.txt", b"one\n");
    origin.commit_all("first");
    let work = clone_of(&origin, "fetch-says-work");

    // Nothing new: silence, which is what makes the UI say "already up to date".
    let quiet = branch::fetch(&work.root, None).expect("fetch");
    assert!(
        !quiet.shelled_out,
        "a path remote stays on the libgit2 side"
    );
    assert_eq!(quiet.output, "", "an up-to-date fetch has nothing to say");

    origin.write("a.txt", b"two\n");
    origin.commit_all("second");

    let outcome = branch::fetch(&work.root, None).expect("fetch");
    assert!(
        !outcome.output.is_empty(),
        "a fetch that brought commits must not read as 'already up to date'"
    );
    assert!(
        !outcome.output.contains('\r') && !outcome.output.contains('\n'),
        "the note bar is one line: {:?}",
        outcome.output
    );
    assert!(
        !outcome.output.contains("Counting objects")
            && !outcome.output.contains("Compressing objects"),
        "sideband progress is not a message: {:?}",
        outcome.output
    );
    assert!(
        outcome.output.contains("origin"),
        "it names the remote it talked to: {:?}",
        outcome.output
    );
}
