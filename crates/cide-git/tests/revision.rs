//! The revision read surface, against real repositories with the real `git` binary as the
//! oracle.
//!
//! Two claims are worth testing here and everything else follows from them.
//!
//! **The diff is the same diff `git show` prints.** Not similar — the same hunk headers and the
//! same body lines, because the pane this feeds is read side by side with a terminal and a
//! divergence of one context line is the kind of thing that gets reported as "cide's diff is
//! wrong" and takes a day to trace. `git show` is therefore the oracle for every ordinary case.
//!
//! **A rename survives.** `cide_git::revision` builds every diff over the whole tree and then
//! picks the delta, specifically so `find_similar` sees both sides of a rename. The
//! `a_rename_carries_both_paths` test is the one that fails if somebody "optimises" that back
//! into a pathspec, and it is the only test here that would.

mod support;

use std::collections::HashSet;

use cide_git::revision::{self, MAX_BLOB_BYTES, RANGE_FILE_CAP};
use cide_ipc::git::{FileState, GitError, LineOrigin};
use cide_ipc::history::{RevSide, RevisionDiff};
use support::TempRepo;

// --- fixtures ---------------------------------------------------------------------------------

/// Three commits, each editing a different part of one file, plus a second file nobody touches
/// after the first commit.
fn history(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("src.txt", b"one\ntwo\nthree\nfour\nfive\nsix\n");
    repo.write("other.txt", b"untouched\n");
    repo.commit_all("first");
    repo.write("src.txt", b"one\nTWO\nthree\nfour\nfive\nsix\n");
    repo.commit_all("second");
    repo.write("src.txt", b"one\nTWO\nthree\nfour\nfive\nSIX\nseven\n");
    repo.commit_all("third");
    repo
}

fn commit(repo: &TempRepo, spec: &str) -> RevSide {
    RevSide::Commit {
        oid: repo.git(&["rev-parse", spec]).trim().to_string(),
    }
}

fn oid(repo: &TempRepo, spec: &str) -> String {
    repo.git(&["rev-parse", spec]).trim().to_string()
}

// --- helpers ----------------------------------------------------------------------------------

/// Our hunks flattened to the lines `git` would print for them.
fn ours(diff: &RevisionDiff) -> Vec<String> {
    let mut out = Vec::new();
    for hunk in &diff.hunks {
        out.push(hunk.header.clone());
        for line in &hunk.lines {
            let origin = match line.origin {
                LineOrigin::Addition => '+',
                LineOrigin::Deletion => '-',
                LineOrigin::Context => ' ',
            };
            out.push(format!("{origin}{}", line.content));
        }
    }
    out
}

/// The same lines out of the `git` binary.
///
/// Everything above the first `@@` — `diff --git`, `index`, `---`, `+++` — is dropped, because
/// that header is libgit2's to render and `cide_git::diff` already asserts it byte for byte in
/// its own tests. The `\ No newline at end of file` marker is dropped too: it rides on the
/// [`cide_ipc::git::DiffLineView::no_newline`] flag rather than being a line of its own.
fn theirs(repo: &TempRepo, args: &[&str]) -> Vec<String> {
    let text = repo.git(args);
    let mut out = Vec::new();
    let mut started = false;
    for line in text.lines() {
        if line.starts_with("@@") {
            started = true;
        }
        if !started || line.starts_with('\\') {
            continue;
        }
        out.push(line.to_string());
    }
    out
}

// --- one file at a revision -------------------------------------------------------------------

#[test]
fn a_files_diff_at_a_commit_matches_git_show() {
    let repo = history("show");
    let head = oid(&repo, "HEAD");
    let diff = revision::revision_diff(
        &repo.root,
        "src.txt",
        &commit(&repo, "HEAD"),
        &RevSide::FirstParent,
    )
    .expect("revision diff");

    assert_eq!(diff.path, "src.txt");
    assert_eq!(diff.old_path, None);
    assert_eq!(diff.status, FileState::Modified);
    assert!(!diff.binary);
    assert_eq!(diff.new_oid.as_deref(), Some(head.as_str()));
    assert_eq!(diff.old_oid.as_deref(), Some(oid(&repo, "HEAD~1").as_str()));
    assert_eq!(diff.new, commit(&repo, "HEAD"));
    assert_eq!(diff.old, RevSide::FirstParent);

    assert_eq!(
        ours(&diff),
        theirs(&repo, &["show", "--format=", &head, "--", "src.txt"])
    );
}

