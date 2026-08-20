//! The commit log walk, against real repositories, with the real `git` binary as the oracle.
//!
//! `cide_git::log` reimplements `rev-list` ordering, because `git2::Revwalk` cannot stream (see
//! that module's header for the `revwalk.c` line numbers). Reimplementing git is exactly the kind
//! of thing that is *nearly* right for years, so almost every claim here about git semantics is
//! asserted against `git log` or `git rev-list` rather than against a second model of them —
//! the discipline `tests/patch_props.rs` sets for patch synthesis.
//!
//! # Two things about the fixtures
//!
//! **Every commit gets its own second.** `git log` breaks a committer-time tie by its own
//! insertion order and our heap breaks it by oid; a differential test over commits made inside
//! one second would be comparing two tie-break conventions, not two walks. The clock is part of
//! the fixture, not decoration.
//!
//! **Histories are built with `git fast-import`.** One process instead of one per commit, which
//! is what makes a 250-commit paging test and a 4000-file commit affordable, and it gives exact
//! control over parents, trees and timestamps — a two-parent merge whose tree is *neither*
//! parent's (an evil merge) cannot be produced reliably by driving `git merge` and resolving a
//! conflict by hand. It is still the real `git` binary writing real objects.

mod fastimport;
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cide_git::log;
use cide_git::log::LogWalk;
use cide_git::repo as repo_mod;
use cide_ipc::git::{GitError, RepoInfo};
use cide_ipc::history::{
    CommitPage, GraphOff, LogCursor, LogGraph, LogQuery, LogRefs, LogResume, LogScope, LogStop,
    RefKind, Simplify,
};
use fastimport::{Import, Op};
use support::TempRepo;

fn info(repo: &TempRepo) -> RepoInfo {
    repo_mod::discover(std::slice::from_ref(&repo.root))
        .into_iter()
        .next()
        .expect("the temp repo is discoverable")
}

/// A plain query over one repository: HEAD, newest first, no filters, no graph.
fn query(info: &RepoInfo) -> LogQuery {
    LogQuery {
        scope: LogScope::One { repo: info.id },
        refs: LogRefs::Head,
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
    }
}

fn walk(info: &RepoInfo, query: &LogQuery) -> CommitPage {
    log::log(std::slice::from_ref(info), query).expect("a log page")
}

fn oids(page: &CommitPage) -> Vec<String> {
    page.commits.iter().map(|c| c.oid.clone()).collect()
}

/// What `git` itself says, as a list of full oids.
fn git_oids(repo: &TempRepo, args: &[&str]) -> Vec<String> {
    repo.git(args)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A linear history of `count` commits, oldest first, each touching `file.txt`. The **root** also
/// creates `needle.txt` and nothing else ever touches it, which is the shape a path filter has to
/// walk the whole repository to answer.
fn linear(repo: &TempRepo, count: usize) {
    let mut import = Import::new();
    let mut previous = None;
    for index in 0..count {
        let body = format!("line {index}");
        let mut ops = vec![Op::Set("100644", "file.txt", &body)];
        if index == 0 {
            ops.push(Op::Set("100644", "needle.txt", "needle"));
        }
        previous = Some(import.commit(
            "main",
            index as i64 * 60,
            &format!("commit {index}"),
            previous,
            None,
            &ops,
        ));
    }
    import.run(repo);
}

// --- the baseline -------------------------------------------------------------------------------

#[test]
fn a_plain_walk_is_git_log() {
    let repo = TempRepo::new("log-baseline");
    linear(&repo, 40);
    let info = info(&repo);

    let mut request = query(&info);
    request.limit = 1_000;
    let page = walk(&info, &request);

    assert_eq!(oids(&page), git_oids(&repo, &["log", "--format=%H"]));
    assert_eq!(page.stop, LogStop::Exhausted, "the whole history fitted");
    assert!(page.resume.is_none(), "nothing left to continue from");
    assert_eq!(page.scanned, 40);
}

#[test]
fn the_root_commit_is_listed_and_has_no_parents() {
    let repo = TempRepo::new("log-root");
    linear(&repo, 3);
    let info = info(&repo);
    let page = walk(&info, &query(&info));

    let last = page.commits.last().expect("three rows");
    assert!(last.parents.is_empty(), "the root has no parents: {last:?}");
    assert_eq!(last.summary, "commit 0");
    assert_eq!(page.stop, LogStop::Exhausted);
}

#[test]
fn an_unborn_head_is_an_empty_page_and_not_an_error() {
    // `TempRepo::new` inits and configures and commits nothing, which is exactly the state a
    // brand-new project is in when someone opens the log tab. A dialog here would be the first
    // thing they saw.
    let repo = TempRepo::new("log-unborn");
    let info = info(&repo);
    let page = walk(&info, &query(&info));

    assert!(page.commits.is_empty());
    assert_eq!(page.stop, LogStop::Exhausted);
    assert!(page.resume.is_none());
}

// --- paging ---------------------------------------------------------------------------------------

#[test]
fn one_big_page_and_five_small_ones_are_the_same_list() {
    let repo = TempRepo::new("log-paging");
    linear(&repo, 250);
    let info = info(&repo);

    let mut whole = query(&info);
    whole.limit = 250;
    let all = walk(&info, &whole);
    assert_eq!(all.commits.len(), 250);

    let mut chained: Vec<String> = Vec::new();
    let mut request = query(&info);
    request.limit = 50;
    let mut token: Option<LogResume> = None;
    for round in 0..5 {
        request.cursor = match token.take() {
            Some(token) => LogCursor::Resume { token },
            None => LogCursor::Newest,
        };
        let page = walk(&info, &request);
        assert_eq!(page.commits.len(), 50, "round {round}");
        chained.extend(oids(&page));
        token = page.resume;
    }

    assert_eq!(chained, oids(&all), "five pages concatenate to one");
    let mut sorted = chained.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), chained.len(), "no oid appears twice");
    assert!(token.is_none(), "the fifth page exhausted the history");
}

#[test]
fn a_token_from_a_different_query_is_refused() {
    let repo = TempRepo::new("log-foreign-token");
    linear(&repo, 20);
    let info = info(&repo);

    let mut first = query(&info);
    first.limit = 5;
    let token = walk(&info, &first).resume.expect("more to come");

    // Same repository, same refs, different *filter*. The frontier was built without a path and
    // the budget accounted without one; using it here would produce a page that is neither query's
    // answer, and would do it silently.
    let mut other = query(&info);
    other.limit = 5;
    other.path = Some("file.txt".into());
    other.cursor = LogCursor::Resume {
        token: token.clone(),
    };
    let refused = log::log(std::slice::from_ref(&info), &other);
    assert!(
        matches!(refused, Err(GitError::StaleLogCursor)),
        "a foreign token is an error, not a page: {refused:?}"
    );

    // A version this build does not know is the same refusal, and it has to be *reachable* —
    // which is why `LogResume` has no `deny_unknown_fields`.
    let mut aged = query(&info);
    aged.cursor = LogCursor::Resume {
        token: LogResume {
            version: token.version + 99,
            ..token
        },
    };
    assert!(matches!(
        log::log(std::slice::from_ref(&info), &aged),
        Err(GitError::StaleLogCursor)
    ));
}

