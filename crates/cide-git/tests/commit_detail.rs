//! One commit's contents, against real repositories, with `git show --numstat` as the oracle.
//!
//! Two separate claims are under test and they pull in opposite directions.
//!
//! **The numbers are right.** `cide_git::show` counts with `Patch::from_diff` per delta rather
//! than with `Diff::stats()`, and a per-delta count is exactly the kind of thing that is off by
//! one on a file with no trailing newline, or that quietly counts a rename's whole content twice.
//! So the counts are asserted against the binary, over seeded random working trees — the same
//! discipline `tests/patch_props.rs` applies to patch synthesis.
//!
//! **The cost is bounded.** Every gate in that module exists to keep a four-thousand-file merge
//! from generating four thousand patches, and a gate is only observable by its *effect on the
//! output*: `NotCounted` where nothing was counted, `Binary` where there was nothing to count,
//! `TooLarge` with the number in it, and a `files` total that stays honest while the wire list is
//! truncated. Those are the assertions; the wall-clock ceiling below is a smoke bound, not a
//! benchmark.

mod fastimport;
mod support;

use cide_git::repo as repo_mod;
use cide_git::show::{self, DetailLimits};
use cide_ipc::git::{FileState, GitError};
use cide_ipc::history::{DiffAgainst, LineCount};
use fastimport::{Import, Op};
use support::{Rng, TempRepo};

// --- what it was diffed against -------------------------------------------------------------------

#[test]
fn a_root_commit_is_diffed_against_the_empty_tree() {
    let repo = TempRepo::new("detail-root");
    let mut import = Import::new();
    import.commit(
        "main",
        0,
        "the first commit",
        None,
        None,
        &[
            Op::Set("100644", "a", "one\n"),
            Op::Set("100644", "b", "two\n"),
        ],
    );
    import.run(&repo);

    let detail = show::detail(&repo.root, "HEAD", None, DetailLimits::default()).expect("detail");
    assert_eq!(detail.against, DiffAgainst::EmptyTree);
    assert!(!detail.merge);
    assert_eq!(detail.total.files, 2);
    assert!(detail.files.iter().all(|f| f.status == FileState::Added));
    assert_eq!(detail.commit.summary, "the first commit");
    assert_eq!(
        detail.message, "the first commit",
        "the **full** message, summary line included"
    );
}

#[test]
fn a_merge_defaults_to_its_first_parent_and_can_be_asked_for_another() {
    let repo = TempRepo::new("detail-merge");
    let mut import = Import::new();
    let base = import.commit(
        "main",
        0,
        "base",
        None,
        None,
        &[Op::Set("100644", "f", "base\n")],
    );
    let left = import.commit(
        "main",
        60,
        "left",
        Some(base),
        None,
        &[Op::Set("100644", "f", "left\n")],
    );
    let right = import.commit(
        "side",
        120,
        "right",
        Some(base),
        None,
        &[Op::Set("100644", "g", "right\n")],
    );
    import.commit(
        "main",
        180,
        "merge",
        Some(left),
        Some(right),
        &[Op::Set("100644", "g", "right\n")],
    );
    import.run(&repo);

    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let first = repo.git(&["rev-parse", "HEAD^1"]).trim().to_string();
    let second = repo.git(&["rev-parse", "HEAD^2"]).trim().to_string();

    let detail = show::detail(&repo.root, &head, None, DetailLimits::default()).expect("detail");
    assert!(detail.merge);
    assert_eq!(
        detail.against,
        DiffAgainst::Parent {
            index: 0,
            oid: first
        },
        "git show's own default, and the only one that makes a merge readable"
    );
    let paths: Vec<&str> = detail.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["g"], "against the mainline, only `g` moved");

    let detail = show::detail(&repo.root, &head, Some(1), DetailLimits::default()).expect("detail");
    assert_eq!(
        detail.against,
        DiffAgainst::Parent {
            index: 1,
            oid: second
        }
    );
    let paths: Vec<&str> = detail.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["f"],
        "against the other side, `f` is what arrived"
    );
}