#[test]
fn a_file_added_by_the_root_commit_diffs_against_the_empty_tree() {
    let repo = history("root");
    let root = oid(&repo, "HEAD~2");
    let diff = revision::revision_diff(
        &repo.root,
        "src.txt",
        &RevSide::Commit { oid: root.clone() },
        &RevSide::FirstParent,
    )
    .expect("the first commit of a repository is still a diff");

    assert_eq!(diff.status, FileState::Added);
    assert_eq!(
        diff.old_oid, None,
        "there is no old commit; the empty tree has no oid to quote"
    );
    assert_eq!(diff.old_mode, 0, "the convention FileDiff already uses");
    assert_eq!(
        ours(&diff),
        theirs(&repo, &["show", "--format=", &root, "--", "src.txt"])
    );
}

#[test]
fn an_explicit_old_two_commits_back_accumulates_both_changes() {
    let repo = history("accumulate");
    let head = oid(&repo, "HEAD");
    let back = oid(&repo, "HEAD~2");
    let diff = revision::revision_diff(
        &repo.root,
        "src.txt",
        &RevSide::Commit { oid: head.clone() },
        &RevSide::Commit { oid: back.clone() },
    )
    .expect("revision diff");

    assert_eq!(
        ours(&diff),
        theirs(&repo, &["diff", &back, &head, "--", "src.txt"])
    );
    // Both edits are in the one diff; against the immediate parent only the second one is.
    let against_parent = revision::revision_diff(
        &repo.root,
        "src.txt",
        &RevSide::Commit { oid: head },
        &RevSide::FirstParent,
    )
    .expect("revision diff");
    assert!(
        ours(&diff).len() > ours(&against_parent).len(),
        "two commits' worth of change is more than one's"
    );
}

#[test]
fn the_working_tree_is_a_legal_new_side() {
    let repo = history("workdir");
    repo.write(
        "src.txt",
        b"one\nTWO\nthree\nfour\nfive\nSIX\nseven\nUNSAVED\n",
    );
    let diff = revision::revision_diff(
        &repo.root,
        "src.txt",
        &RevSide::WorkingTree,
        &commit(&repo, "HEAD"),
    )
    .expect("revision diff");

    assert_eq!(diff.new, RevSide::WorkingTree);
    assert_eq!(diff.new_oid, None, "the working tree is not a commit");
    assert_eq!(diff.old_oid.as_deref(), Some(oid(&repo, "HEAD").as_str()));
    assert_eq!(diff.status, FileState::Modified);
    assert_eq!(
        ours(&diff),
        theirs(&repo, &["diff", "HEAD", "--", "src.txt"])
    );
}

/// The test that fails if the whole-tree diff is ever narrowed to a pathspec.
///
/// A pathspec hands `git_diff_find_similar` one side of the rename; the pair never forms, the
/// survivor comes back `Added`, and the pane shows the entire file as new with no old path — a
/// plausible, quiet, completely wrong answer.
#[test]
fn a_rename_at_a_revision_carries_both_paths() {
    let repo = TempRepo::new("rename");
    repo.write("old.txt", b"alpha\nbravo\ncharlie\ndelta\necho\n");
    repo.commit_all("first");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", b"alpha\nBRAVO\ncharlie\ndelta\necho\n");
    repo.commit_all("rename and edit");

    let diff = revision::revision_diff(
        &repo.root,
        "new.txt",
        &commit(&repo, "HEAD"),
        &RevSide::FirstParent,
    )
    .expect("revision diff");
    assert_eq!(diff.status, FileState::Renamed);
    assert_eq!(diff.path, "new.txt");
    assert_eq!(diff.old_path.as_deref(), Some("old.txt"));
    assert!(
        !diff.hunks.is_empty(),
        "and the content change is still there"
    );

    // Asking by the *old* name finds the same delta, because a rename is one change with two
    // names and a caller may hold either.
    let by_old = revision::revision_diff(
        &repo.root,
        "old.txt",
        &commit(&repo, "HEAD"),
        &RevSide::FirstParent,
    )
    .expect("revision diff");
    assert_eq!(by_old.path, "new.txt");
    assert_eq!(by_old.old_path.as_deref(), Some("old.txt"));
}

#[test]
fn a_binary_file_is_flagged_and_has_no_hunks() {
    let repo = TempRepo::new("binary");
    repo.write("blob.bin", b"\x00\x01\x02\x03plain\n");
    repo.commit_all("first");
    repo.write("blob.bin", b"\x00\x09\x08\x07other\n");
    repo.commit_all("second");

    let diff = revision::revision_diff(
        &repo.root,
        "blob.bin",
        &commit(&repo, "HEAD"),
        &RevSide::FirstParent,
    )
    .expect("revision diff");
    assert!(diff.binary);
    assert!(diff.hunks.is_empty(), "there is nothing to render as lines");
    assert_eq!(diff.status, FileState::Modified);
}

