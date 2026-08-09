//! Property tests for patch synthesis, against the real `git`.
//!
//! The claim under test is the one the whole staging design rests on:
//!
//! > For any working-tree state and any selection of hunks or lines, the unified diff
//! > `cide-git` synthesizes produces the same index whether it is applied by
//! > `Repository::apply(ApplyLocation::Index)` or by `git apply --cached`.
//!
//! Both applications start from a byte-identical `.git/index` (snapshotted and restored
//! between them), and the result is read back with `git ls-files --stage` — mode, blob id,
//! stage and path for every entry — by the `git` binary rather than by the library under
//! test, so libgit2 is never the judge of its own work.
//!
//! Cases are generated from a seeded [`Rng`]; the seed is printed on failure, so a bad case is
//! reproduced by re-running rather than by re-rolling. `CIDE_GIT_CASES` overrides the count.

mod support;

use std::collections::BTreeSet;

use cide_git::diff::{self, DiffRequest, RawFile};
use cide_git::patch;
use cide_ipc::git::{DiffSide, GitError, PartialRefusal};
use support::{Eol, Rng, TempRepo, mutate, text};

const PATH: &str = "f.txt";

/// How the working tree got into the state under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A tracked file edited in the working tree.
    Modified,
    /// A file that exists only in the working tree.
    Added,
    /// A tracked file whose every line is being removed.
    Emptied,
}

struct Generated {
    shape: Shape,
    eol: Eol,
    trailing: bool,
}

fn generate(repo: &TempRepo, rng: &mut Rng) -> Generated {
    let eol = if rng.chance(1, 3) { Eol::Crlf } else { Eol::Lf };
    // A file with no terminator on its last line is the case that produces
    // `\ No newline at end of file`, and it is where naive synthesis breaks first.
    let trailing = !rng.chance(1, 3);
    let shape = match rng.below(10) {
        0 | 1 => Shape::Added,
        2 => Shape::Emptied,
        _ => Shape::Modified,
    };

    // The index is set up in-process rather than with `git add`: 500 forks of git would
    // dominate the runtime and the entry libgit2 writes is the entry `git add` writes.
    let git_repo = git2::Repository::open(&repo.root).expect("open");
    let mut index = git_repo.index().expect("index");

    match shape {
        Shape::Added => {
            let _ = index.remove_path(std::path::Path::new(PATH));
            index.write().expect("write index");
            let lines = 2 + rng.below(10);
            repo.write(PATH, &text(rng, lines, eol, trailing));
        }
        Shape::Modified => {
            let lines = 4 + rng.below(20);
            let base = text(rng, lines, eol, trailing);
            repo.write(PATH, &base);
            index.add_path(std::path::Path::new(PATH)).expect("add");
            index.write().expect("write index");
            repo.write(PATH, &mutate(rng, &base, eol, trailing));
        }
        Shape::Emptied => {
            let lines = 3 + rng.below(8);
            let base = text(rng, lines, eol, trailing);
            repo.write(PATH, &base);
            index.add_path(std::path::Path::new(PATH)).expect("add");
            index.write().expect("write index");
            repo.write(PATH, b"");
        }
    }

    Generated {
        shape,
        eol,
        trailing,
    }
}

/// A random subset of the file's additions and deletions.
fn select(rng: &mut Rng, file: &RawFile) -> BTreeSet<(usize, usize)> {
    let all: Vec<(usize, usize)> = patch::every_change(file).into_iter().collect();
    if all.is_empty() {
        return BTreeSet::new();
    }
    match rng.below(4) {
        // Whole hunks, the common gesture.
        0 => {
            let keep: BTreeSet<usize> = (0..file.hunks.len()).filter(|_| rng.flip()).collect();
            all.into_iter().filter(|(h, _)| keep.contains(h)).collect()
        }
        // Exactly one hunk.
        1 => {
            let hunk = rng.below(file.hunks.len());
            all.into_iter().filter(|(h, _)| *h == hunk).collect()
        }
        // Everything: the degenerate case, which still has to round-trip.
        2 if rng.chance(1, 4) => all.into_iter().collect(),
        // Individual lines.
        _ => all.into_iter().filter(|_| rng.flip()).collect(),
    }
}

