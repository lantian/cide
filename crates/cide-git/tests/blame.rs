//! Blame, against real repositories and with the real `git` binary as the oracle.
//!
//! Blame is the one surface in this crate where a wrong answer looks exactly like a right one.
//! A staging bug loses work loudly; a blame that is one line off paints a full gutter of real
//! commits with real authors, every line labelled, nothing empty, nothing red — and the only way
//! to notice is to already know the answer. So the tests here are of two kinds and both are
//! necessary:
//!
//! * **differential** — every line's attribution compared against `git blame --porcelain`, which
//!   is the definition of the right answer and is not a second model of it written by us;
//! * **structural** — [`cide_git::blame::check_runs`]'s invariant asserted directly on the output
//!   of every route, because the differential test only covers the shapes the fixtures happen to
//!   produce and a hole is exactly the thing a fixture will not think to produce.
//!
//! The `-M`/`-C` tests need the `git` binary and nothing else — no network, no account, no quota
//! — so they are not `#[ignore]`d. CI has git; it is what the rest of this directory already
//! shells out to.

mod support;

use cide_git::blame::{self, MAX_BLAME_BYTES};
use cide_ipc::git::GitError;
use cide_ipc::history::{BlameFile, BlameFollow, BlameRequest, BlameSource};
use support::TempRepo;

// --- fixtures ---------------------------------------------------------------------------------

/// Four commits over one file, each touching a different part of it.
///
/// Deliberately interleaved rather than appended: an append-only history blames every line to
/// the commit that added it in file order, so the runs come out ascending for free and a walker
/// that emitted them in the wrong order would still pass. Editing the middle and then the end
/// forces the hunks out of chronological order.
fn layered(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("src.txt", b"one\ntwo\nthree\nfour\nfive\nsix\n");
    repo.commit_all("first");
    repo.write("src.txt", b"one\nTWO\nthree\nfour\nfive\nsix\n");
    repo.commit_all("second");
    repo.write("src.txt", b"one\nTWO\nthree\nfour\nFIVE\nsix\n");
    repo.commit_all("third");
    repo.write("src.txt", b"one\nTWO\nTHREE\nfour\nFIVE\nsix\nseven\n");
    repo.commit_all("fourth");
    repo
}

fn plain() -> BlameRequest {
    BlameRequest::default()
}

// --- helpers ----------------------------------------------------------------------------------

/// The run-length encoding expanded back to one entry per line, `None` for uncommitted.
///
/// Everything differential compares this rather than the runs, because the runs are a *encoding*
/// of the answer and git's porcelain output is a different encoding of the same answer. Comparing
/// encodings would fail on a boundary that both agree about.
fn per_line(file: &BlameFile) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for run in &file.runs {
        for _ in 0..run.lines {
            out.push(
                run.commit
                    .map(|index| file.commits[index as usize].oid.clone()),
            );
        }
    }
    out
}

/// `git blame --porcelain`, parsed into one oid per line.
///
/// A second, deliberately naive parser: it is the oracle, so sharing
/// `cide_git::blame`'s own parser would make the comparison circular. It only has to handle the
/// two things this file asserts about — the group headers and the content lines.
fn oracle(repo: &TempRepo, args: &[&str], stdin: &[u8]) -> Vec<Option<String>> {
    let mut full = vec!["blame", "--porcelain"];
    full.extend_from_slice(args);
    let (ok, text) = repo.try_git_stdin(&full, stdin);
    assert!(ok, "git {full:?} failed:\n{text}");

    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        if let Some(stripped) = line.strip_prefix('\t') {
            let _ = stripped;
            let oid = pending.take().expect("a content line inside a group");
            out.push(if oid.bytes().all(|b| b == b'0') {
                None
            } else {
                Some(oid)
            });
            continue;
        }
        if pending.is_none() {
            let head = line.split(' ').next().unwrap_or_default();
            if head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()) {
                pending = Some(head.to_string());
            }
        }
    }
    out
}