#[test]
fn a_path_the_commit_did_not_touch_is_no_such_change() {
    let repo = history("untouched");
    let error = revision::revision_diff(
        &repo.root,
        "other.txt",
        &commit(&repo, "HEAD"),
        &RevSide::FirstParent,
    )
    .expect_err("an empty diff and `this commit is not about this file` are different claims");
    assert!(
        matches!(&error, GitError::NoSuchChange { path } if path == "other.txt"),
        "{error:?}"
    );
}

#[test]
fn a_deletion_keeps_its_path_and_its_lines() {
    let repo = history("deletion");
    repo.git(&["rm", "-q", "other.txt"]);
    repo.git(&["commit", "-q", "-m", "remove other"]);
    let diff = revision::revision_diff(
        &repo.root,
        "other.txt",
        &commit(&repo, "HEAD"),
        &RevSide::FirstParent,
    )
    .expect("revision diff");
    assert_eq!(diff.status, FileState::Deleted);
    assert_eq!(diff.new_mode, 0);
    assert_eq!(
        ours(&diff),
        theirs(
            &repo,
            &["show", "--format=", &oid(&repo, "HEAD"), "--", "other.txt"]
        )
    );
}

// --- the illegal pairings -----------------------------------------------------------------

#[test]
fn first_parent_is_refused_as_the_new_side() {
    let repo = history("side-new-parent");
    let error = revision::revision_diff(
        &repo.root,
        "src.txt",
        &RevSide::FirstParent,
        &commit(&repo, "HEAD"),
    )
    .expect_err("the first parent of what?");
    assert!(matches!(error, GitError::BadRevspec { .. }), "{error:?}");
}

#[test]
fn the_working_tree_is_refused_as_the_old_side() {
    let repo = history("side-old-workdir");
    let error = revision::revision_diff(
        &repo.root,
        "src.txt",
        &commit(&repo, "HEAD"),
        &RevSide::WorkingTree,
    )
    .expect_err("a diff whose old side moves under it is not a diff of anything");
    assert!(matches!(error, GitError::BadRevspec { .. }), "{error:?}");
}

#[test]
fn first_parent_against_the_working_tree_is_refused() {
    let repo = history("side-workdir-parent");
    let error = revision::revision_diff(
        &repo.root,
        "src.txt",
        &RevSide::WorkingTree,
        &RevSide::FirstParent,
    )
    .expect_err("the working tree has no parents");
    assert!(matches!(error, GitError::BadRevspec { .. }), "{error:?}");
    // The same refusal from the range entry point, so the two cannot drift apart.
    assert!(
        revision::range_files(&repo.root, &RevSide::WorkingTree, &RevSide::FirstParent).is_err()
    );
}

// --- the range ---------------------------------------------------------------------------

#[test]
fn a_range_lists_every_file_with_its_counts() {
    let repo = history("range");
    let range = revision::range_files(&repo.root, &commit(&repo, "HEAD"), &commit(&repo, "HEAD~2"))
        .expect("range");

    assert!(!range.truncated);
    assert_eq!(range.new_summary, "third");
    assert_eq!(range.old_summary, "first");
    let names: Vec<&str> = range.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        names,
        vec!["src.txt"],
        "other.txt is unchanged across the range"
    );

    let file = &range.files[0];
    assert_eq!(file.status, FileState::Modified);
    assert!(!file.binary);
    // Cross-checked against git's own arithmetic rather than counted by hand: `two`->`TWO`,
    // `six`->`SIX` and an added `seven`.
    let numstat = repo.git(&[
        "diff",
        "--numstat",
        &oid(&repo, "HEAD~2"),
        &oid(&repo, "HEAD"),
        "--",
        "src.txt",
    ]);
    let mut fields = numstat.split_whitespace();
    let additions: u32 = fields.next().unwrap().parse().unwrap();
    let deletions: u32 = fields.next().unwrap().parse().unwrap();
    assert_eq!((file.additions, file.deletions), (additions, deletions));
}

#[test]
fn a_range_against_the_root_commits_first_parent_is_the_whole_tree() {
    let repo = history("range-root");
    let range = revision::range_files(&repo.root, &commit(&repo, "HEAD~2"), &RevSide::FirstParent)
        .expect("range");
    let mut names: Vec<&str> = range.files.iter().map(|f| f.path.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["other.txt", "src.txt"]);
    assert!(range.files.iter().all(|f| f.status == FileState::Added));
    assert_eq!(range.old_oid, None);
    assert_eq!(range.old_summary, "");
}