#[test]
fn synthesized_patches_agree_with_git_apply_cached() {
    let repo = TempRepo::new("props");
    repo.write("seed.txt", b"seed\n");
    repo.commit_all("seed");

    let total: u64 = std::env::var("CIDE_GIT_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);

    let mut applied = 0u32;
    let mut refused = 0u32;
    let mut empty = 0u32;

    for seed in 0..total {
        let mut rng = Rng::new(seed);
        let generated = generate(&repo, &mut rng);

        let git_repo = git2::Repository::open(&repo.root).expect("open");
        let request = DiffRequest::new(DiffSide::Unstaged);
        let Some(file) = diff::file_diff(&git_repo, PATH, request).expect("diff") else {
            empty += 1;
            continue;
        };
        let chosen = select(&mut rng, &file);
        if chosen.is_empty() {
            empty += 1;
            continue;
        }

        let text = match patch::synthesize(&file, &chosen) {
            Ok(Some(text)) => text,
            Ok(None) => {
                empty += 1;
                continue;
            }
            // The only refusal a generated text file can produce. Everything else would be a
            // bug, so it is asserted rather than counted.
            Err(GitError::PartialRefused {
                reason: PartialRefusal::NoNewlineOrdering,
                ..
            }) => {
                refused += 1;
                continue;
            }
            Err(error) => panic!("seed {seed} ({:?}): {error}", generated.shape),
        };
        drop(git_repo);

        let before = repo.save_index();
        let ours = apply_with_libgit2(&repo, &text).unwrap_or_else(|e| {
            panic!(
                "seed {seed}: libgit2 refused our own patch: {e}\n{}",
                show(&text)
            )
        });
        repo.restore_index(&before);

        let (ok, output) =
            repo.try_git_stdin(&["apply", "--cached", "--whitespace=nowarn", "-"], &text);
        assert!(
            ok,
            "seed {seed} ({:?}, {:?}, trailing={}): git apply --cached refused our patch:\n{output}\n{}",
            generated.shape,
            generated.eol,
            generated.trailing,
            show(&text)
        );
        let theirs = repo.index_state();
        repo.restore_index(&before);

        assert_eq!(
            ours,
            theirs,
            "seed {seed} ({:?}, {:?}, trailing={}): index differs\n{}",
            generated.shape,
            generated.eol,
            generated.trailing,
            show(&text)
        );
        applied += 1;
    }

    eprintln!("[props] {applied} compared, {refused} refused, {empty} empty of {total}");
    // A run where almost nothing produced a patch would pass vacuously.
    assert!(
        u64::from(applied) > total / 2,
        "only {applied} of {total} cases produced a comparable patch"
    );
    // The no-newline refusal is a real, narrow case; if it starts swallowing most of the
    // corpus then the rule has grown too broad and the tests above stopped covering anything.
    assert!(
        u64::from(refused) < total / 5,
        "{refused} of {total} cases were refused for marker ordering"
    );
}

/// Apply through `Repository::apply(ApplyLocation::Index)` and read the index back with git.
fn apply_with_libgit2(repo: &TempRepo, text: &[u8]) -> Result<String, String> {
    let git_repo = git2::Repository::open(&repo.root).map_err(|e| e.to_string())?;
    let parsed = git2::Diff::from_buffer(text).map_err(|e| e.to_string())?;
    git_repo
        .apply(&parsed, git2::ApplyLocation::Index, None)
        .map_err(|e| format!("{:?}: {}", e.class(), e.message()))?;
    drop(git_repo);
    Ok(repo.index_state())
}

fn show(text: &[u8]) -> String {
    format!(
        "--- patch ---\n{}\n-------------",
        String::from_utf8_lossy(text)
    )
}

/// A hunk we did not modify must render byte-identically to the one libgit2 printed.
///
/// This is the sharper half of the property: `git apply` would tolerate `-5,1` where git
/// writes `-5`, so an agreement test alone cannot tell whether the header format is right.
#[test]
fn selecting_everything_reproduces_libgit2s_own_patch_bytes() {
    let repo = TempRepo::new("identity");
    repo.write("seed.txt", b"seed\n");
    repo.commit_all("seed");

    for seed in 0..60u64 {
        let mut rng = Rng::new(seed ^ 0xfeed);
        let generated = generate(&repo, &mut rng);
        let git_repo = git2::Repository::open(&repo.root).expect("open");
        let Some(file) =
            diff::file_diff(&git_repo, PATH, DiffRequest::new(DiffSide::Unstaged)).expect("diff")
        else {
            continue;
        };
        let all = patch::every_change(&file);
        if all.is_empty() {
            continue;
        }
        let Ok(Some(ours)) = patch::synthesize(&file, &all) else {
            continue;
        };
        assert_eq!(
            String::from_utf8_lossy(&ours),
            String::from_utf8_lossy(&file.render()),
            "seed {seed} ({:?}, {:?}, trailing={}) did not round-trip",
            generated.shape,
            generated.eol,
            generated.trailing
        );
    }
}