#[test]
fn a_frontier_that_no_longer_resolves_is_cursor_lost() {
    let repo = TempRepo::new("log-cursor-lost");
    linear(&repo, 20);
    let info = info(&repo);

    let mut first = query(&info);
    first.limit = 5;
    let mut token = walk(&info, &first).resume.expect("more to come");
    // A rewrite, a prune, a `gc`: the frontier names commits the repository no longer has. Faked
    // by hand because reproducing it with a real `gc` needs the reflog expired first, and the
    // *shape* is what the walk has to survive.
    for entry in &mut token.repos[0].frontier {
        entry.oid = "0".repeat(40);
    }

    let mut second = query(&info);
    second.limit = 5;
    second.cursor = LogCursor::Resume { token };
    let page = walk(&info, &second);

    assert_eq!(page.stop, LogStop::CursorLost);
    assert!(page.commits.is_empty());
    assert!(
        page.resume.is_none(),
        "no continuation, or the caller loops for ever on the same dead token"
    );
}

#[test]
fn a_cursor_at_a_commit_starts_there_and_after_it_starts_below() {
    let repo = TempRepo::new("log-seek");
    linear(&repo, 30);
    let info = info(&repo);

    let mut request = query(&info);
    request.limit = 30;
    let all = oids(&walk(&info, &request));
    let anchor = all[10].clone();

    let mut at = query(&info);
    at.limit = 5;
    at.cursor = LogCursor::At {
        oid: anchor.clone(),
    };
    assert_eq!(oids(&walk(&info, &at)), all[10..15]);

    let mut after = query(&info);
    after.limit = 5;
    after.cursor = LogCursor::After { oid: anchor };
    assert_eq!(oids(&walk(&info, &after)), all[11..16]);
}

#[test]
fn an_anchor_that_is_not_reachable_is_cursor_lost() {
    let repo = TempRepo::new("log-seek-lost");
    linear(&repo, 10);
    let info = info(&repo);

    let mut request = query(&info);
    request.cursor = LogCursor::At {
        oid: "0".repeat(40),
    };
    let page = walk(&info, &request);
    assert_eq!(page.stop, LogStop::CursorLost);
    assert!(page.commits.is_empty());
}

// --- the budget -------------------------------------------------------------------------------------

#[test]
fn the_budget_stops_short_says_how_far_it_got_and_resumes_to_the_match() {
    // The acceptance test for the honesty requirement. `needle.txt` is touched by the *root* of a
    // 300-commit history and by nothing else, so a small budget cannot reach it — and the page
    // that comes back must say `Budget`, not `Exhausted`. Reporting `Exhausted` here tells the
    // user their file has one revision when it has one they cannot see.
    let repo = TempRepo::new("log-budget");
    linear(&repo, 300);
    let info = info(&repo);

    let mut first = query(&info);
    first.path = Some("needle.txt".into());
    first.limit = 10;
    first.scan_limit = 50;
    let page = walk(&info, &first);

    assert!(page.commits.is_empty(), "the match is 300 commits down");
    assert_eq!(page.stop, LogStop::Budget);
    assert_eq!(page.scanned, 50, "and it says exactly how far it got");
    assert_eq!(page.repos[0].stop, LogStop::Budget);
    let token = page.resume.expect("a budget stop always leaves a frontier");
    assert_eq!(
        token.repos[0].scanned, 50,
        "the budget is cumulative across pages, or it bounds nothing"
    );

    // "Keep looking" — the same query, the returned token, a wider budget. The budgets are
    // deliberately *not* part of the token's identity, precisely so this call is legal.
    let mut second = query(&info);
    second.path = Some("needle.txt".into());
    second.limit = 10;
    second.cursor = LogCursor::Resume { token };
    let page = walk(&info, &second);

    assert_eq!(page.commits.len(), 1, "the root created it");
    assert_eq!(page.commits[0].summary, "commit 0");
    assert_eq!(page.stop, LogStop::Exhausted);
}

// --- ref chips ---------------------------------------------------------------------------------------

#[test]
fn ref_chips_peel_an_annotated_tag_and_leave_origin_head_out() {
    let repo = TempRepo::new("log-chips");
    linear(&repo, 3);
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

    repo.git(&["tag", "-a", "v1.0", "-m", "release", &head]);
    repo.git(&["tag", "light", &head]);
    repo.git(&["branch", "side", &head]);
    repo.git(&["update-ref", "refs/remotes/origin/main", &head]);
    // The one every ref lister has to exclude: a symbolic ref naming another row in the same list.
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);

    let handle = repo_mod::open(&repo.root).expect("open");
    let chips = log::ref_chips(&handle).expect("chips");
    let oid = git2::Oid::from_str(&head).unwrap();
    let names: Vec<&str> = chips[&oid].iter().map(|c| c.name.as_str()).collect();

    assert!(
        names.contains(&"v1.0"),
        "an annotated tag's ref points at a *tag object*; without `peel_to_commit` the chip is \
         keyed by the tag's own oid and never appears on any row: {names:?}"
    );
    assert!(names.contains(&"light"), "{names:?}");
    assert!(
        names.contains(&"main") && names.contains(&"side"),
        "{names:?}"
    );
    assert!(names.contains(&"origin/main"), "{names:?}");
    assert!(
        !names.iter().any(|n| n.ends_with("/HEAD")),
        "origin/HEAD is a second name for a branch already chipped: {names:?}"
    );

    let current: Vec<&str> = chips[&oid]
        .iter()
        .filter(|c| c.current)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(current, vec!["main"], "exactly the branch HEAD is on");
    assert!(
        !chips[&oid].iter().any(|c| c.kind == RefKind::Head),
        "an attached HEAD gets no chip of its own — the branch chip beside it already says so"
    );

    // And the rows carry them.
    let info = info(&repo);
    let page = walk(&info, &query(&info));
    assert!(page.commits[0].refs.iter().any(|c| c.name == "v1.0"));
}