/// The whole structural contract, asserted on every answer this file produces.
///
/// Called from every test rather than from one, because the shape that breaks it is produced by
/// a *particular* history and there is no single fixture that covers them all.
fn assert_well_formed(file: &BlameFile) {
    let mut expected = 1u32;
    for (index, run) in file.runs.iter().enumerate() {
        assert!(
            run.lines > 0,
            "run {index} of {:?} covers no lines",
            file.path
        );
        assert_eq!(
            run.start, expected,
            "run {index} starts at {} where {expected} was expected: {:?}",
            run.start, file.runs
        );
        expected += run.lines;
    }
    assert_eq!(
        expected - 1,
        file.lines,
        "the runs must cover exactly 1..={} : {:?}",
        file.lines,
        file.runs
    );
    // The run-collapsing rule. Two adjacent runs naming one commit is not wrong, only wasteful —
    // but it is wasteful *per run*, and a 5000-line file is where it is measured, so the rule is
    // asserted rather than assumed.
    for pair in file.runs.windows(2) {
        assert_ne!(
            pair[0].commit, pair[1].commit,
            "adjacent runs share a commit and were not merged: {:?}",
            file.runs
        );
    }
    // The table is a set. A duplicate would mean the dedupe map missed, which costs the payload
    // one full commit record per extra run of the same commit.
    let mut oids: Vec<&str> = file.commits.iter().map(|c| c.oid.as_str()).collect();
    let before = oids.len();
    oids.sort_unstable();
    oids.dedup();
    assert_eq!(before, oids.len(), "the commit table has duplicates");
    for run in &file.runs {
        if let Some(index) = run.commit {
            assert!(
                (index as usize) < file.commits.len(),
                "a run indexes past the commit table"
            );
        }
    }
}

// --- the differential core --------------------------------------------------------------------

#[test]
fn every_line_is_attributed_the_way_git_attributes_it() {
    let repo = layered("attribution");
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.lines, 7);
    assert_eq!(file.source, BlameSource::Head);
    assert!(!file.dirty);
    assert_eq!(file.follow, BlameFollow::Renames);
    assert_eq!(file.downgraded, None);

    assert_eq!(per_line(&file), oracle(&repo, &["--", "src.txt"], &[]));
}

#[test]
fn the_summary_author_and_time_come_from_the_commit_git_names() {
    let repo = layered("fields");
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");

    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let fourth = file
        .commits
        .iter()
        .find(|commit| commit.oid == head)
        .expect("the newest commit is in the table");
    assert_eq!(fourth.summary, "fourth");
    assert_eq!(fourth.author, "cide tests");
    assert_eq!(fourth.author_email, "tests@cide.invalid");
    assert_eq!(fourth.short_oid, head[..8]);
    assert!(fourth.authored > 0, "author time is filled in");
    assert_eq!(fourth.orig_path, None, "the path has not changed");
}

#[test]
fn ignoring_whitespace_reattributes_a_reindented_line() {
    let repo = TempRepo::new("whitespace");
    repo.write("src.txt", b"alpha\nbravo\ncharlie\n");
    repo.commit_all("first");
    repo.write("src.txt", b"    alpha\n    bravo\n    charlie\n");
    repo.commit_all("reindent");

    let first = repo.git(&["rev-parse", "HEAD~1"]).trim().to_string();
    let with = BlameRequest {
        ignore_whitespace: true,
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "src.txt", None, &with).expect("blame");
    assert_well_formed(&file);
    assert_eq!(
        per_line(&file),
        vec![Some(first.clone()), Some(first.clone()), Some(first)],
        "-w must credit the original author, not the reformat"
    );

    let without = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_ne!(
        per_line(&without),
        per_line(&file),
        "and the option must actually change the answer, or it is not being applied"
    );
}

#[test]
fn first_parent_agrees_with_the_binary() {
    let repo = merged("first-parent");
    let request = BlameRequest {
        first_parent: true,
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "shared.txt", None, &request).expect("blame");
    assert_well_formed(&file);
    assert_eq!(
        per_line(&file),
        oracle(&repo, &["--first-parent", "--", "shared.txt"], &[])
    );
}

// --- the dirty buffer ----------------------------------------------------------------------

#[test]
fn a_supplied_buffer_leaves_new_lines_unattributed_and_shifts_the_rest() {
    let repo = layered("buffer-insert");
    let clean = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");

    // Two lines inserted at the top. Every original line keeps its commit and moves down by two,
    // which is the whole reason the backend does this rather than the editor guessing.
    let buffer = b"NEW A\nNEW B\none\nTWO\nTHREE\nfour\nFIVE\nsix\nseven\n";
    let file = blame::blame(&repo.root, "src.txt", Some(buffer), &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.source, BlameSource::Buffer);
    assert!(file.dirty);
    assert_eq!(file.lines, 9);

    let lines = per_line(&file);
    assert_eq!(lines[0], None, "an inserted line belongs to no commit");
    assert_eq!(lines[1], None);
    assert_eq!(
        &lines[2..],
        &per_line(&clean)[..],
        "the untouched lines keep their commits, at shifted line numbers"
    );
    assert_eq!(
        lines,
        oracle(&repo, &["--contents", "-", "--", "src.txt"], buffer)
    );
}