#[test]
fn a_range_over_the_cap_is_truncated_and_says_so() {
    let repo = TempRepo::new("range-cap");
    let over = RANGE_FILE_CAP + 5;
    for index in 0..over {
        repo.write(&format!("many/{index:05}.txt"), b"one line\n");
    }
    repo.commit_all("a great many files");

    let range = revision::range_files(&repo.root, &commit(&repo, "HEAD"), &RevSide::FirstParent)
        .expect("range");
    assert_eq!(range.files.len(), RANGE_FILE_CAP);
    assert!(range.truncated, "{over} files is more than the cap");
}

// --- a file's contents ----------------------------------------------------------------------

#[test]
fn a_file_at_a_revision_is_the_text_that_was_committed() {
    let repo = history("blob");
    let blob = revision::file_at_revision(&repo.root, "src.txt", &oid(&repo, "HEAD~2"))
        .expect("file at revision");
    assert_eq!(blob.text, "one\ntwo\nthree\nfour\nfive\nsix\n");
    assert!(!blob.binary);
    assert!(!blob.truncated);
    assert_eq!(blob.bytes as usize, blob.text.len());
    assert_eq!(
        blob.oid,
        repo.git(&["rev-parse", "HEAD~2:src.txt"]).trim(),
        "the blob's own oid, not the commit's"
    );
}

#[test]
fn a_binary_blob_shows_nothing_rather_than_a_screenful_of_replacements() {
    let repo = TempRepo::new("blob-binary");
    let bytes = b"\x00\x01\x02\x03\x04not text at all\n";
    repo.write("blob.bin", bytes);
    repo.commit_all("first");
    let blob = revision::file_at_revision(&repo.root, "blob.bin", "HEAD").expect("file");
    assert!(blob.binary);
    assert_eq!(blob.text, "");
    assert!(
        !blob.truncated,
        "binary and truncated are different sentences"
    );
    assert_eq!(blob.bytes as usize, bytes.len());
}

#[test]
fn a_large_file_is_truncated_at_a_line_boundary_and_says_so() {
    let repo = TempRepo::new("blob-large");
    let line = b"the quick brown fox jumps over the lazy dog\n";
    let mut content = Vec::with_capacity(MAX_BLOB_BYTES + line.len() * 100);
    while content.len() < MAX_BLOB_BYTES + line.len() * 50 {
        content.extend_from_slice(line);
    }
    let total = content.len();
    repo.write("big.txt", &content);
    repo.commit_all("a large file");

    let blob = revision::file_at_revision(&repo.root, "big.txt", "HEAD").expect("file");
    assert!(blob.truncated);
    assert!(!blob.binary);
    assert_eq!(
        blob.bytes as usize, total,
        "the real size, not the shown size"
    );
    assert!(blob.text.len() <= MAX_BLOB_BYTES);
    assert!(
        blob.text.ends_with('\n'),
        "a half line at the bottom of the pane reads as corruption"
    );
}

#[test]
fn a_file_that_did_not_exist_yet_is_not_tracked() {
    let repo = history("blob-missing");
    let error =
        revision::file_at_revision(&repo.root, "later.txt", "HEAD").expect_err("never committed");
    assert!(
        matches!(&error, GitError::NotTracked { path } if path == "later.txt"),
        "{error:?}"
    );
}

#[test]
fn a_file_at_a_revision_that_is_gone_is_refused() {
    let repo = history("blob-no-rev");
    let error = revision::file_at_revision(
        &repo.root,
        "src.txt",
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    )
    .expect_err("no such object");
    assert!(
        matches!(
            error,
            GitError::NoSuchCommit { .. } | GitError::NoSuchRevision { .. }
        ),
        "{error:?}"
    );
}

// --- resolving what a user typed ---------------------------------------------------------

#[test]
fn a_branch_name_resolves_to_its_tip_and_keeps_its_name() {
    let repo = history("resolve-branch");
    let resolved = revision::resolve_rev(&repo.root, "main").expect("resolve");
    assert_eq!(resolved.spec, "main");
    assert_eq!(resolved.from, oid(&repo, "main"));
    assert_eq!(resolved.to, None);
    assert!(!resolved.range);
    assert_eq!(
        resolved.label, "main",
        "a tab titled with an oid is unreadable"
    );
}