#[test]
fn a_detached_head_gets_its_own_chip() {
    let repo = TempRepo::new("log-chips-detached");
    linear(&repo, 3);
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    repo.git(&["checkout", "-q", "--detach", &head]);

    let handle = repo_mod::open(&repo.root).expect("open");
    let chips = log::ref_chips(&handle).expect("chips");
    let oid = git2::Oid::from_str(&head).unwrap();
    assert!(
        chips[&oid]
            .iter()
            .any(|c| c.kind == RefKind::Head && c.current),
        "{:?}",
        chips[&oid]
    );
}

// --- the shallow floor -----------------------------------------------------------------------------------

#[test]
fn a_shallow_clone_stops_at_the_graft_and_does_not_call_it_the_end() {
    let repo = TempRepo::new("log-shallow-origin");
    linear(&repo, 20);

    let clone_root = repo
        .root
        .parent()
        .unwrap()
        .join(format!("cide-git-shallow-clone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&clone_root);
    let url = format!("file://{}", repo.root.display());
    let out = std::process::Command::new("git")
        .args(["clone", "--depth", "3", "-q", &url])
        .arg(&clone_root)
        .output()
        .expect("git clone");
    assert!(
        out.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        clone_root.join(".git/shallow").is_file(),
        "the fixture is only meaningful if the clone really is shallow"
    );

    let info = repo_mod::discover(std::slice::from_ref(&clone_root))
        .into_iter()
        .next()
        .expect("the clone is a repository");
    let page = walk(&info, &query(&info));

    assert_eq!(page.commits.len(), 3);
    assert_eq!(
        page.stop,
        LogStop::Shallow,
        "the commits below the graft are not in this repository and never were; `Exhausted` \
         would be a false statement about the project"
    );

    let _ = std::fs::remove_dir_all(&clone_root);
}

// --- clock skew ---------------------------------------------------------------------------------------------

#[test]
fn a_child_older_than_its_parent_still_terminates_and_stays_in_child_order() {
    // The clamped key exists for this: a parent queued with its own raw time would sort *above*
    // rows already emitted, and the page would repeat them for ever. `min(parent, child)` cannot
    // rise, so the popped sequence is non-increasing whatever the clocks say.
    let mut import = Import::new();
    let a = import.commit(
        "main",
        5_000,
        "a, honest clock",
        None,
        None,
        &[Op::Set("100644", "f", "1")],
    );
    let b = import.commit(
        "main",
        1_000,
        "b, clock is behind",
        Some(a),
        None,
        &[Op::Set("100644", "f", "2")],
    );
    let c = import.commit(
        "main",
        9_000,
        "c, back to normal",
        Some(b),
        None,
        &[Op::Set("100644", "f", "3")],
    );
    let _ = c;

    let repo = TempRepo::new("log-skew");
    import.run(&repo);
    let info = info(&repo);
    let page = walk(&info, &query(&info));

    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        vec!["c, back to normal", "b, clock is behind", "a, honest clock"],
        "a child is always emitted before its parent, whatever the stamps say"
    );
    assert_eq!(page.stop, LogStop::Exhausted);
}

// --- path filters -------------------------------------------------------------------------------------------

/// A history whose only interesting property is what happens to `f`: created, changed, deleted,
/// re-added, chmod'ed. Every assertion below is against `git log -- f`.
fn path_history(repo: &TempRepo) {
    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "create f",
        None,
        None,
        &[Op::Set("100644", "f", "one")],
    );
    let b = import.commit(
        "main",
        60,
        "unrelated",
        Some(a),
        None,
        &[Op::Set("100644", "g", "g")],
    );
    let c = import.commit(
        "main",
        120,
        "change f",
        Some(b),
        None,
        &[Op::Set("100644", "f", "two")],
    );
    let d = import.commit("main", 180, "delete f", Some(c), None, &[Op::Delete("f")]);
    let e = import.commit(
        "main",
        240,
        "re-add f",
        Some(d),
        None,
        &[Op::Set("100644", "f", "three")],
    );
    // Same blob, different mode. A `(blob, mode)` comparison catches it; a blob-only comparison
    // silently does not, and "when did this become executable" becomes unanswerable.
    let g = import.commit(
        "main",
        300,
        "chmod +x f",
        Some(e),
        None,
        &[Op::Set("100755", "f", "three")],
    );
    import.commit(
        "main",
        360,
        "unrelated again",
        Some(g),
        None,
        &[Op::Set("100644", "g", "gg")],
    );
    import.run(repo);
}

#[test]
fn a_path_filter_matches_git_log_including_delete_readd_and_a_bare_chmod() {
    let repo = TempRepo::new("log-path");
    path_history(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.path = Some("f".into());
    let page = walk(&info, &request);

    assert_eq!(
        oids(&page),
        git_oids(&repo, &["log", "--format=%H", "--", "f"])
    );
    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        vec!["chmod +x f", "re-add f", "delete f", "change f", "create f"],
        "the mode-only commit is in the list: {summaries:?}"
    );
}

#[test]
fn a_directory_has_a_history() {
    // A directory resolves to its subtree oid, so this needs no special case in the walk — which
    // is the whole reason the comparison is on tree *entries* rather than on a diff.
    let repo = TempRepo::new("log-dir");
    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "outside",
        None,
        None,
        &[Op::Set("100644", "top", "1")],
    );
    let b = import.commit(
        "main",
        60,
        "inside one",
        Some(a),
        None,
        &[Op::Set("100644", "dir/a", "1")],
    );
    let c = import.commit(
        "main",
        120,
        "outside again",
        Some(b),
        None,
        &[Op::Set("100644", "top", "2")],
    );
    import.commit(
        "main",
        180,
        "inside two",
        Some(c),
        None,
        &[Op::Set("100644", "dir/b", "1")],
    );
    import.run(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.path = Some("dir".into());
    assert_eq!(
        oids(&walk(&info, &request)),
        git_oids(&repo, &["log", "--format=%H", "--", "dir"])
    );
}

/// `base` → two branches that each change `f`, joined twice: an ordinary merge whose `f` comes
/// straight from one side, and an **evil** merge whose `f` matches neither.
fn merge_history(repo: &TempRepo) {
    let mut import = Import::new();
    let base = import.commit(
        "main",
        0,
        "base",
        None,
        None,
        &[
            Op::Set("100644", "f", "base"),
            Op::Set("100644", "g", "base"),
        ],
    );
    let left = import.commit(
        "main",
        60,
        "left changes f",
        Some(base),
        None,
        &[Op::Set("100644", "f", "left")],
    );
    let right = import.commit(
        "side",
        120,
        "right changes g",
        Some(base),
        None,
        &[Op::Set("100644", "g", "right")],
    );
    // Tree comes from the first parent (`left`), so `f` is left's and `g` is set to right's:
    // TREESAME to `left` for `f`, and therefore not a commit that changed `f`.
    let plain = import.commit(
        "main",
        180,
        "ordinary merge",
        Some(left),
        Some(right),
        &[Op::Set("100644", "g", "right")],
    );
    let other = import.commit(
        "side",
        240,
        "right changes f too",
        Some(right),
        None,
        &[Op::Set("100644", "f", "right-f")],
    );
    // `f` is set to a third value — a change that exists on neither side. This is the merge a
    // file's history *must* show, and the "differs from every parent" rule is what shows it.
    import.commit(
        "main",
        300,
        "evil merge",
        Some(plain),
        Some(other),
        &[Op::Set("100644", "f", "evil")],
    );
    import.run(repo);
}