#[test]
fn the_runs_stay_total_after_an_unsaved_deletion() {
    let repo = layered("buffer-delete");
    // `three` and `four` removed from the middle; the two surviving halves of what was one
    // commit's span become adjacent and must be merged rather than left as two runs with a
    // boundary that no longer exists.
    let buffer = b"one\nTWO\nFIVE\nsix\nseven\n";
    let file = blame::blame(&repo.root, "src.txt", Some(buffer), &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.lines, 5);
    assert_eq!(
        per_line(&file),
        oracle(&repo, &["--contents", "-", "--", "src.txt"], buffer)
    );
    // The case that decides how `dirty` is computed. A buffer whose only edit is a deletion has
    // **no** uncommitted line — every surviving line is still exactly some commit's — so a
    // `dirty` derived from the runs would say this gutter is a picture of HEAD, and it is not.
    assert!(
        file.runs.iter().all(|run| run.commit.is_some()),
        "nothing was added, so nothing is unattributed: {:?}",
        file.runs
    );
    assert!(file.dirty, "and yet the blamed bytes are not HEAD's");
}

#[test]
fn a_buffer_identical_to_head_is_not_dirty() {
    let repo = layered("buffer-clean");
    let same = b"one\nTWO\nTHREE\nfour\nFIVE\nsix\nseven\n";
    let file = blame::blame(&repo.root, "src.txt", Some(same), &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(
        file.source,
        BlameSource::Buffer,
        "we blamed what we were given"
    );
    assert!(!file.dirty, "and nothing in it is uncommitted");
    assert!(file.runs.iter().all(|run| run.commit.is_some()));
}

#[test]
fn a_whole_file_rewritten_in_the_buffer_is_one_uncommitted_run() {
    let repo = layered("buffer-rewrite");
    let buffer = b"nothing\nlike\nthe\noriginal\n";
    let file = blame::blame(&repo.root, "src.txt", Some(buffer), &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.lines, 4);
    assert!(file.dirty);
    assert!(
        file.runs.iter().all(|run| run.commit.is_none()),
        "no line of this buffer is in any commit: {:?}",
        file.runs
    );
}

// --- which version was blamed -----------------------------------------------------------------

#[test]
fn a_dirty_working_file_is_the_working_source() {
    let repo = layered("source-working");
    repo.write(
        "src.txt",
        b"one\nTWO\nTHREE\nfour\nFIVE\nsix\nseven\nEIGHT\n",
    );
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.source, BlameSource::Working);
    assert!(file.dirty);
    assert_eq!(file.lines, 8);
    assert_eq!(per_line(&file).last().unwrap().as_ref(), None);
}

#[test]
fn a_clean_working_file_is_the_head_source() {
    let repo = layered("source-head");
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_eq!(file.source, BlameSource::Head);
    assert!(!file.dirty);
}

#[test]
fn a_file_deleted_from_the_worktree_still_blames_at_head() {
    let repo = layered("source-deleted");
    repo.remove("src.txt");
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.source, BlameSource::Head);
    assert_eq!(file.lines, 7);
}

#[test]
fn newest_blames_the_file_as_it_was_at_that_revision() {
    let repo = layered("source-revision");
    let second = repo.git(&["rev-parse", "HEAD~2"]).trim().to_string();
    let request = BlameRequest {
        newest: Some(second.clone()),
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "src.txt", None, &request).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.source, BlameSource::Revision);
    assert_eq!(file.head, second);
    assert_eq!(file.lines, 6, "the seventh line did not exist yet");
    assert_eq!(
        per_line(&file),
        oracle(&repo, &[&second, "--", "src.txt"], &[])
    );
}

// --- the refusals -------------------------------------------------------------------------