#[test]
fn a_relative_spec_resolves_to_an_oid_and_is_labelled_with_one() {
    let repo = history("resolve-relative");
    let resolved = revision::resolve_rev(&repo.root, "HEAD~2").expect("resolve");
    assert_eq!(resolved.from, oid(&repo, "HEAD~2"));
    assert!(!resolved.range);
    assert!(
        resolved.from.starts_with(&resolved.label),
        "the label abbreviates the oid it resolved to: {resolved:?}"
    );
    assert!(resolved.label.len() >= 7, "{resolved:?}");
}

#[test]
fn a_range_resolves_to_both_ends() {
    let repo = history("resolve-range");
    let resolved = revision::resolve_rev(&repo.root, "HEAD~2..HEAD").expect("resolve");
    assert!(resolved.range);
    assert_eq!(resolved.from, oid(&repo, "HEAD~2"));
    assert_eq!(resolved.to.as_deref(), Some(oid(&repo, "HEAD").as_str()));
    assert!(resolved.label.contains('‥'), "{resolved:?}");
}

#[test]
fn an_annotated_tag_peels_to_its_commit() {
    let repo = history("resolve-tag");
    repo.git(&["tag", "-a", "v1.0", "-m", "release"]);
    let resolved = revision::resolve_rev(&repo.root, "v1.0").expect("resolve");
    assert_eq!(
        resolved.from,
        oid(&repo, "HEAD"),
        "the commit, not the tag object"
    );
    assert_eq!(resolved.label, "v1.0");
}

#[test]
fn a_tree_spec_is_refused_as_not_a_commit() {
    let repo = history("resolve-tree");
    let error = revision::resolve_rev(&repo.root, "HEAD^{tree}")
        .expect_err("it resolves fine and then fails to peel, which is its own answer");
    match error {
        GitError::NotACommit { spec, kind } => {
            assert_eq!(spec, "HEAD^{tree}");
            assert_eq!(kind, "tree", "git's own word for what it found");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn garbage_is_refused_as_a_bad_revspec() {
    let repo = history("resolve-garbage");
    let error =
        revision::resolve_rev(&repo.root, "no-such-thing-anywhere").expect_err("nothing matches");
    match error {
        GitError::BadRevspec { spec, detail } => {
            assert_eq!(spec, "no-such-thing-anywhere");
            assert!(
                !detail.is_empty(),
                "libgit2's own complaint is the actionable half"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// An abbreviation that matches more than one object gets its own refusal, because the fix is
/// mechanical — type more characters — and "that revspec does not parse" would send the user
/// looking for a syntax error in a string that parses perfectly.
///
/// Asserted at **two** hex characters as well as four. That was worth checking rather than
/// assuming: `GIT_OID_MINPREFIXLEN` is 4 and it would have been reasonable to expect libgit2 to
/// refuse a two-character string as unparseable before ever looking an object up. It does not —
/// `git_revparse_single` tries the reference dwim, fails, and then does the prefix lookup anyway
/// — so two characters is `GIT_EAMBIGUOUS` like any other collision, and the box can explain
/// itself from the first keystroke that is not a ref.
#[test]
fn an_ambiguous_abbreviation_is_refused_as_ambiguous() {
    let repo = TempRepo::new("resolve-ambiguous");
    repo.write("seed.txt", b"seed\n");
    repo.commit_all("first");

    // Enough blobs that a four-hex collision is overwhelming: with 1500 objects over 65 536
    // buckets the chance of no collision at all is vanishingly small, and the loop below asserts
    // rather than assumes.
    let mut paths = String::new();
    for index in 0..1500u32 {
        let name = format!("many/{index:05}.txt");
        repo.write(&name, format!("blob {index}\n").as_bytes());
        paths.push_str(&name);
        paths.push('\n');
    }
    let (ok, out) = repo.try_git_stdin(&["hash-object", "-w", "--stdin-paths"], paths.as_bytes());
    assert!(ok, "hash-object failed:\n{out}");

    let mut seen: HashSet<String> = HashSet::new();
    let mut prefix = None;
    for oid in out.lines() {
        let head = oid.trim();
        if head.len() < 4 {
            continue;
        }
        if !seen.insert(head[..4].to_string()) {
            prefix = Some(head[..4].to_string());
            break;
        }
    }
    let prefix = prefix.expect("1500 blobs must collide on four hex characters");

    let error = revision::resolve_rev(&repo.root, &prefix).expect_err("more than one object");
    assert!(
        matches!(&error, GitError::AmbiguousRev { spec } if spec == &prefix),
        "{error:?}"
    );

    let short = revision::resolve_rev(&repo.root, &prefix[..2])
        .expect_err("two characters cannot possibly be unique here either");
    assert!(matches!(short, GitError::AmbiguousRev { .. }), "{short:?}");
}