#[test]
fn a_merge_is_listed_only_when_it_differs_from_every_parent() {
    let repo = TempRepo::new("log-merge-path");
    merge_history(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.path = Some("f".into());
    let page = walk(&info, &request);
    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();

    assert!(
        summaries.contains(&"evil merge"),
        "an evil merge — a change to `f` that exists on neither side — is the one merge that has \
         to appear: {summaries:?}"
    );
    assert!(
        !summaries.contains(&"ordinary merge"),
        "a merge TREESAME to a parent brought the file along and did nothing to it: {summaries:?}"
    );
    assert_eq!(
        oids(&page),
        git_oids(&repo, &["log", "--format=%H", "--", "f"])
    );

    // And this is the one place the two modes visibly disagree. `--full-history` lists the
    // ordinary merge as well, because it differs from its *second* parent — git only marks a
    // commit TREESAME (and prunes it) when `simplify_history` is on, which `--full-history` turns
    // off. The documentation's worked example only shows the TREESAME-to-both case, so this is the
    // assertion that keeps our rule honest.
    request.simplify = Simplify::Full;
    let full = walk(&info, &request);
    let summaries: Vec<&str> = full.commits.iter().map(|c| c.summary.as_str()).collect();
    assert!(summaries.contains(&"ordinary merge"), "{summaries:?}");
    assert_eq!(
        oids(&full),
        git_oids(&repo, &["log", "--format=%H", "--full-history", "--", "f"])
    );
}

#[test]
fn simplify_default_and_full_are_gits_two_answers() {
    let repo = TempRepo::new("log-simplify");
    merge_history(&repo);
    let info = info(&repo);

    let mut default = query(&info);
    default.path = Some("f".into());
    assert_eq!(
        oids(&walk(&info, &default)),
        git_oids(&repo, &["log", "--format=%H", "--", "f"]),
        "Simplify::Default is `git log -- <path>`"
    );

    let mut full = query(&info);
    full.path = Some("f".into());
    full.simplify = Simplify::Full;
    assert_eq!(
        oids(&walk(&info, &full)),
        git_oids(&repo, &["log", "--format=%H", "--full-history", "--", "f"]),
        "Simplify::Full is `--full-history`"
    );
}

#[test]
fn first_parent_is_gits_first_parent() {
    let repo = TempRepo::new("log-first-parent");
    merge_history(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.first_parent = true;
    request.limit = 100;
    assert_eq!(
        oids(&walk(&info, &request)),
        git_oids(&repo, &["log", "--format=%H", "--first-parent"])
    );
}

// --- follow -----------------------------------------------------------------------------------------------------

/// Thirty lines, one of them stamped, so two versions are ~97 % similar and a rename is detected
/// well above the threshold. A generated file of a handful of lines is a coin toss at 50 %.
fn body(tag: &str) -> String {
    let mut out = String::new();
    for line in 0..30 {
        if line == 7 {
            out.push_str(&format!("marker {tag}\n"));
        } else {
            out.push_str(&format!("line {line}\n"));
        }
    }
    out
}

#[test]
fn follow_crosses_a_rename_and_without_it_the_history_stops_there() {
    let repo = TempRepo::new("log-follow");
    let one = body("one");
    let two = body("two");
    let three = body("three");
    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "create old",
        None,
        None,
        &[Op::Set("100644", "old.txt", &one)],
    );
    let b = import.commit(
        "main",
        60,
        "edit old",
        Some(a),
        None,
        &[Op::Set("100644", "old.txt", &two)],
    );
    let c = import.commit(
        "main",
        120,
        "rename",
        Some(b),
        None,
        &[Op::Delete("old.txt"), Op::Set("100644", "new.txt", &two)],
    );
    import.commit(
        "main",
        180,
        "edit new",
        Some(c),
        None,
        &[Op::Set("100644", "new.txt", &three)],
    );
    import.run(&repo);
    let info = info(&repo);

    let mut off = query(&info);
    off.path = Some("new.txt".into());
    let page = walk(&info, &off);
    assert_eq!(
        page.commits.len(),
        2,
        "the file appears to begin at the rename"
    );
    assert!(!page.followed, "the flag was not asked for");

    let mut on = query(&info);
    on.path = Some("new.txt".into());
    on.follow = true;
    let page = walk(&info, &on);
    assert!(
        page.followed,
        "reported, so a silently-ignored flag is visible"
    );
    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        vec!["edit new", "rename", "edit old", "create old"],
        "{summaries:?}"
    );
    assert_eq!(
        oids(&page),
        git_oids(&repo, &["log", "--format=%H", "--follow", "--", "new.txt"])
    );

    assert_eq!(page.renames.len(), 1, "{:?}", page.renames);
    let hop = &page.renames[0];
    assert_eq!((hop.from.as_str(), hop.to.as_str()), ("old.txt", "new.txt"));
    assert!(
        hop.similarity >= 50,
        "libgit2's own score: {}",
        hop.similarity
    );
    assert!(!page.follow_capped);
}

#[test]
fn follow_is_refused_on_a_multi_tip_walk() {
    // Two tips can reach one commit under two different names, and the union of those is a claim
    // neither tip supports. Refused and *reported*, not silently ignored.
    let repo = TempRepo::new("log-follow-multi");
    let one = body("one");
    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "create",
        None,
        None,
        &[Op::Set("100644", "old.txt", &one)],
    );
    import.commit(
        "main",
        60,
        "rename",
        Some(a),
        None,
        &[Op::Delete("old.txt"), Op::Set("100644", "new.txt", &one)],
    );
    import.reset("other", a);
    import.run(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.refs = LogRefs::All;
    request.path = Some("new.txt".into());
    request.follow = true;
    let page = walk(&info, &request);
    assert!(!page.followed);
    assert!(
        page.renames.is_empty(),
        "no hop was taken: {:?}",
        page.renames
    );
}