#[test]
fn an_untracked_path_is_refused_as_not_tracked() {
    let repo = layered("untracked");
    repo.write("scratch.txt", b"never committed\n");
    let error = blame::blame(&repo.root, "scratch.txt", None, &plain())
        .expect_err("a file git has never seen has no blame");
    assert!(
        matches!(&error, GitError::NotTracked { path } if path == "scratch.txt"),
        "{error:?}"
    );
}

#[test]
fn a_directory_is_refused_as_not_tracked() {
    let repo = TempRepo::new("directory");
    repo.write("pkg/one.txt", b"a\n");
    repo.commit_all("first");
    let error = blame::blame(&repo.root, "pkg", None, &plain()).expect_err("a directory");
    assert!(matches!(error, GitError::NotTracked { .. }), "{error:?}");
}

#[test]
fn an_unborn_repository_is_refused_as_unborn() {
    let repo = TempRepo::new("unborn");
    repo.write("src.txt", b"a\n");
    let error = blame::blame(&repo.root, "src.txt", None, &plain()).expect_err("no commits yet");
    assert!(matches!(error, GitError::Unborn), "{error:?}");
}

/// The cap is exercised through `blame_with` rather than by generating two megabytes: the
/// assertion is about the refusal carrying honest numbers, and a real 2 MiB file would make the
/// suite slower and the assertion no stronger.
#[test]
fn a_file_past_the_cap_is_refused_with_both_numbers() {
    let repo = layered("too-large");
    let error = blame::blame_with(&repo.root, "src.txt", None, &plain(), 4)
        .expect_err("a four-byte cap refuses everything");
    match error {
        GitError::FileTooLarge { path, bytes, limit } => {
            assert_eq!(path, "src.txt");
            assert_eq!(limit, 4);
            assert!(bytes > 4, "the real size, not the cap: {bytes}");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        blame::blame_with(&repo.root, "src.txt", None, &plain(), MAX_BLAME_BYTES).is_ok(),
        "and the same file is fine under the real cap"
    );
}

#[test]
fn the_cap_measures_the_buffer_when_one_is_given() {
    let repo = layered("too-large-buffer");
    let big = vec![b'x'; 512];
    let error = blame::blame_with(&repo.root, "src.txt", Some(&big), &plain(), 100)
        .expect_err("the buffer is what would be blamed");
    match error {
        GitError::FileTooLarge { bytes, .. } => assert_eq!(bytes, 512),
        other => panic!("{other:?}"),
    }
}

// --- the things that only a particular repository shows -----------------------------------

/// The assertion that proves rename following is switched on.
///
/// It is the one test that fails the day somebody reads the module header's *"the four
/// `track_copies_*` setters do nothing"* and concludes that the blame options are therefore
/// pointless and tidies them away — because `orig_path` comes from libgit2's unconditional
/// `find_similar` and survives, while `use_mailmap` and the rest do not. Losing it would leave
/// *Open the file as it was then* asking for a path that did not exist yet.
#[test]
fn orig_path_is_set_for_lines_written_before_a_git_mv() {
    let repo = TempRepo::new("rename");
    repo.write("old.txt", b"alpha\nbravo\ncharlie\n");
    repo.commit_all("first");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.git(&["commit", "-q", "-m", "rename"]);
    repo.write("new.txt", b"alpha\nbravo\ncharlie\ndelta\n");
    repo.commit_all("extend");

    let file = blame::blame(&repo.root, "new.txt", None, &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.lines, 4);

    let first = repo.git(&["rev-parse", "HEAD~2"]).trim().to_string();
    let before = file
        .commits
        .iter()
        .find(|commit| commit.oid == first)
        .expect("the pre-rename commit is still reachable through the blame");
    assert_eq!(
        before.orig_path.as_deref(),
        Some("old.txt"),
        "the pre-rename spelling has to survive, or the rename was not followed"
    );

    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let latest = file
        .commits
        .iter()
        .find(|commit| commit.oid == head)
        .expect("the newest commit is in the table");
    assert_eq!(
        latest.orig_path, None,
        "and a commit that used today's name carries no second path"
    );
}

#[test]
fn a_mailmap_rewrites_the_author_the_way_git_does() {
    let repo = TempRepo::new("mailmap");
    repo.git(&["config", "user.name", "Old Name"]);
    repo.git(&["config", "user.email", "old@example.invalid"]);
    repo.write("src.txt", b"alpha\nbravo\n");
    repo.commit_all("first");
    repo.write(
        ".mailmap",
        b"New Name <new@example.invalid> <old@example.invalid>\n",
    );
    repo.commit_all("mailmap");

    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_well_formed(&file);
    let first = &file.commits[0];
    assert_eq!(
        first.author, "New Name",
        "use_mailmap(true) is what keeps this identical to `git blame`"
    );
    assert_eq!(first.author_email, "new@example.invalid");
}

#[test]
fn the_root_commit_is_a_boundary() {
    let repo = layered("boundary-root");
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    let first = repo.git(&["rev-parse", "HEAD~3"]).trim().to_string();
    let root = file
        .commits
        .iter()
        .find(|commit| commit.oid == first)
        .expect("the first commit still owns a line");
    assert!(
        root.boundary,
        "the walk stopped there because there is no more history"
    );
}

/// A shallow clone's floor is not authorship, and the flag is the only thing that says so.
///
/// Not `#[ignore]`d: `file://` needs no network, only the `git` binary, which every other test in
/// this directory already requires.
#[test]
fn a_shallow_clone_marks_its_floor_as_a_boundary() {
    let origin = layered("shallow-origin");
    // The clone lives inside a second `TempRepo` purely so its `Drop` cleans it up; the outer
    // repository is never used.
    let host = TempRepo::new("shallow-host");
    let url = format!("file://{}", origin.root.display());
    host.git(&["clone", "--quiet", "--depth", "1", &url, "clone"]);
    let clone = host.root.join("clone");

    let file = blame::blame(&clone, "src.txt", None, &plain()).expect("blame in a shallow clone");
    assert_well_formed(&file);
    assert_eq!(file.lines, 7);
    assert!(
        file.commits.iter().all(|commit| commit.boundary),
        "every commit a depth-1 clone can see is its floor: {:?}",
        file.commits
    );
    assert_eq!(
        file.commits.len(),
        1,
        "and there is exactly one, because nothing above it was fetched"
    );
}

/// A blob full of NULs is still lines to blame, and refusing it would leave the gutter with an
/// error where an ordinary answer would do.
#[test]
fn a_binary_file_degrades_rather_than_aborting() {
    let repo = TempRepo::new("binary");
    repo.write("blob.bin", b"\x00\x01\x02\n\x00\xffbinary\n\x00\n");
    repo.commit_all("first");

    let file = blame::blame(&repo.root, "blob.bin", None, &plain()).expect("a binary blame");
    assert_well_formed(&file);
    assert_eq!(file.lines, 3);
    assert!(file.runs.iter().all(|run| run.commit.is_some()));
}

#[test]
fn a_file_with_no_trailing_newline_still_covers_its_last_line() {
    let repo = TempRepo::new("no-eol");
    repo.write("src.txt", b"alpha\nbravo");
    repo.commit_all("first");
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.lines, 2);
}