#[test]
fn a_parent_index_past_the_end_is_refused_rather_than_clamped() {
    // Clamping would show a diff against a *different* parent under a label saying otherwise,
    // which is a lie the reader has no way to catch.
    let repo = TempRepo::new("detail-bad-parent");
    let mut import = Import::new();
    import.commit(
        "main",
        0,
        "only",
        None,
        None,
        &[Op::Set("100644", "f", "x\n")],
    );
    let a = import.commit(
        "main",
        60,
        "second",
        Some(1),
        None,
        &[Op::Set("100644", "f", "y\n")],
    );
    let _ = a;
    import.run(&repo);

    let refused = show::detail(&repo.root, "HEAD", Some(3), DetailLimits::default());
    assert!(
        matches!(refused, Err(GitError::NoSuchCommit { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_revspec_that_is_not_a_commit_is_told_apart_from_one_that_does_not_resolve() {
    let repo = TempRepo::new("detail-refusals");
    let mut import = Import::new();
    import.commit(
        "main",
        0,
        "only",
        None,
        None,
        &[Op::Set("100644", "f", "x\n")],
    );
    import.run(&repo);

    match show::detail(&repo.root, "HEAD^{tree}", None, DetailLimits::default()) {
        Err(GitError::NotACommit { kind, .. }) => assert_eq!(kind, "tree"),
        other => panic!("expected NotACommit, got {other:?}"),
    }
    assert!(matches!(
        show::detail(&repo.root, "nope-nope-nope", None, DetailLimits::default()),
        Err(GitError::NoSuchCommit { .. })
    ));
}

#[test]
fn a_rename_carries_its_old_path() {
    let repo = TempRepo::new("detail-rename");
    let body: String = (0..40).map(|line| format!("line {line}\n")).collect();
    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "create",
        None,
        None,
        &[Op::Set("100644", "old.txt", &body)],
    );
    import.commit(
        "main",
        60,
        "rename",
        Some(a),
        None,
        &[Op::Delete("old.txt"), Op::Set("100644", "new.txt", &body)],
    );
    import.run(&repo);

    let detail = show::detail(&repo.root, "HEAD", None, DetailLimits::default()).expect("detail");
    assert_eq!(detail.files.len(), 1, "{:?}", detail.files);
    assert_eq!(detail.files[0].status, FileState::Renamed);
    assert_eq!(detail.files[0].path, "new.txt");
    assert_eq!(detail.files[0].old_path.as_deref(), Some("old.txt"));
    assert_eq!(
        detail.files[0].lines,
        LineCount::Counted {
            added: 0,
            deleted: 0
        }
    );
}

// --- the gates -----------------------------------------------------------------------------------------

#[test]
fn a_binary_file_is_binary_and_not_zero_over_zero() {
    // `0/0` is a real answer — a mode change, an empty file — so it cannot also mean "there was
    // nothing to count". `git --numstat` draws the distinction with a dash; this is the dash.
    let repo = TempRepo::new("detail-binary");
    let mut rng = Rng::new(7);
    let before = support::binary_blob(&mut rng, 4_096);
    let mut after = before.clone();
    after[0] ^= 0xff;
    after[1] = 0; // a NUL in the first 8000 is what makes both git and libgit2 say binary

    let mut import = Import::new();
    let a = import.commit_bytes("main", 0, "create", None, &[("blob.bin", &before)]);
    import.commit_bytes("main", 60, "change", Some(a), &[("blob.bin", &after)]);
    import.run(&repo);

    let detail = show::detail(&repo.root, "HEAD", None, DetailLimits::default()).expect("detail");
    assert_eq!(detail.files.len(), 1);
    assert!(detail.files[0].binary);
    assert_eq!(detail.files[0].lines, LineCount::Binary);
    assert!(
        detail.total.partial,
        "a total that cannot be complete has to say so, or it states a number that is wrong"
    );

    let numstat = repo.git(&["show", "--numstat", "--format=", "HEAD"]);
    assert!(
        numstat.starts_with("-\t-\t"),
        "and this is what git prints for it: {numstat:?}"
    );
}

#[test]
fn an_oversized_blob_says_how_big_it_is_rather_than_calling_itself_binary() {
    let repo = TempRepo::new("detail-too-large");
    let small: String = (0..10).map(|line| format!("line {line}\n")).collect();
    let big: String = (0..2_000)
        .map(|line| format!("a fairly long line number {line}\n"))
        .collect();
    assert!(big.len() > 4_096);

    let mut import = Import::new();
    let a = import.commit(
        "main",
        0,
        "create",
        None,
        None,
        &[
            Op::Set("100644", "f", &small),
            Op::Set("100644", "g", &small),
        ],
    );
    import.commit(
        "main",
        60,
        "grow f",
        Some(a),
        None,
        &[Op::Set("100644", "f", &big)],
    );
    import.run(&repo);

    let limits = DetailLimits {
        count_bytes: 4_096,
        ..DetailLimits::default()
    };
    let detail = show::detail(&repo.root, "HEAD", None, limits).expect("detail");
    assert_eq!(detail.files.len(), 1);
    match detail.files[0].lines {
        LineCount::TooLarge { bytes } => {
            assert!(bytes > 4_096, "the number the row shows: {bytes}")
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
    assert!(detail.total.partial);
}

#[test]
fn a_four_thousand_file_commit_returns_fast_and_counts_nothing() {
    // The acceptance test for the cost control. Four thousand deltas is over `MAX_COUNT_FILES`, so
    // **no patch is generated at all** and the call costs one tree-to-tree diff. The observable
    // consequence is `NotCounted` everywhere; the clock below is a smoke bound in case someone
    // reintroduces `Diff::stats()`, which would take tens of seconds here.
    let repo = TempRepo::new("detail-huge");
    let mut import = Import::new();
    let mut first: Vec<(String, String)> = Vec::new();
    for index in 0..4_000 {
        first.push((
            format!("dir{}/file{index}.txt", index % 40),
            format!("v1 {index}\n"),
        ));
    }
    let ops: Vec<Op<'_>> = first
        .iter()
        .map(|(path, body)| Op::Set("100644", path, body))
        .collect();
    let root = import.commit("main", 0, "create everything", None, None, &ops);
    drop(ops);
    let second: Vec<(String, String)> = first
        .iter()
        .map(|(path, _)| (path.clone(), format!("v2 {path}\n")))
        .collect();
    let ops: Vec<Op<'_>> = second
        .iter()
        .map(|(path, body)| Op::Set("100644", path, body))
        .collect();
    import.commit("main", 60, "touch everything", Some(root), None, &ops);
    drop(ops);
    import.run(&repo);

    let started = std::time::Instant::now();
    let detail = show::detail(&repo.root, "HEAD", None, DetailLimits::default()).expect("detail");
    let elapsed = started.elapsed();

    assert_eq!(detail.total.files, 4_000, "the total stays honest");
    assert_eq!(
        detail.files.len(),
        show::COMMIT_FILE_CAP,
        "the wire list is capped"
    );
    assert!(detail.files_truncated);
    assert!(
        detail
            .files
            .iter()
            .all(|f| f.lines == LineCount::NotCounted),
        "nothing was counted, and nothing was diffed to find that out"
    );
    assert!(detail.total.partial);
    assert_eq!(detail.total.added, 0);
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "one tree diff, not four thousand patches: {elapsed:?}"
    );

    // And the lazy pass fills in exactly what was asked for, and nothing else.
    let wanted: Vec<String> = detail.files[..5].iter().map(|f| f.path.clone()).collect();
    let counts = show::line_counts(&repo.root, "HEAD", None, &wanted, DetailLimits::lazy())
        .expect("line counts");
    assert!(!counts.deadline_hit);
    assert_eq!(counts.files.len(), show::MAX_DETAIL_FILES.min(4_000));
    let filled: Vec<&str> = counts
        .files
        .iter()
        .filter(|f| matches!(f.lines, LineCount::Counted { .. }))
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(
        filled,
        wanted.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(counts.total.added, 5, "one added line each");
    assert!(
        counts.total.partial,
        "3995 files are still uncounted, so the totals are a lower bound"
    );
}

#[test]
fn the_wire_cap_never_lies_about_how_many_files_there_were() {
    let repo = TempRepo::new("detail-wire-cap");
    let mut import = Import::new();
    let bodies: Vec<(String, String)> = (0..50)
        .map(|index| (format!("f{index}"), format!("v1 {index}\n")))
        .collect();
    let ops: Vec<Op<'_>> = bodies
        .iter()
        .map(|(path, body)| Op::Set("100644", path, body))
        .collect();
    import.commit("main", 0, "fifty files", None, None, &ops);
    drop(ops);
    import.run(&repo);

    let limits = DetailLimits {
        wire_files: 10,
        ..DetailLimits::default()
    };
    let detail = show::detail(&repo.root, "HEAD", None, limits).expect("detail");
    assert_eq!(detail.files.len(), 10);
    assert!(detail.files_truncated);
    assert_eq!(detail.total.files, 50);
    assert_eq!(
        detail.total.added, 50,
        "the totals cover every file, not only the ones on the wire"
    );
}

// --- the oracle ------------------------------------------------------------------------------------------

/// Every line begins with the path, so no two generated files are similar enough for either
/// implementation to call a delete-plus-add a rename. That matters because git's `--numstat`
/// prints a rename as `{old => new}` on one row and libgit2 would produce one delta for it, and a
/// test that tripped over that would be testing rename formatting rather than counting.
fn lines(path: &str, rng: &mut Rng, count: usize, trailing: bool) -> String {
    let mut out = String::new();
    for index in 0..count {
        out.push_str(&format!("{path} {index:04} {}\n", rng.next() % 100_000));
    }
    if !trailing && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Insert, rewrite and delete lines. Runs of untouched lines are what produce hunks with context,
/// and a diff with no context is not the diff this code meets in practice.
fn mutate(path: &str, rng: &mut Rng, original: &str, trailing: bool) -> String {
    let mut rows: Vec<String> = original.lines().map(str::to_owned).collect();
    for _ in 0..1 + rng.below(5) {
        if rows.is_empty() {
            rows.push(format!("{path} new"));
            continue;
        }
        let at = rng.below(rows.len());
        match rng.below(3) {
            0 => rows.insert(at, format!("{path} inserted {}", rng.next() % 1_000)),
            1 => {
                rows.remove(at);
            }
            _ => rows[at] = format!("{path} rewritten {}", rng.next() % 1_000),
        }
    }
    let mut out = rows.join("\n");
    if trailing && !out.is_empty() {
        out.push('\n');
    }
    out
}

/// `git show --numstat` as a map from path to `(added, deleted)`, with `None` for a binary file.
fn numstat(repo: &TempRepo, rev: &str) -> Vec<(String, Option<(u32, u32)>)> {
    repo.git(&["show", "--numstat", "--format=", rev])
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let added = parts.next().unwrap_or_default();
            let deleted = parts.next().unwrap_or_default();
            let path = parts.next().unwrap_or_default().to_string();
            assert!(
                !path.contains(" => "),
                "the fixture is supposed to make renames impossible: {line}"
            );
            let counts = match (added.parse::<u32>(), deleted.parse::<u32>()) {
                (Ok(added), Ok(deleted)) => Some((added, deleted)),
                _ => None,
            };
            (path, counts)
        })
        .collect()
}

#[test]
fn the_counts_are_what_git_show_numstat_prints() {
    for seed in 0..16u64 {
        let mut rng = Rng::new(seed);
        let repo = TempRepo::new(&format!("detail-props-{seed}"));

        let count = 1 + rng.below(6);
        let paths: Vec<String> = (0..count)
            .map(|index| format!("src/f{index}.txt"))
            .collect();
        let mut contents: Vec<String> = Vec::with_capacity(count);
        for path in &paths {
            let rows = 5 + rng.below(30);
            let trailing = rng.chance(3, 4);
            contents.push(lines(path, &mut rng, rows, trailing));
        }

        let mut import = Import::new();
        let ops: Vec<Op<'_>> = paths
            .iter()
            .zip(contents.iter())
            .map(|(path, body)| Op::Set("100644", path, body))
            .collect();
        let root = import.commit("main", 0, "create", None, None, &ops);
        drop(ops);

        // One commit that touches a random subset, and never deletes a whole file — see `lines`.
        let touched: Vec<usize> = (0..count).filter(|_| rng.chance(2, 3)).collect();
        let touched = if touched.is_empty() { vec![0] } else { touched };
        for &index in &touched {
            let trailing = rng.chance(3, 4);
            let next = mutate(&paths[index], &mut rng, &contents[index], trailing);
            contents[index] = next;
        }
        let ops: Vec<Op<'_>> = touched
            .iter()
            .map(|&index| Op::Set("100644", &paths[index], &contents[index]))
            .collect();
        import.commit("main", 60, "mutate", Some(root), None, &ops);
        drop(ops);
        import.run(&repo);

        for rev in ["HEAD~1", "HEAD"] {
            let detail = show::detail(&repo.root, rev, None, DetailLimits::default())
                .unwrap_or_else(|e| panic!("seed {seed}, {rev}: {e:?}"));
            let ours: Vec<(String, Option<(u32, u32)>)> = detail
                .files
                .iter()
                .map(|file| {
                    let counts = match file.lines {
                        LineCount::Counted { added, deleted } => Some((added, deleted)),
                        LineCount::Binary => None,
                        other => panic!("seed {seed}: unexpected {other:?} for {}", file.path),
                    };
                    (file.path.clone(), counts)
                })
                .collect();
            let mut ours = ours;
            let mut theirs = numstat(&repo, rev);
            ours.sort();
            theirs.sort();
            assert_eq!(ours, theirs, "seed {seed}, {rev}");

            let expected_added: u32 = theirs.iter().filter_map(|(_, c)| c.map(|(a, _)| a)).sum();
            let expected_deleted: u32 = theirs.iter().filter_map(|(_, c)| c.map(|(_, d)| d)).sum();
            assert_eq!(detail.total.added, expected_added, "seed {seed}, {rev}");
            assert_eq!(detail.total.deleted, expected_deleted, "seed {seed}, {rev}");
            assert_eq!(
                detail.total.files as usize,
                theirs.len(),
                "seed {seed}, {rev}"
            );
        }
    }
}

// --- the row the list drew ---------------------------------------------------------------------------------

#[test]
fn the_detail_carries_the_same_row_the_log_drew() {
    // `CommitDetail::commit` is a whole `CommitRow` precisely so the pane and the list cannot
    // disagree about a summary, an abbreviation or a ref chip. Two spellings of "abbreviate an
    // oid" is how they start to.
    let repo = TempRepo::new("detail-row");
    let mut import = Import::new();
    import.commit(
        "main",
        0,
        "the only commit",
        None,
        None,
        &[Op::Set("100644", "f", "x\n")],
    );
    import.run(&repo);
    repo.git(&["tag", "-a", "v9", "-m", "release"]);

    let detail = show::detail(&repo.root, "HEAD", None, DetailLimits::default()).expect("detail");
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(detail.commit.oid, head);
    assert_eq!(detail.commit.repo, repo_mod::repo_id(&repo.root));
    assert_eq!(
        detail.commit.short_oid,
        repo.git(&["rev-parse", "--short", "HEAD"]).trim()
    );
    let chips: Vec<&str> = detail.commit.refs.iter().map(|c| c.name.as_str()).collect();
    assert!(chips.contains(&"v9"), "{chips:?}");
    assert_eq!(detail.committer, "cide tests");
    assert_eq!(detail.committer_email, "tests@cide.invalid");
}