#[test]
fn a_tracked_rename_does_not_leak_from_one_lane_into_another() {
    // The bug the per-frontier-entry tracked path exists to prevent. Two branches rename the same
    // original file to two *different* names and keep editing, interleaved in time so the walk
    // alternates between them; the merge keeps one name. With the tracked path on the walk instead
    // of on the entry, whichever lane is popped second inherits the other's name, finds nothing,
    // and its whole history silently disappears.
    let repo = TempRepo::new("log-rename-leak");
    let base = body("base");
    let a1 = body("a1");
    let a2 = body("a2");
    let b1 = body("b1");
    let b2 = body("b2");

    let mut import = Import::new();
    let root = import.commit(
        "main",
        0,
        "base",
        None,
        None,
        &[Op::Set("100644", "f", &base)],
    );
    // Interleaved: a-rename, b-rename, a-edit, b-edit.
    let ar = import.commit(
        "main",
        60,
        "a renames",
        Some(root),
        None,
        &[Op::Delete("f"), Op::Set("100644", "a.txt", &a1)],
    );
    let br = import.commit(
        "side",
        120,
        "b renames",
        Some(root),
        None,
        &[Op::Delete("f"), Op::Set("100644", "b.txt", &b1)],
    );
    let ae = import.commit(
        "main",
        180,
        "a edits",
        Some(ar),
        None,
        &[Op::Set("100644", "a.txt", &a2)],
    );
    let be = import.commit(
        "side",
        240,
        "b edits",
        Some(br),
        None,
        &[Op::Set("100644", "b.txt", &b2)],
    );
    // The merge keeps `a.txt` and drops `b.txt` — a rename/rename resolution, and the tree comes
    // from the first parent so it is exact.
    import.commit(
        "main",
        300,
        "merge",
        Some(ae),
        Some(be),
        &[Op::Delete("b.txt")],
    );
    import.run(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.path = Some("a.txt".into());
    request.follow = true;
    // `Full`, because the merge is TREESAME to its first parent for `a.txt` and git's default
    // simplification would therefore follow only that parent — correctly, and uselessly for a
    // test about two lanes.
    request.simplify = Simplify::Full;
    let page = walk(&info, &request);

    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();
    for expected in ["a edits", "a renames", "b edits", "b renames", "base"] {
        assert!(
            summaries.contains(&expected),
            "`{expected}` is missing, which is what a leaked tracked path looks like: {summaries:?}"
        );
    }
    let hops: Vec<(&str, &str)> = page
        .renames
        .iter()
        .map(|h| (h.from.as_str(), h.to.as_str()))
        .collect();
    assert!(hops.contains(&("f", "a.txt")), "{hops:?}");
    assert!(
        hops.contains(&("b.txt", "a.txt")),
        "the merge itself is a rename hop for the second lane: {hops:?}"
    );
    assert!(hops.contains(&("f", "b.txt")), "{hops:?}");
}

// --- filters ------------------------------------------------------------------------------------------------------

#[test]
fn author_and_text_filters_suppress_rows_and_never_the_traversal() {
    let repo = TempRepo::new("log-filters");
    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "PROJ-1 the oldest",
        None,
        None,
        &[Op::Set("100644", "f", "1")],
    );
    let b = import.commit(
        "main",
        60,
        "unrelated middle",
        Some(a),
        None,
        &[Op::Set("100644", "f", "2")],
    );
    import.commit(
        "main",
        120,
        "proj-2 the newest",
        Some(b),
        None,
        &[Op::Set("100644", "f", "3")],
    );
    import.run(&repo);
    let info = info(&repo);

    let mut text = query(&info);
    text.text = Some("PROJ".into());
    let page = walk(&info, &text);
    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        vec!["proj-2 the newest", "PROJ-1 the oldest"],
        "case-insensitive, and **the oldest one survived**: a filter must not prune the walk, or \
         every ancestor of the non-matching middle commit disappears: {summaries:?}"
    );
    assert_eq!(page.scanned, 3, "all three were walked");

    // An oid prefix is a text search people type constantly, and it is not in any message.
    let mut by_oid = query(&info);
    by_oid.text = Some(page.commits[1].oid[..7].to_string());
    let hit = walk(&info, &by_oid);
    assert_eq!(oids(&hit), vec![page.commits[1].oid.clone()]);

    let mut author = query(&info);
    author.author = Some("CIDE TESTS".into());
    assert_eq!(walk(&info, &author).commits.len(), 3, "name, case-folded");
    author.author = Some("tests@cide".into());
    assert_eq!(walk(&info, &author).commits.len(), 3, "or the address");
    author.author = Some("nobody".into());
    assert!(walk(&info, &author).commits.is_empty());
}

// --- revspecs ---------------------------------------------------------------------------------------------------------

/// `main` and `side` diverging from a shared base, so `..` and `...` differ.
fn diverged(repo: &TempRepo) {
    let mut import = Import::new();
    let base = import.commit(
        "main",
        0,
        "base",
        None,
        None,
        &[Op::Set("100644", "f", "0")],
    );
    let m1 = import.commit(
        "main",
        60,
        "main one",
        Some(base),
        None,
        &[Op::Set("100644", "f", "m1")],
    );
    import.commit(
        "main",
        120,
        "main two",
        Some(m1),
        None,
        &[Op::Set("100644", "f", "m2")],
    );
    let s1 = import.commit(
        "side",
        180,
        "side one",
        Some(base),
        None,
        &[Op::Set("100644", "g", "s1")],
    );
    import.commit(
        "side",
        240,
        "side two",
        Some(s1),
        None,
        &[Op::Set("100644", "g", "s2")],
    );
    import.run(repo);
}