#[test]
fn an_empty_file_blames_to_nothing_at_all() {
    let repo = TempRepo::new("empty");
    repo.write("src.txt", b"");
    repo.commit_all("first");
    let file = blame::blame(&repo.root, "src.txt", None, &plain()).expect("blame");
    assert_well_formed(&file);
    assert_eq!(file.lines, 0);
    assert!(file.runs.is_empty());
    assert!(file.commits.is_empty());
}

// --- the `git` binary route ---------------------------------------------------------------

#[test]
fn moved_lines_runs_the_binary_and_says_so() {
    let repo = moved("moved-lines");
    let request = BlameRequest {
        follow: BlameFollow::MovedLines,
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "src.txt", None, &request).expect("blame -M");
    assert_well_formed(&file);
    assert_eq!(
        file.follow,
        BlameFollow::MovedLines,
        "the mode reported must be the one that ran: {:?}",
        file.downgraded
    );
    assert_eq!(file.downgraded, None);
    assert_eq!(
        per_line(&file),
        oracle(&repo, &["-M", "--", "src.txt"], &[])
    );
}

#[test]
fn copies_in_commit_matches_the_binary() {
    let repo = copied("copies-in-commit");
    let request = BlameRequest {
        follow: BlameFollow::CopiesInCommit,
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "copy.txt", None, &request).expect("blame -C");
    assert_well_formed(&file);
    assert_eq!(
        file.follow,
        BlameFollow::CopiesInCommit,
        "{:?}",
        file.downgraded
    );
    assert_eq!(
        per_line(&file),
        oracle(&repo, &["-C", "--", "copy.txt"], &[])
    );
}

#[test]
fn copies_anywhere_matches_the_binary() {
    let repo = copied("copies-anywhere");
    let request = BlameRequest {
        follow: BlameFollow::CopiesAnywhere,
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "copy.txt", None, &request).expect("blame -C -C -C");
    assert_well_formed(&file);
    assert_eq!(
        file.follow,
        BlameFollow::CopiesAnywhere,
        "{:?}",
        file.downgraded
    );
    assert_eq!(
        per_line(&file),
        oracle(&repo, &["-C", "-C", "-C", "--", "copy.txt"], &[])
    );
}

/// `git blame --contents -` and `-C` together, which is the combination the dirty-buffer gutter
/// needs and the one nothing else in this repository proves works.
#[test]
fn the_binary_route_takes_a_dirty_buffer() {
    let repo = copied("binary-buffer");
    let buffer = b"UNSAVED\nalpha\nbravo\ncharlie\ndelta\n";
    let request = BlameRequest {
        follow: BlameFollow::CopiesInCommit,
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "copy.txt", Some(buffer), &request).expect("blame -C");
    assert_well_formed(&file);
    assert_eq!(
        file.follow,
        BlameFollow::CopiesInCommit,
        "-C with --contents - must not have been refused: {:?}",
        file.downgraded
    );
    assert_eq!(file.source, BlameSource::Buffer);
    assert!(file.dirty);
    assert_eq!(
        per_line(&file),
        oracle(&repo, &["-C", "--contents", "-", "--", "copy.txt"], buffer)
    );
}

/// A binary route that refuses degrades to the libgit2 answer and **says** so.
///
/// The refusal is forced with `blame.ignoreRevsFile` pointing at a file that is not there:
/// `git blame` reads that key and exits 128, and libgit2 has never heard of it, so the two routes
/// disagree deterministically. Deliberately **not** by emptying `PATH` — `cargo test` runs these
/// in threads and a process-wide environment change is visible to every other test in the binary,
/// which is exactly how that first attempt turned nine unrelated tests red. Repository-local
/// config affects one repository.
///
/// What is being asserted is the whole point of the field: the answer still arrives, it is the
/// rename-follow answer, `follow` reports *that* and not what was asked for, and the sentence
/// exists. A silently different answer is the failure this design exists to prevent.
#[test]
fn a_binary_route_that_refuses_degrades_with_a_sentence() {
    let repo = copied("downgrade");
    repo.git(&["config", "blame.ignoreRevsFile", "/definitely/not/here"]);
    let request = BlameRequest {
        follow: BlameFollow::CopiesAnywhere,
        ..Default::default()
    };
    let file = blame::blame(&repo.root, "copy.txt", None, &request)
        .expect("a refusal from the binary must not fail the blame");
    assert_well_formed(&file);
    assert_eq!(
        file.follow,
        BlameFollow::Renames,
        "the mode reported is what actually ran"
    );
    let sentence = file
        .downgraded
        .clone()
        .expect("and the downgrade is stated");
    assert!(
        sentence.contains("renames"),
        "the sentence has to say what the user is looking at: {sentence}"
    );
    assert!(
        sentence.contains("object name list"),
        "and it quotes git's own complaint, which is the actionable half: {sentence}"
    );
    // The same repository blames fine in the default mode, so the downgrade is about the mode and
    // not about the repository being broken.
    let plain_answer = blame::blame(&repo.root, "copy.txt", None, &plain()).expect("blame");
    assert_eq!(plain_answer.downgraded, None);
    assert_eq!(per_line(&plain_answer), per_line(&file));
}

// --- parent_of ------------------------------------------------------------------------------

#[test]
fn parent_of_a_root_commit_is_the_end_of_the_walk() {
    let repo = layered("parent-root");
    let first = repo.git(&["rev-parse", "HEAD~3"]).trim().to_string();
    let parent = blame::parent_of(&repo.root, "src.txt", &first).expect("parent_of");
    assert_eq!(parent, None, "a root commit has nowhere to hop to");
}

#[test]
fn parent_of_the_commit_that_introduced_a_file_is_none() {
    let repo = TempRepo::new("parent-introduced");
    repo.write("first.txt", b"a\n");
    repo.commit_all("first");
    repo.write("second.txt", b"b\n");
    repo.commit_all("adds second");

    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(
        blame::parent_of(&repo.root, "second.txt", &head).expect("parent_of"),
        None,
        "the parent exists but does not have this file, which is the end of *its* history"
    );
    // The same commit, a different path: the parent does have `first.txt`, so the hop is real.
    let other = blame::parent_of(&repo.root, "first.txt", &head)
        .expect("parent_of")
        .expect("the parent has this one");
    assert_eq!(other.path, "first.txt");
}