#[test]
fn a_two_dot_range_is_rev_list_a_dot_dot_b() {
    let repo = TempRepo::new("log-range");
    diverged(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.refs = LogRefs::Rev {
        spec: "main..side".into(),
    };
    let mut ours = oids(&walk(&info, &request));
    let mut theirs = git_oids(&repo, &["rev-list", "main..side"]);
    ours.sort();
    theirs.sort();
    assert_eq!(ours, theirs);
}

#[test]
fn a_three_dot_range_is_rev_list_a_dot_dot_dot_b() {
    let repo = TempRepo::new("log-symmetric");
    diverged(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.refs = LogRefs::Rev {
        spec: "main...side".into(),
    };
    let mut ours = oids(&walk(&info, &request));
    let mut theirs = git_oids(&repo, &["rev-list", "main...side"]);
    ours.sort();
    theirs.sort();
    assert_eq!(
        ours, theirs,
        "the symmetric difference hides *every* merge base, not just the first"
    );
}

#[test]
fn an_empty_range_is_zero_rows_and_exhausted_not_an_error() {
    let repo = TempRepo::new("log-empty-range");
    diverged(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.refs = LogRefs::Rev {
        spec: "main..main".into(),
    };
    let page = walk(&info, &request);
    assert!(page.commits.is_empty());
    assert_eq!(page.stop, LogStop::Exhausted);
}

#[test]
fn a_single_revspec_walks_from_there() {
    let repo = TempRepo::new("log-single-rev");
    diverged(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.refs = LogRefs::Rev {
        spec: "side~1".into(),
    };
    assert_eq!(
        oids(&walk(&info, &request)),
        git_oids(&repo, &["rev-list", "side~1"])
    );
}

#[test]
fn the_three_revspec_refusals_are_told_apart() {
    let repo = TempRepo::new("log-revspec-refusals");
    diverged(&repo);
    let info = info(&repo);

    let mut bad = query(&info);
    bad.refs = LogRefs::Rev {
        spec: "no-such-branch-here".into(),
    };
    assert!(
        matches!(
            log::log(std::slice::from_ref(&info), &bad),
            Err(GitError::BadRevspec { .. })
        ),
        "a spec that does not resolve"
    );

    let mut tree = query(&info);
    tree.refs = LogRefs::Rev {
        spec: "HEAD^{tree}".into(),
    };
    match log::log(std::slice::from_ref(&info), &tree) {
        Err(GitError::NotACommit { kind, .. }) => assert_eq!(
            kind, "tree",
            "the kind is carried so the sentence can name it; libgit2's own message cannot"
        ),
        other => panic!("expected NotACommit, got {other:?}"),
    }
}

#[test]
fn a_branch_that_does_not_exist_is_a_stop_and_not_a_dialog() {
    let repo = TempRepo::new("log-no-such-branch");
    diverged(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.refs = LogRefs::Branch {
        name: "deleted-yesterday".into(),
    };
    let page = walk(&info, &request);
    assert!(page.commits.is_empty());
    assert_eq!(page.stop, LogStop::NoSuchRef);
}

// --- merged scope ---------------------------------------------------------------------------------------------------------

#[test]
fn a_merged_walk_interleaves_by_time_shares_the_budget_and_draws_no_graph() {
    let left = TempRepo::new("log-merged-a");
    let right = TempRepo::new("log-merged-b");
    // Interleaved on the clock: right, left, right, left, …
    let mut a = Import::new();
    let mut previous = None;
    for index in 0..4 {
        previous = Some(a.commit(
            "main",
            index as i64 * 120,
            &format!("left {index}"),
            previous,
            None,
            &[Op::Set("100644", "f", &format!("{index}"))],
        ));
    }
    a.run(&left);
    let mut b = Import::new();
    let mut previous = None;
    for index in 0..4 {
        previous = Some(b.commit(
            "main",
            60 + index as i64 * 120,
            &format!("right {index}"),
            previous,
            None,
            &[Op::Set("100644", "f", &format!("{index}"))],
        ));
    }
    b.run(&right);

    let infos = vec![info(&left), info(&right)];
    let request = LogQuery {
        scope: LogScope::Merged {
            repos: vec![infos[0].id, infos[1].id],
        },
        graph: true,
        ..query(&infos[0])
    };
    let page = log::log(&infos, &request).expect("a merged page");

    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(
        summaries,
        vec![
            "right 3", "left 3", "right 2", "left 2", "right 1", "left 1", "right 0", "left 0"
        ],
        "{summaries:?}"
    );
    assert_eq!(page.repos.len(), 2);
    assert_eq!(page.repos[0].name, infos[0].name);
    assert_eq!(page.scanned, 8, "the budget is shared, and so is the count");
    assert_eq!(page.stop, LogStop::Exhausted, "both are finished");
    assert!(
        matches!(
            page.graph,
            LogGraph::Off {
                reason: GraphOff::Merged
            }
        ),
        "two repositories share no DAG: {:?}",
        page.graph
    );
}

#[test]
fn a_merged_stop_is_the_most_restrictive_of_the_two() {
    let left = TempRepo::new("log-merged-stop-a");
    linear(&left, 6);
    let right = TempRepo::new("log-merged-stop-b");
    linear(&right, 6);

    let infos = vec![info(&left), info(&right)];
    let request = LogQuery {
        scope: LogScope::Merged {
            repos: vec![infos[0].id, infos[1].id],
        },
        // Only the left repository has this branch; the right one must report `NoSuchRef` rather
        // than being silently dropped — which would make the branch filter useless in exactly the
        // monorepo it exists for.
        refs: LogRefs::Branch {
            name: "only-here".into(),
        },
        ..query(&infos[0])
    };
    left.git(&["branch", "only-here", "main"]);

    let page = log::log(&infos, &request).expect("a merged page");
    assert_eq!(page.commits.len(), 6, "the left root's history is shown");
    assert_eq!(page.repos[1].stop, LogStop::NoSuchRef);
    assert_eq!(
        page.stop,
        LogStop::NoSuchRef,
        "NoSuchRef outranks Exhausted: the page is not finished, it is incomplete"
    );
}

// --- the graph seam ------------------------------------------------------------------------------------------------------------

#[test]
fn the_graph_says_why_it_is_off() {
    let repo = TempRepo::new("log-graph-off");
    merge_history(&repo);
    let info = info(&repo);

    let mut on = query(&info);
    on.graph = true;
    assert!(matches!(walk(&info, &on).graph, LogGraph::Rows { .. }));

    let mut off = query(&info);
    off.graph = false;
    assert!(matches!(
        walk(&info, &off).graph,
        LogGraph::Off {
            reason: GraphOff::Disabled
        }
    ));

    let mut filtered = query(&info);
    filtered.graph = true;
    filtered.text = Some("merge".into());
    assert!(matches!(
        walk(&info, &filtered).graph,
        LogGraph::Off {
            reason: GraphOff::Filtered
        }
    ));

    let mut full = query(&info);
    full.graph = true;
    full.simplify = Simplify::Full;
    full.path = Some("f".into());
    assert!(matches!(
        walk(&info, &full).graph,
        LogGraph::Off {
            reason: GraphOff::FullHistory
        }
    ));

    // The one that is easy to get backwards: a path filter under the *default* simplification
    // keeps its graph, because the parents are rewritten onto surviving ancestors. This is why
    // `git log --graph -- path` works.
    let mut path = query(&info);
    path.graph = true;
    path.path = Some("f".into());
    assert!(
        matches!(walk(&info, &path).graph, LogGraph::Rows { .. }),
        "a rewritten parent function still closes the row set"
    );

    let mut rerooted = query(&info);
    rerooted.graph = true;
    rerooted.cursor = LogCursor::At {
        oid: walk(&info, &query(&info)).commits[1].oid.clone(),
    };
    assert!(matches!(
        walk(&info, &rerooted).graph,
        LogGraph::Off {
            reason: GraphOff::Rerooted
        }
    ));
}

#[test]
fn a_graph_row_exists_for_every_commit_row() {
    let repo = TempRepo::new("log-graph-parallel");
    merge_history(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.graph = true;
    request.refs = LogRefs::All;
    let page = walk(&info, &request);
    match &page.graph {
        LogGraph::Rows { rows, lanes, .. } => {
            assert_eq!(
                rows.len(),
                page.commits.len(),
                "parallel to `commits`, index for index"
            );
            assert!(*lanes >= 1);
        }
        other => panic!("expected rows, got {other:?}"),
    }
}

#[test]
fn simplification_counts_what_it_hid() {
    let repo = TempRepo::new("log-pruned");
    path_history(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.path = Some("f".into());
    request.graph = true;
    let page = walk(&info, &request);

    // `create f` … `unrelated` … `change f`: exactly one commit was simplified away between the
    // two rows that touched `f`, and the row that points across the gap says so.
    let hidden: u32 = page.commits.iter().map(|c| c.pruned).sum();
    assert!(
        hidden >= 1,
        "the two `unrelated` commits are hidden between rows and must be counted: {:?}",
        page.commits
            .iter()
            .map(|c| (c.summary.as_str(), c.pruned))
            .collect::<Vec<_>>()
    );
    // The real parents stay real: a caller that passes one to `git show` must get git's answer.
    let change = page
        .commits
        .iter()
        .find(|c| c.summary == "change f")
        .expect("the row exists");
    let real = repo.git(&["rev-parse", &format!("{}^", change.oid)]);
    assert_eq!(change.parents, vec![real.trim().to_string()]);
}

// --- clamps ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_limits_are_clamped_in_rust() {
    let repo = TempRepo::new("log-clamps");
    linear(&repo, 5);
    let info = info(&repo);

    // A frontend bug asking for a million rows must not be able to allocate them here.
    let mut huge = query(&info);
    huge.limit = u32::MAX;
    huge.scan_limit = u32::MAX;
    assert_eq!(walk(&info, &huge).commits.len(), 5);

    let mut zero = query(&info);
    zero.limit = 0;
    zero.scan_limit = 0;
    assert_eq!(
        walk(&info, &zero).commits.len(),
        5,
        "zero means the default, not a page of nothing"
    );
}

#[test]
fn a_hop_is_charged_to_the_budget_and_a_tree_wide_move_refuses_to_pay() {
    // A hop is a whole-tree diff plus `find_similar`, not a commit's worth of work, so it costs
    // `RENAME_SCAN_COST` against `scan_limit`. Charging it as one commit would let a follow walk
    // burn its entire budget without the number moving, which is the failure the budget exists to
    // make visible.
    let repo = TempRepo::new("log-follow-cost");
    let one = body("one");
    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "create",
        None,
        None,
        &[Op::Set("100644", "old.txt", &one)],
    );
    import.commit(
        "main",
        60,
        "rename",
        Some(a),
        None,
        &[Op::Delete("old.txt"), Op::Set("100644", "new.txt", &one)],
    );
    import.run(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.path = Some("new.txt".into());
    request.follow = true;
    let page = walk(&info, &request);
    assert_eq!(page.commits.len(), 2);
    assert_eq!(
        page.scanned,
        2 + log::RENAME_SCAN_COST,
        "two commits and one hop"
    );
}

#[test]
fn a_tree_wide_move_reports_that_it_gave_up_rather_than_inventing_a_hop() {
    // `find_similar` is quadratic in the unmatched adds and deletes, so the one commit that most
    // needs rename detection — a repository-wide move — is the one that cannot afford it.
    // `RENAME_MAX_DELTAS` refuses the pass, and `follow_capped` says so: from that point back the
    // history shown is the file's under its current name only, which is a true subsequence but not
    // the whole story.
    let repo = TempRepo::new("log-follow-capped");
    let one = body("one");
    let filler: Vec<String> = (0..2_100).map(|index| format!("noise/f{index}")).collect();
    let before: Vec<String> = (0..2_100).map(|index| format!("v1 {index}\n")).collect();
    let after: Vec<String> = (0..2_100).map(|index| format!("v2 {index}\n")).collect();

    let mut import = Import::new();
    let mut ops: Vec<Op<'_>> = filler
        .iter()
        .zip(before.iter())
        .map(|(path, bytes)| Op::Set("100644", path, bytes))
        .collect();
    ops.push(Op::Set("100644", "old.txt", &one));
    let a = import.commit("main", 0, "create everything", None, None, &ops);
    drop(ops);

    let mut ops: Vec<Op<'_>> = filler
        .iter()
        .zip(after.iter())
        .map(|(path, bytes)| Op::Set("100644", path, bytes))
        .collect();
    ops.push(Op::Delete("old.txt"));
    ops.push(Op::Set("100644", "new.txt", &one));
    import.commit("main", 60, "move the world", Some(a), None, &ops);
    drop(ops);
    import.run(&repo);
    let info = info(&repo);

    let mut request = query(&info);
    request.path = Some("new.txt".into());
    request.follow = true;
    let page = walk(&info, &request);

    assert!(
        page.followed,
        "the flag was applied; the *hop* is what was refused"
    );
    assert!(
        page.follow_capped,
        "and the page says so instead of implying the file was created here"
    );
    assert!(page.renames.is_empty(), "{:?}", page.renames);
    let summaries: Vec<&str> = page.commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, vec!["move the world"], "{summaries:?}");
}

// --- cancellation ---------------------------------------------------------------------------------------------------------------------

/// The walk, over a flag the caller holds. `cide-git` owns no state, so the registry that decides
/// a page has been superseded lives in `cide-app` and lends this in — see the module header.
fn cancellable(info: &RepoInfo, request: &LogQuery, cancel: &AtomicBool) -> CommitPage {
    log::log_cancellable(&LogWalk {
        repos: std::slice::from_ref(info),
        query: request,
        cancel,
    })
    .expect("a cancelled walk is a partial answer, not a failure")
}

/// A flag already set stops the walk before it looks at a single commit.
///
/// The degenerate end of the property the next test covers, and worth its own test because it is
/// the one a poll placed at the *bottom* of the loop body would fail: the page would come back
/// holding one commit nobody asked it to walk, and on a repository whose first row costs a
/// whole-tree diff that one commit is the whole complaint.
#[test]
fn a_flag_set_before_the_first_poll_returns_an_empty_cancelled_page() {
    let repo = TempRepo::new("log-cancel-immediate");
    linear(&repo, 20);
    let info = info(&repo);

    let stop = AtomicBool::new(true);
    let page = cancellable(&info, &query(&info), &stop);

    assert!(page.cancelled, "the page has to say why it is short");
    assert!(page.commits.is_empty(), "{:?}", oids(&page));
    assert_eq!(
        page.scanned, 0,
        "the poll is at the top of the loop body, before the commit is even looked up"
    );
    assert_ne!(
        page.stop,
        LogStop::Exhausted,
        "a walk that never started has not reached the end of history, and a panel that read \
         `Exhausted` here would hide `load more` over twenty commits it never saw"
    );
    assert!(
        page.resume.is_some(),
        "the tips are still on the frontier, so there is a continuation to hand back"
    );
}

/// A linear history of `count` commits where every hundredth summary carries `needle`.
///
/// Built for the cancellation test below and for one reason: it makes the interval during which a
/// cancellation is *observable* almost the whole walk. A history where every commit matched would
/// have to be caught before the page filled, and one where a single commit matched could never be
/// caught short. Here the first match lands 1% in and the last 100% in, so the flag can be set
/// almost anywhere and still produce the state under test — some rows, and not all of them.
fn sparse_matches(repo: &TempRepo, count: usize) {
    let mut import = Import::new();
    let mut previous = None;
    for index in 0..count {
        let body = format!("line {index}");
        let summary = if index % 100 == 0 {
            format!("commit {index} needle")
        } else {
            format!("commit {index}")
        };
        previous = Some(import.commit(
            "main",
            index as i64 * 60,
            &summary,
            previous,
            None,
            &[Op::Set("100644", "file.txt", &body)],
        ));
    }
    import.run(repo);
}

/// A walk stopped partway keeps the rows it had, says so, and is **not** an error.
///
/// # Why the flag is set on a timer, and why the timer is searched for
///
/// There is no hook inside the walk to fire from — deliberately, because the seam is one atomic,
/// and a test-only callback would be the `&dyn Fn` design the module header rejects, let in
/// through the back door. So the flag is set from a second thread while the walk runs.
///
/// A *fixed* sleep would be a coin flip: five thousand commits take a few milliseconds on a warm
/// page cache and can take twenty times that on a cold one or a loaded CI box, and a thread spawn
/// plus a sleep has a floor of its own. The delay is therefore searched in both directions —
/// doubled when the flag was set before the first row (an empty cancelled page), halved when the
/// walk had already finished (no cancellation at all) — until it lands inside the window
/// [`sparse_matches`] exists to widen. The property under test is not "cancellation is prompt"; it
/// is "a walk that *is* cancelled reports what it found", and the search is what makes that the
/// only thing this test can fail on.
#[test]
fn a_walk_cancelled_partway_keeps_the_rows_it_found_and_is_not_an_error() {
    let repo = TempRepo::new("log-cancel-partway");
    sparse_matches(&repo, 5_000);
    let info = info(&repo);
    let mut request = query(&info);
    request.text = Some("needle".into());
    request.limit = log::LIMIT_MAX;

    let whole = walk(&info, &request);
    assert!(
        !whole.cancelled,
        "the uninterrupted answer, to compare against"
    );
    assert_eq!(whole.commits.len(), 50, "one match per hundred commits");
    assert_eq!(whole.stop, LogStop::Exhausted);

    let mut delay = Duration::from_micros(100);
    let mut attempts = 0;
    let page = loop {
        attempts += 1;
        assert!(
            attempts <= 40,
            "forty attempts never caught the walk mid-flight; either the flag is no longer \
             polled, or the walk no longer runs"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(delay);
            flag.store(true, Ordering::Release);
        });
        let page = cancellable(&info, &request, &stop);
        canceller.join().expect("the canceller thread");

        if page.cancelled && !page.commits.is_empty() {
            break page;
        }
        if page.cancelled {
            // Set before the walk had emitted anything. Give it longer.
            delay *= 2;
            assert!(
                delay < Duration::from_secs(4),
                "the walk cannot be this slow"
            );
        } else {
            // The walk finished first. Set it sooner.
            delay /= 2;
        }
    };

    assert!(
        page.commits.len() < whole.commits.len(),
        "caught mid-walk, so the page must be short: {} of {}",
        page.commits.len(),
        whole.commits.len()
    );
    let found = oids(&page);
    let expected: Vec<String> = oids(&whole).into_iter().take(found.len()).collect();
    assert_eq!(
        found, expected,
        "the rows are the ones this same walk would have emitted, in the same order — a partial \
         answer, not a different one"
    );
    assert!(
        page.scanned < whole.scanned,
        "the walk stopped where the flag was seen rather than running to the end of history: \
         {} of {} commits examined",
        page.scanned,
        whole.scanned
    );
    assert_ne!(
        page.stop,
        LogStop::Exhausted,
        "thousands of commits are still queued; only `cancelled` explains the missing rows, and \
         a panel that read `Exhausted` here would hide `load more` over all of them"
    );
}

/// `log` and `log_cancellable` over an unset flag are the same function.
///
/// This is the test that earns the wrapper. `log` is kept so that the forty-odd tests above, and
/// every caller that has nothing to cancel, are untouched by this feature — and the only way that
/// stays true is if the two agree on a page rich enough to catch a difference: several branches so
/// there is a graph, a limit short of history so there is a resume token and a `Page` stop, and
/// ref chips on the tips.
///
/// Compared as serialised JSON rather than field by field, deliberately. A field added to
/// `CommitPage` later that the wrapper failed to carry would pass a hand-written comparison of the
/// fields somebody thought of in 2026 and fail this one.
#[test]
fn an_uncancelled_walk_is_byte_identical_to_logs_answer() {
    let repo = TempRepo::new("log-cancel-identical");
    linear(&repo, 40);
    let info = info(&repo);
    let mut request = query(&info);
    request.limit = 10;
    request.graph = true;

    let direct = walk(&info, &request);
    let never = AtomicBool::new(false);
    let through = cancellable(&info, &request, &never);

    assert!(!direct.cancelled, "nothing set the flag");
    assert!(!through.cancelled);
    assert_eq!(direct.commits.len(), 10, "a page short of history");
    assert!(direct.resume.is_some(), "with a continuation");
    assert!(
        matches!(direct.graph, LogGraph::Rows { .. }),
        "and a graph, so the lane state is part of what is being compared"
    );
    assert_eq!(
        serde_json::to_string(&direct).expect("a page serialises"),
        serde_json::to_string(&through).expect("a page serialises"),
        "the wrapper has to be the same walk, byte for byte, or every test above is testing \
         something the app no longer calls"
    );
}