#[test]
fn parent_of_follows_the_rename_at_that_boundary() {
    let repo = TempRepo::new("parent-rename");
    repo.write("old.txt", b"alpha\nbravo\ncharlie\n");
    repo.commit_all("first");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", b"alpha\nBRAVO\ncharlie\n");
    repo.commit_all("rename and edit");

    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let first = repo.git(&["rev-parse", "HEAD~1"]).trim().to_string();
    let parent = blame::parent_of(&repo.root, "new.txt", &head)
        .expect("parent_of")
        .expect("there is a parent");
    assert_eq!(parent.rev, first);
    assert_eq!(parent.short_rev, first[..8]);
    assert_eq!(
        parent.path, "old.txt",
        "carrying today's name across the rename would ask for a path that is not there"
    );
    assert_eq!(parent.summary, "first");
}

/// The merge case, which is the whole reason this is not `rev^`.
///
/// `side.txt` exists only on the branch that was merged in, so the merge's **first** parent has
/// never heard of it. `rev^` names that parent, and a blame there comes back empty with no error
/// at all — a blank pane and nothing to explain it.
#[test]
fn parent_of_a_merge_is_the_parent_that_has_the_file() {
    let repo = merged("parent-merge");
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let first = repo.git(&["rev-parse", "HEAD^1"]).trim().to_string();
    let second = repo.git(&["rev-parse", "HEAD^2"]).trim().to_string();
    assert_ne!(first, second);

    let parent = blame::parent_of(&repo.root, "side.txt", &head)
        .expect("parent_of")
        .expect("the second parent has it");
    assert_eq!(parent.rev, second, "not {first}, which is `rev^`");
    assert_eq!(parent.path, "side.txt");

    // And the file both sides have still hops to the first parent, so the rule is "the parent
    // that has it" and not "always the second".
    let shared = blame::parent_of(&repo.root, "shared.txt", &head)
        .expect("parent_of")
        .expect("both parents have it");
    assert_eq!(shared.rev, first);
}

#[test]
fn parent_of_refuses_a_revision_that_is_not_there() {
    let repo = layered("parent-missing");
    let error = blame::parent_of(&repo.root, "src.txt", "nonesuch").expect_err("no such rev");
    assert!(
        matches!(error, GitError::NoSuchRevision { .. }),
        "{error:?}"
    );
}

// --- more fixtures ------------------------------------------------------------------------

/// `main` and a `side` branch merged into it: `shared.txt` on both, `side.txt` only on the
/// branch.
fn merged(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    // Ten lines, and the two branches edit lines two and nine. Far enough apart that the merge is
    // clean without `-X`: three lines of context on each side of two hunks that are seven apart
    // never overlap, and a fixture that conflicts would be testing the merge driver rather than
    // blame.
    repo.write(
        "shared.txt",
        b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
    );
    repo.commit_all("first");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("side.txt", b"side one\nside two\n");
    repo.write(
        "shared.txt",
        b"one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
    );
    repo.commit_all("on the side");
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "shared.txt",
        b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nNINE\nten\n",
    );
    repo.commit_all("on main");
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    repo
}

/// A commit that moves a block of lines from the top of a file to the bottom, which is exactly
/// what `-M` is for and what a plain blame credits to the move.
fn moved(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write(
        "src.txt",
        b"block one\nblock two\nblock three\nkeep one\nkeep two\n",
    );
    repo.commit_all("first");
    repo.write(
        "src.txt",
        b"keep one\nkeep two\nblock one\nblock two\nblock three\n",
    );
    repo.commit_all("move the block");
    repo
}

/// A commit that copies a block out of one file into a new one, which is what `-C` is for.
fn copied(tag: &str) -> TempRepo {
    let repo = TempRepo::new(tag);
    repo.write("origin.txt", b"alpha\nbravo\ncharlie\ndelta\n");
    repo.commit_all("first");
    repo.write("copy.txt", b"alpha\nbravo\ncharlie\ndelta\n");
    repo.commit_all("copy the block");
    repo
}
