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
//! Agreement is necessary and not sufficient: two appliers of one bad patch agree on the wrong
//! answer. So every case is also checked against [`expected_blob`], which builds the content
//! the selection *means* out of the pre-image and the diff, with no reference to the patch
//! under test. That is the check that fails when a synthesized patch applies cleanly and
//! stages something else — the shape of bug that the agreement check alone let through.
//!
//! # What the corpus covers
//!
//! One generated text file proves nothing about the cases that actually break naive patch
//! synthesis, so [`Category`] enumerates them and every one of them is generated:
//!
//! | category | outcome | what it is |
//! | --- | --- | --- |
//! | `text` | compared | LF/CRLF × trailing-newline × modified/added/emptied |
//! | `multi-file` | compared | one selection spanning two or three paths, applied as one patch |
//! | `mode` | compared | the exec bit going on and coming off, with and without edits |
//! | `intent-to-add` | compared | `git add -N`, whose pre-image is the empty blob |
//! | `binary` | refused | `Binary` |
//! | `submodule` | refused | `Submodule` |
//! | `rename` | refused | `Rename` — a rename *with edits*, one indivisible delta |
//! | `deletion` | refused | `Deletion` |
//!
//! A refusal is the correct answer for the last four: there is no patch to compare, because
//! there is no patch this crate is willing to write. Those cases are still generated, and each
//! asserts three things — that the delta really has the shape the category claims (a rename
//! that stopped being detected as a rename would otherwise pass silently), that the refusal
//! comes back with the right reason, and that whole-file staging of the same path still
//! matches `git add`. The last is what makes a refusal safe rather than a dead end.
//!
//! # Every category checks that it is still itself
//!
//! A category that stops being what it is named for does not fail a comparison: it produces a
//! different, still-correct patch that both appliers still agree on. So the property the name
//! makes is asserted per case, on the comparable categories as well as the refusing ones —
//! [`part_delta`] for the mode header and the exec bit, [`assert_intent_to_add`] for the index
//! entry `git add -N` leaves, [`check_refusal`] for the other four — and the multi-file span is
//! counted and guarded at the end.
//!
//! This is not theoretical. Before those assertions existed, deleting the `chmod` from
//! [`mode_part`] and deleting the `git add -N` from [`generate`] each left the whole run green
//! to the line, counts included, with 66 "mode" cases that were plain text edits and 51
//! "intent-to-add" cases that were ordinary untracked additions.
//!
//! Per-category counts are printed at the end of the run and guarded: every category has to
//! appear, every comparable category has to actually compare in the majority of its cases, and
//! most multi-file cases have to apply a patch that really covers more than one path — so one
//! of them quietly turning into all-refusals, all-empties or a second `text` is a failure and
//! not a silently smaller corpus.
//!
//! Cases are generated from a seeded [`Rng`]; the seed is printed on failure, so a bad case is
//! reproduced by re-running rather than by re-rolling. `CIDE_GIT_CASES` overrides the count.
//!
//! Unix only, as `tests/staging.rs` already is: the mode cases need `chmod`, and the exec bit
//! is the mode change that exists.

mod support;

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cide_git::diff::{self, DiffRequest, RawFile};
use cide_git::{patch, stage};
use cide_ipc::git::{DiffSide, GitError, PartialRefusal, PathSelection, Selection};
use support::{Eol, Rng, TempRepo, binary_blob, mutate, text};

// --- the corpus's repository ----------------------------------------------------------------

/// Generated text, written fresh by each case. Untracked in the baseline commit, so a case is
/// free to make it an addition by leaving it out of the index.
const TEXT_A: &str = "f.txt";
const TEXT_B: &str = "g.txt";
/// Committed 100644 and 100755 respectively, so the exec bit can be tested going both ways.
const MODE_OFF: &str = "s.sh";
const MODE_ON: &str = "x.sh";
/// Committed binary, and the path an untracked binary is written to.
const BINARY: &str = "b.bin";
const BINARY_NEW: &str = "new.bin";
/// `git add -N` lands here.
const INTENT: &str = "n.txt";
/// Rename sources — committed, in three terminator shapes — and the path they move to.
const RENAME_SOURCES: [&str; 3] = ["r_lf.txt", "r_crlf.txt", "r_nonl.txt"];
const RENAME_TO: &str = "r_new.txt";
/// Deleted by the deletion cases.
const DELETABLE: &str = "d.txt";
const SUBMODULE: &str = "sub";
/// Written inside the submodule to dirty its content as well as its HEAD.
const SUBMODULE_DIRT: &str = "sub/dirt.txt";

/// Everything a case may leave behind that the baseline commit does not contain.
const SCRATCH: [&str; 6] = [
    TEXT_A,
    TEXT_B,
    INTENT,
    RENAME_TO,
    BINARY_NEW,
    SUBMODULE_DIRT,
];

/// A file the baseline commit contains, remembered with the shape of its text so a case can
/// mutate it without having to re-derive what it looks like.
struct Baseline {
    path: &'static str,
    bytes: Vec<u8>,
    eol: Eol,
    trailing: bool,
    mode: u32,
}

/// The repository every case runs in, plus what it takes to put it back.
///
/// One repository for the whole run rather than one per case: a `git init` and a baseline
/// commit per case would dominate the runtime, and the state a case needs is exactly the state
/// [`Corpus::reset`] restores — the index from a snapshot of its own bytes, the tracked files
/// from the bytes they were committed with, and the untracked ones by deletion.
struct Corpus {
    repo: TempRepo,
    /// The submodule's upstream. Held only to keep its directory alive: `sub`'s `origin`
    /// points into it, and a dropped `TempRepo` deletes itself.
    _upstream: TempRepo,
    /// `.git/index` as it stood after setup.
    pristine: Vec<u8>,
    baseline: Vec<Baseline>,
}

impl Corpus {
    fn new(tag: &str) -> Self {
        // The baseline commit's own contents are generated too, from a fixed seed: it is the
        // pre-image half the corpus mutates, so it should not be four hand-typed lines.
        let mut rng = Rng::new(0x0c0d_5eed);

        let upstream = TempRepo::new("props-upstream");
        upstream.write("a.txt", b"one\n");
        upstream.commit_all("submodule base");

        let repo = TempRepo::new(tag);
        let mut baseline = vec![
            Baseline {
                path: "seed.txt",
                bytes: b"seed\n".to_vec(),
                eol: Eol::Lf,
                trailing: true,
                mode: 0o644,
            },
            Baseline {
                path: MODE_OFF,
                bytes: text(&mut rng, 12, Eol::Lf, true),
                eol: Eol::Lf,
                trailing: true,
                mode: 0o644,
            },
            Baseline {
                path: MODE_ON,
                bytes: text(&mut rng, 12, Eol::Lf, true),
                eol: Eol::Lf,
                trailing: true,
                mode: 0o755,
            },
            Baseline {
                path: DELETABLE,
                bytes: text(&mut rng, 9, Eol::Lf, true),
                eol: Eol::Lf,
                trailing: true,
                mode: 0o644,
            },
        ];
        // The rename sources are committed in all three terminator shapes, because a rename is
        // detected by comparing HEAD's bytes with the working tree's — so the source has to be
        // *committed* in the shape under test, not written in it by the case.
        for (index, path) in RENAME_SOURCES.iter().enumerate() {
            let (eol, trailing) = match index {
                0 => (Eol::Lf, true),
                1 => (Eol::Crlf, true),
                _ => (Eol::Lf, false),
            };
            baseline.push(Baseline {
                path,
                bytes: text(&mut rng, 14, eol, trailing),
                eol,
                trailing,
                mode: 0o644,
            });
        }
        let mut blob = binary_blob(&mut rng, 512);
        blob[3] = 0;
        baseline.push(Baseline {
            path: BINARY,
            bytes: blob,
            eol: Eol::Lf,
            trailing: true,
            mode: 0o644,
        });

        for file in &baseline {
            repo.write(file.path, &file.bytes);
            set_mode(&repo, file.path, file.mode);
        }
        repo.commit_all("baseline");

        repo.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            upstream.root.to_str().expect("utf-8 path"),
            SUBMODULE,
        ]);
        repo.commit_all("add submodule");

        // Move the submodule's HEAD past the commit the outer index records, and leave it
        // there. The gitlink is then dirty in every case for the whole run, which costs
        // nothing: each case diffs its own paths through a pathspec, so no other category can
        // see it, and it is exactly the state a submodule case needs.
        upstream.write("a.txt", b"two\n");
        upstream.commit_all("submodule moves");
        let moved = upstream.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["-C", SUBMODULE, "fetch", "-q", "origin"]);
        repo.git(&["-C", SUBMODULE, "checkout", "-q", "--detach", &moved]);

        let pristine = repo.save_index();
        Self {
            repo,
            _upstream: upstream,
            pristine,
            baseline,
        }
    }

    /// Put the repository back to the state every case starts from.
    fn reset(&self) {
        self.repo.restore_index(&self.pristine);
        for file in &self.baseline {
            self.repo.write(file.path, &file.bytes);
            set_mode(&self.repo, file.path, file.mode);
        }
        for path in SCRATCH {
            self.repo.remove(path);
        }
    }

    fn baseline(&self, path: &str) -> &Baseline {
        self.baseline
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} is not in the baseline commit"))
    }
}

fn set_mode(repo: &TempRepo, path: &str, mode: u32) {
    std::fs::set_permissions(repo.root.join(path), PermissionsExt::from_mode(mode))
        .unwrap_or_else(|e| panic!("chmod {path}: {e}"));
}

// --- what a case is -------------------------------------------------------------------------

/// The categories the milestone criterion names, plus the multi-file case that no single-path
/// corpus can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Text,
    MultiFile,
    Mode,
    IntentToAdd,
    Binary,
    Submodule,
    Rename,
    Deletion,
}

impl Category {
    const ALL: [Category; 8] = [
        Category::Text,
        Category::MultiFile,
        Category::Mode,
        Category::IntentToAdd,
        Category::Binary,
        Category::Submodule,
        Category::Rename,
        Category::Deletion,
    ];

    fn name(self) -> &'static str {
        match self {
            Category::Text => "text",
            Category::MultiFile => "multi-file",
            Category::Mode => "mode",
            Category::IntentToAdd => "intent-to-add",
            Category::Binary => "binary",
            Category::Submodule => "submodule",
            Category::Rename => "rename",
            Category::Deletion => "deletion",
        }
    }

    /// The refusal this category must produce, or `None` when it is comparable against
    /// `git apply --cached`.
    fn refusal(self) -> Option<PartialRefusal> {
        match self {
            Category::Binary => Some(PartialRefusal::Binary),
            Category::Submodule => Some(PartialRefusal::Submodule),
            Category::Rename => Some(PartialRefusal::Rename),
            Category::Deletion => Some(PartialRefusal::Deletion),
            Category::Text | Category::MultiFile | Category::Mode | Category::IntentToAdd => None,
        }
    }

    /// Weights out of 100. The comparable categories carry most of the corpus because they are
    /// the ones with a byte-identical index to compare; a refusal case has one answer and
    /// checking it a hundred times over is not worth the forks.
    fn pick(rng: &mut Rng) -> Self {
        match rng.below(100) {
            0..=39 => Category::Text,
            40..=54 => Category::MultiFile,
            55..=66 => Category::Mode,
            67..=78 => Category::IntentToAdd,
            79..=84 => Category::Binary,
            85..=89 => Category::Submodule,
            90..=94 => Category::Rename,
            _ => Category::Deletion,
        }
    }
}

/// How one path in a case got into the state under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A tracked file edited in the working tree.
    Modified,
    /// A file that exists only in the working tree.
    Added,
    /// A tracked file whose every line is being removed.
    Emptied,
    /// Edited *and* chmod'd, so the mode lines ride in the same header as the hunks.
    ModifiedAndChmod,
    /// chmod'd with no content edit at all. There are no hunks, so there is nothing to
    /// select and the whole-file path is the only correct answer.
    ChmodOnly,
    /// `git add -N`, then written: the pre-image is the empty blob rather than nothing.
    IntentToAdd,
    /// A file libgit2 calls binary, tracked or not.
    Binary,
    /// A gitlink whose recorded commit is behind the submodule's HEAD.
    Submodule,
    /// Moved to another path, and edited there.
    Renamed,
    /// Removed from the working tree.
    Deleted,
}

/// One path in a case, and how it got that way.
struct Part {
    path: &'static str,
    shape: Shape,
    eol: Eol,
    trailing: bool,
}

struct Case {
    category: Category,
    parts: Vec<Part>,
    /// The diff the selection is made against. A rename only exists on a side that sees the
    /// index and only when rename detection was asked for, so that category diffs differently
    /// from every other one.
    request: DiffRequest,
}

impl Case {
    /// The prefix every failure message carries: enough to re-run and enough to read.
    fn describe(&self, seed: u64) -> String {
        let parts: Vec<String> = self
            .parts
            .iter()
            .map(|p| {
                format!(
                    "{}({:?}, {:?}, trailing={})",
                    p.path, p.shape, p.eol, p.trailing
                )
            })
            .collect();
        format!(
            "seed {seed} [{}] {}",
            self.category.name(),
            parts.join(" + ")
        )
    }
}

// --- generation -----------------------------------------------------------------------------

fn generate(corpus: &Corpus, rng: &mut Rng) -> Case {
    let category = Category::pick(rng);
    let repo = &corpus.repo;
    let mut parts = Vec::new();
    let mut request = DiffRequest::new(DiffSide::Unstaged);

    {
        // The index is set up in-process rather than with `git add`: 500 forks of git would
        // dominate the runtime and the entry libgit2 writes is the entry `git add` writes.
        let git_repo = git2::Repository::open(&repo.root).expect("open");
        let mut index = git_repo.index().expect("index");

        match category {
            Category::Text => parts.push(text_part(repo, &mut index, rng, TEXT_A)),
            Category::MultiFile => {
                parts.push(text_part(repo, &mut index, rng, TEXT_A));
                parts.push(text_part(repo, &mut index, rng, TEXT_B));
                // A third path that is not a plain text edit, so the concatenated patch has to
                // carry a mode header in the middle of it.
                if rng.flip() {
                    parts.push(mode_part(corpus, rng, false));
                }
            }
            Category::Mode => parts.push(mode_part(corpus, rng, true)),
            Category::IntentToAdd => {
                // Only the file is written here; `git add -N` runs below, after our own index
                // write, or that write would clobber the entry it creates.
                let eol = if rng.chance(1, 3) { Eol::Crlf } else { Eol::Lf };
                let trailing = !rng.chance(1, 3);
                let lines = 2 + rng.below(10);
                repo.write(INTENT, &text(rng, lines, eol, trailing));
                parts.push(Part {
                    path: INTENT,
                    shape: Shape::IntentToAdd,
                    eol,
                    trailing,
                });
            }
            Category::Binary => {
                // Tracked and modified, or untracked and new: libgit2 sniffs both.
                let path = if rng.flip() { BINARY } else { BINARY_NEW };
                let len = 64 + rng.below(1024);
                let mut blob = binary_blob(rng, len);
                blob[rng.below(32)] = 0;
                repo.write(path, &blob);
                parts.push(Part {
                    path,
                    shape: Shape::Binary,
                    eol: Eol::Lf,
                    trailing: true,
                });
            }
            Category::Submodule => {
                // The gitlink is already behind the submodule's HEAD — see `Corpus::new`. The
                // only thing left to vary is whether the submodule's own working tree is dirty
                // too, which is a different path through libgit2's submodule status.
                if rng.flip() {
                    repo.write(SUBMODULE_DIRT, b"uncommitted\n");
                }
                parts.push(Part {
                    path: SUBMODULE,
                    shape: Shape::Submodule,
                    eol: Eol::Lf,
                    trailing: true,
                });
            }
            Category::Rename => {
                let source = corpus.baseline(RENAME_SOURCES[rng.below(RENAME_SOURCES.len())]);
                // What `git mv` does — the file moves and the index entry moves with it —
                // followed by an edit left in the working tree. Both halves are needed: the
                // move is what libgit2's similarity pass pairs up, and the edit is what makes
                // it a rename *with edits* rather than a pure one.
                repo.write(RENAME_TO, &source.bytes);
                repo.remove(source.path);
                index
                    .remove_path(Path::new(source.path))
                    .expect("remove rename source");
                index
                    .add_path(Path::new(RENAME_TO))
                    .expect("add rename target");
                repo.rewrite(
                    RENAME_TO,
                    &mutate(rng, &source.bytes, source.eol, source.trailing),
                );
                request = DiffRequest::new(DiffSide::Combined).renames(true);
                parts.push(Part {
                    path: RENAME_TO,
                    shape: Shape::Renamed,
                    eol: source.eol,
                    trailing: source.trailing,
                });
            }
            Category::Deletion => {
                // Any committed text file will do, and rotating through them keeps the
                // deletion cases from all being one file in one terminator shape.
                let victims = [
                    DELETABLE,
                    RENAME_SOURCES[0],
                    RENAME_SOURCES[1],
                    RENAME_SOURCES[2],
                ];
                let source = corpus.baseline(victims[rng.below(victims.len())]);
                repo.remove(source.path);
                parts.push(Part {
                    path: source.path,
                    shape: Shape::Deleted,
                    eol: source.eol,
                    trailing: source.trailing,
                });
            }
        }

        index.write().expect("write index");
    }

    if category == Category::IntentToAdd {
        // `git add -N` is the only way to get a real intent-to-add entry, flags and all.
        repo.git(&["add", "-N", "--", INTENT]);
    }

    Case {
        category,
        parts,
        request,
    }
}

/// Set up one generated text file and describe it.
fn text_part(repo: &TempRepo, index: &mut git2::Index, rng: &mut Rng, path: &'static str) -> Part {
    let eol = if rng.chance(1, 3) { Eol::Crlf } else { Eol::Lf };
    // A file with no terminator on its last line is the case that produces
    // `\ No newline at end of file`, and it is where naive synthesis breaks first.
    let trailing = !rng.chance(1, 3);
    let shape = match rng.below(10) {
        0 | 1 => Shape::Added,
        2 => Shape::Emptied,
        _ => Shape::Modified,
    };

    match shape {
        Shape::Added => {
            let _ = index.remove_path(Path::new(path));
            let lines = 2 + rng.below(10);
            repo.write(path, &text(rng, lines, eol, trailing));
        }
        Shape::Modified => {
            let lines = 4 + rng.below(20);
            let base = text(rng, lines, eol, trailing);
            repo.write(path, &base);
            index.add_path(Path::new(path)).expect("add");
            repo.rewrite(path, &mutate(rng, &base, eol, trailing));
        }
        Shape::Emptied => {
            let lines = 3 + rng.below(8);
            let base = text(rng, lines, eol, trailing);
            repo.write(path, &base);
            index.add_path(Path::new(path)).expect("add");
            repo.write(path, b"");
        }
        other => unreachable!("{other:?} is not a text shape"),
    }

    Part {
        path,
        shape,
        eol,
        trailing,
    }
}

/// Flip the exec bit on a committed file, with or without a content edit.
///
/// `solo` says whether this is the only part of its case: [`Shape::ChmodOnly`] produces no
/// hunks at all, and a case with nothing to synthesize can only be checked on its own.
fn mode_part(corpus: &Corpus, rng: &mut Rng, solo: bool) -> Part {
    // Both directions. The bit coming *off* is the one a naive header writer gets wrong,
    // because it is the only case where `old mode` is the executable one.
    let source = if rng.flip() {
        corpus.baseline(MODE_OFF)
    } else {
        corpus.baseline(MODE_ON)
    };
    let flipped = if source.mode == 0o644 { 0o755 } else { 0o644 };
    set_mode(&corpus.repo, source.path, flipped);

    let shape = if solo && rng.chance(1, 4) {
        Shape::ChmodOnly
    } else {
        corpus.repo.write(
            source.path,
            &mutate(rng, &source.bytes, source.eol, source.trailing),
        );
        Shape::ModifiedAndChmod
    };
    Part {
        path: source.path,
        shape,
        eol: source.eol,
        trailing: source.trailing,
    }
}

/// The delta for one path, built the way the case needs it.
///
/// Everything goes through [`diff::file_diff`] — the call the production staging path makes —
/// except a rename, which cannot: see
/// [`a_rename_is_only_detected_when_both_sides_are_in_the_diff`].
fn delta_for(repo: &git2::Repository, path: &str, request: DiffRequest) -> Option<RawFile> {
    if !request.renames {
        return diff::file_diff(repo, path, request).expect("diff");
    }
    let diff = diff::build(repo, request, None).expect("diff");
    diff::raw_files(&diff)
        .expect("raw files")
        .into_iter()
        .find(|file| file.path == path)
}

/// The delta for one part of a comparable case, with the property its category is *named* for
/// checked before anything is synthesized.
///
/// The refusal categories get this in [`check_refusal`]; the comparable ones had nothing like
/// it, and a category that stops being itself does not fail a comparison — it produces a
/// slightly different, still-correct patch that both appliers still agree on. Verified by
/// mutation, twice: deleting the `chmod` from [`mode_part`] leaves 66 "mode" cases that are
/// plain text edits, and deleting the `git add -N` from [`generate`] leaves 51 "intent-to-add"
/// cases that are ordinary untracked additions. Both runs were green to the line, counts
/// included, before these assertions existed.
fn part_delta(
    repo: &git2::Repository,
    part: &Part,
    request: DiffRequest,
    what: &str,
) -> Option<RawFile> {
    if part.shape == Shape::IntentToAdd {
        assert_intent_to_add(repo, part.path, what);
    }

    let file = delta_for(repo, part.path, request);
    // `mutate` can undo its own edit — insert at `n`, then remove at `n` — so a plain
    // modification is the one shape whose case may legitimately have nothing to diff.
    if part.shape == Shape::Modified {
        return file;
    }
    let file = file.unwrap_or_else(|| {
        panic!(
            "{what}: {:?} produced no delta at {} — the case set nothing up",
            part.shape, part.path
        )
    });

    if matches!(part.shape, Shape::ModifiedAndChmod | Shape::ChmodOnly) {
        // Both halves. The modes are what the index comparison would notice; the header
        // lines are what `synthesize` has to carry through and what a rewritten header
        // drops, and they are only in the corpus because of this category.
        assert_eq!(
            (file.old_mode, file.new_mode),
            // Named, not defaulted: a wildcard arm would quietly assert "the bit came off"
            // about any path a future mode case reached for.
            match part.path {
                MODE_OFF => (0o100644, 0o100755),
                MODE_ON => (0o100755, 0o100644),
                other => panic!("{what}: {other} is not one of the mode-case paths"),
            },
            "{what}: the exec bit did not flip at {}",
            part.path
        );
        assert!(
            contains(&file.header, b"\nold mode ") && contains(&file.header, b"\nnew mode "),
            "{what}: libgit2 rendered no mode header for {}:\n{}",
            part.path,
            String::from_utf8_lossy(&file.header)
        );
    }
    Some(file)
}

/// The empty blob, `git hash-object -t blob /dev/null`.
const EMPTY_BLOB: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

/// `git add -N` leaves an index entry holding the empty blob with the intent-to-add bit set.
///
/// Nothing downstream can tell that apart from an ordinary untracked file: the delta, the
/// pre-image, the synthesized patch and the resulting index are identical, so the category
/// name was a claim no assertion made. The index entry is the only place the difference
/// exists, so it is where it has to be checked.
fn assert_intent_to_add(repo: &git2::Repository, path: &str, what: &str) {
    let index = repo.index().expect("index");
    let entry = index
        .get_path(Path::new(path), 0)
        .unwrap_or_else(|| panic!("{what}: {path} has no index entry, so it is not intent-to-add"));
    assert!(
        entry.flags_extended & git2::IndexEntryExtendedFlag::INTENT_TO_ADD.bits() != 0,
        "{what}: {path} is in the index without the intent-to-add bit"
    );
    assert_eq!(
        entry.id.to_string(),
        EMPTY_BLOB,
        "{what}: {path}'s pre-image is not the empty blob"
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Rename detection is a pass over the deltas *of one diff*, so both sides have to be in it.
///
/// [`diff::file_diff`] passes a pathspec, and libgit2 applies it while building the diff —
/// before `find_similar` ever runs. The deleted counterpart is filtered out, there is nothing
/// left to pair the addition with, and a rename comes back as a plain added file.
/// `DiffRequest::renames(true)` is therefore inert for every caller that goes through
/// `file_diff`, which today is `shelf::shelve` and the app's `git_diff_file` command — the one
/// whose comment says "the panel wants to *show* a rename".
///
/// Nothing is corrupted by it: staging never asks for rename detection (a rename delta is
/// refused there anyway), and what the narrow diff describes — a new file, whole — is a patch
/// that applies correctly. What is lost is the display, and the refusal that goes with it:
/// through `file_diff` the same file reports `partial_ok`, so the panel offers partial staging
/// of a rename's new side while believing it is offering it on an ordinary addition.
///
/// The assertions below pin the *mechanism*, not the wish. When `file_diff` learns to widen its
/// diff before the similarity pass, this is the test that says so.
#[test]
fn a_rename_is_only_detected_when_both_sides_are_in_the_diff() {
    let repo = TempRepo::new("rename-pathspec");
    let mut rng = Rng::new(0x5217);
    let base = text(&mut rng, 14, Eol::Lf, true);
    repo.write("a.txt", &base);
    repo.commit_all("base");

    // `git mv`, then an edit left in the working tree.
    repo.write("b.txt", &base);
    repo.remove("a.txt");
    let git_repo = git2::Repository::open(&repo.root).expect("open");
    let mut index = git_repo.index().expect("index");
    index.remove_path(Path::new("a.txt")).expect("remove");
    index.add_path(Path::new("b.txt")).expect("add");
    index.write().expect("write index");
    repo.write("b.txt", &mutate(&mut rng, &base, Eol::Lf, true));

    let request = DiffRequest::new(DiffSide::Combined).renames(true);

    let narrow = diff::file_diff(&git_repo, "b.txt", request)
        .expect("diff")
        .expect("a delta for b.txt");
    assert_eq!(
        narrow.status,
        git2::Delta::Added,
        "a pathspec'd diff found a rename — `file_diff` now widens its diff, so the corpus's \
         `delta_for` and both `renames(true)` callers should be revisited"
    );
    assert_eq!(narrow.old_path, None);
    assert_eq!(narrow.partial_refusal(), None);

    let wide = diff::build(&git_repo, request, None).expect("diff");
    let wide = diff::raw_files(&wide)
        .expect("raw files")
        .into_iter()
        .find(|file| file.path == "b.txt")
        .expect("a delta for b.txt");
    assert_eq!(wide.status, git2::Delta::Renamed);
    assert_eq!(wide.old_path.as_deref(), Some("a.txt"));
    assert_eq!(wide.partial_refusal(), Some(PartialRefusal::Rename));
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

// --- the property ---------------------------------------------------------------------------

/// What became of one generated case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// A patch was synthesized and both appliers agreed on the index it produces. Carries how
    /// many paths that one patch covered, because a multi-file case whose second path drew an
    /// empty selection is a single-file case wearing the label.
    Compared { paths: usize },
    /// There was nothing to synthesize and the whole-file path was checked instead.
    Whole,
    /// Refused — correctly, and for the reason the category expects.
    Refused,
    /// Nothing was selected, so there is nothing to say.
    Empty,
}

#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    generated: u32,
    compared: u32,
    /// Of `compared`, the ones whose single patch covered more than one path.
    spanned: u32,
    whole: u32,
    refused: u32,
    empty: u32,
}

#[test]
fn synthesized_patches_agree_with_git_apply_cached() {
    let corpus = Corpus::new("props");

    let total: u64 = std::env::var("CIDE_GIT_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);

    let mut counts = [Counts::default(); Category::ALL.len()];
    // Marker refusals from the categories that are supposed to produce a patch, counted apart
    // from the refusals that are a category's correct answer.
    let mut marker_refusals = 0u32;

    for seed in 0..total {
        let mut rng = Rng::new(seed);
        corpus.reset();
        let case = generate(&corpus, &mut rng);

        let outcome = match case.category.refusal() {
            Some(reason) => {
                check_refusal(&corpus, &case, reason, seed);
                Outcome::Refused
            }
            None => compare(&corpus, &case, &mut rng, seed),
        };

        let slot = &mut counts[Category::ALL
            .iter()
            .position(|c| *c == case.category)
            .expect("every category is in ALL")];
        slot.generated += 1;
        match outcome {
            Outcome::Compared { paths } => {
                slot.compared += 1;
                if paths > 1 {
                    slot.spanned += 1;
                }
            }
            Outcome::Whole => slot.whole += 1,
            Outcome::Refused => {
                slot.refused += 1;
                if case.category.refusal().is_none() {
                    marker_refusals += 1;
                }
            }
            Outcome::Empty => slot.empty += 1,
        }
    }

    for (category, count) in Category::ALL.iter().zip(counts.iter()) {
        eprintln!(
            "[props] {:<14} {:>4} generated  {:>4} compared ({:>3} spanning >1 path)  {:>4} whole  \
             {:>4} refused  {:>4} empty",
            category.name(),
            count.generated,
            count.compared,
            count.spanned,
            count.whole,
            count.refused,
            count.empty
        );
    }
    let compared: u32 = counts.iter().map(|c| c.compared).sum();
    eprintln!(
        "[props] {compared} compared, {marker_refusals} refused for marker ordering, of {total}"
    );

    // Both of these are ratios, and a ratio needs a denominator. Written as `x < total / 5`
    // they were unsatisfiable below five cases — `total / 5` is 0, and nothing is fewer than
    // none — so `CIDE_GIT_CASES=2` could not pass however well the code behaved. Multiplying
    // out instead of dividing keeps the meaning identical at every size that already worked
    // (at 500 both forms are `compared >= 167` and `marker_refusals <= 99`) and makes the
    // small ones merely weak rather than impossible.
    //
    // A run where almost nothing produced a patch would pass vacuously. A third rather than a
    // half, now that a fifth of the corpus is categories whose right answer is a refusal and
    // which therefore have no index to compare at all.
    assert!(
        u64::from(compared) * 3 > total,
        "only {compared} of {total} cases produced a comparable patch"
    );
    // The no-newline refusal is a real, narrow case; if it starts swallowing most of the
    // corpus then the rule has grown too broad and the tests above stopped covering anything.
    assert!(
        u64::from(marker_refusals) * 5 < total.max(5),
        "{marker_refusals} of {total} cases were refused for marker ordering"
    );

    // Per category, because the totals above are dominated by the text cases and would stay
    // green with a category generating nothing, comparing nothing, or refusing everything.
    //
    // The floor only applies to a long run: `CIDE_GIT_CASES=20` is a smoke test and would fail
    // a coverage claim it was never asked to make.
    //
    // 300 and not 200, which is where this was and which did not work: the gate and the floor
    // were picked independently and did not meet. At exactly 200 the submodule cases come out
    // at 4 against a floor of 5, so `CIDE_GIT_CASES=200` — the one size the code itself named —
    // failed. Category counts are noisy at that length; 300 is where `total / 40` has real
    // headroom (the rarest category is 10 against a floor of 7), and it holds for every size
    // from there to 2000, which was checked rather than assumed.
    let floor = if total >= 300 { (total / 40) as u32 } else { 0 };
    for (category, count) in Category::ALL.iter().zip(counts.iter()) {
        assert!(
            count.generated >= floor,
            "{} was generated {} times, below the floor of {floor} — the corpus stopped covering it",
            category.name(),
            count.generated
        );
        if count.generated == 0 {
            continue;
        }
        match category.refusal() {
            // Structural rather than measured: `check_refusal` panics on a wrong refusal, so a
            // refusal category that got this far refused every case by construction. Stated
            // anyway, because it is the line that fails if the loop above ever starts routing
            // one of these categories through `compare` instead.
            Some(_) => assert_eq!(
                count.refused,
                count.generated,
                "{} did not refuse every case",
                category.name()
            ),
            // A majority — but "majority" of two cases is a claim about the dice, not about
            // the synthesizer. `CIDE_GIT_CASES=10` gives the mode category two cases, and one
            // of them drawing an empty selection failed this. Below eight, the useful claim is
            // that the category produced a patch at all.
            None => {
                let good = count.compared + count.whole;
                let want = if count.generated >= 8 {
                    count.generated / 2 + 1
                } else {
                    1
                };
                assert!(
                    good >= want,
                    "{} compared {good} of {} cases, wanted {want}: {count:?}",
                    category.name(),
                    count.generated
                );
            }
        }
        // The one property `multi-file` exists for. A case whose second path happened to draw
        // an empty selection applies a one-path patch and still counts as compared, so without
        // this the category could shrink to `text` with a longer name and every guard above
        // would stay green.
        if *category == Category::MultiFile {
            let want = if count.generated >= 8 {
                count.generated / 2 + 1
            } else {
                1
            };
            assert!(
                count.spanned >= want,
                "only {} of {} multi-file cases applied a patch spanning more than one path, \
                 wanted {want}",
                count.spanned,
                count.generated
            );
        }
    }
}

/// A category whose right answer is a refusal.
///
/// There is no index to compare, so the assertions are the other two halves: the delta really
/// is what the category claims, and the file is still stageable whole.
fn check_refusal(corpus: &Corpus, case: &Case, reason: PartialRefusal, seed: u64) {
    let what = case.describe(seed);
    let [part] = &case.parts[..] else {
        panic!("{what}: a refusal case names exactly one path");
    };

    let git_repo = git2::Repository::open(&corpus.repo.root).expect("open");
    let file = delta_for(&git_repo, part.path, case.request)
        .unwrap_or_else(|| panic!("{what}: no delta at all — the case set nothing up"));

    // The delta has to have the shape the category thinks it does. Without this a rename that
    // stopped being detected as a rename, or a binary libgit2 stopped sniffing, would still
    // "refuse correctly" — for some other reason — and the category would silently stop
    // covering itself.
    match part.shape {
        Shape::Binary => assert!(file.binary, "{what}: libgit2 did not call it binary"),
        Shape::Submodule => assert_eq!(
            file.new_mode,
            u32::from(git2::FileMode::Commit),
            "{what}: not a gitlink"
        ),
        Shape::Renamed => {
            assert_eq!(file.status, git2::Delta::Renamed, "{what}: not a rename");
            assert!(
                RENAME_SOURCES.contains(&file.old_path.as_deref().unwrap_or_default()),
                "{what}: renamed from {:?}",
                file.old_path
            );
            assert!(
                !file.hunks.is_empty(),
                "{what}: a rename with no edits — the corpus stopped covering rename-with-edits"
            );
        }
        Shape::Deleted => assert_eq!(file.status, git2::Delta::Deleted, "{what}: not a deletion"),
        other => panic!("{what}: {other:?} is not a refusal shape"),
    }

    assert_eq!(
        file.partial_refusal(),
        Some(reason),
        "{what}: wrong refusal"
    );
    // A binary or a gitlink has no selectable lines, and `synthesize` refuses before it ever
    // looks at the selection — which is the point, so an impossible position is the right
    // thing to hand it.
    let all = patch::every_change(&file);
    let chosen = if all.is_empty() {
        BTreeSet::from([(0usize, 0usize)])
    } else {
        all
    };
    assert_eq!(
        patch::synthesize(&file, &chosen).unwrap_err(),
        GitError::PartialRefused {
            path: file.path.clone(),
            reason
        },
        "{what}: wrong error from synthesize"
    );
    drop(git_repo);

    assert_whole_matches_git_add(&corpus.repo, part.path, &what);
}

/// Synthesize, apply both ways, and compare — for the categories that have a patch.
fn compare(corpus: &Corpus, case: &Case, rng: &mut Rng, seed: u64) -> Outcome {
    let repo = &corpus.repo;
    let what = case.describe(seed);

    /// One path's synthesized patch and the content it is supposed to stage.
    struct Ready {
        path: &'static str,
        text: Vec<u8>,
        expected: Vec<u8>,
    }

    let git_repo = git2::Repository::open(&repo.root).expect("open");
    let mut ready: Vec<Ready> = Vec::new();
    let mut whole: Option<&'static str> = None;

    for part in &case.parts {
        let Some(file) = part_delta(&git_repo, part, case.request, &what) else {
            continue;
        };

        if part.shape == Shape::ChmodOnly {
            // A pure mode change has no hunks, so there is nothing to select and nothing to
            // synthesize. `stage::is_whole` routes exactly this to the index API; the check
            // below is that it has something to route.
            assert_eq!(
                file.change_count(),
                0,
                "{what}: a pure mode change grew hunks"
            );
            assert!(
                matches!(
                    patch::synthesize(&file, &patch::every_change(&file)),
                    Ok(None)
                ),
                "{what}: a pure mode change synthesized a patch body"
            );
            whole = Some(part.path);
            continue;
        }

        let chosen = select(rng, &file);
        if chosen.is_empty() {
            continue;
        }

        // The pre-image the patch will be applied to, read before anything touches the index.
        let pre = diff::index_blob(&git_repo, part.path)
            .expect("index blob")
            .map(|oid| git_repo.find_blob(oid).expect("blob").content().to_vec())
            .unwrap_or_default();
        let (expected, representable) = expected_blob(&pre, &file, &chosen);

        let text = match patch::synthesize(&file, &chosen) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            // The only refusal a generated text file can produce. Everything else would be a
            // bug, so it is asserted rather than counted.
            Err(GitError::PartialRefused {
                reason: PartialRefusal::NoNewlineOrdering,
                ..
            }) => return Outcome::Refused,
            Err(error) => panic!("{what}: {error}"),
        };
        assert!(
            representable,
            "{what}: the selection describes a file with an unterminated line in the middle, \
             and synthesize emitted a patch for it anyway\n{}",
            show(&text)
        );
        ready.push(Ready {
            path: part.path,
            text,
            expected,
        });
    }
    drop(git_repo);

    if ready.is_empty() {
        // A `ChmodOnly` part is generated on its own precisely so that this is unambiguous:
        // there is no patch in flight whose index state the staging call below would disturb.
        if let Some(path) = whole {
            assert_whole_matches_git_add(repo, path, &what);
            return Outcome::Whole;
        }
        return Outcome::Empty;
    }

    // One patch for the whole selection, which is what `stage::apply_plans` builds: the parts
    // are concatenated and applied in a single call, so a case spanning files is one apply and
    // not several.
    let mut text = Vec::new();
    for part in &ready {
        text.extend_from_slice(&part.text);
    }
    let paths: Vec<&str> = ready.iter().map(|r| r.path).collect();

    let before = repo.save_index();
    let (ours, staged) = apply_with_libgit2(repo, &text, &paths).unwrap_or_else(|e| {
        panic!(
            "{what}: libgit2 refused our own patch: {e}\n{}",
            show(&text)
        )
    });
    for (part, blob) in ready.iter().zip(staged.iter()) {
        assert_eq!(
            String::from_utf8_lossy(blob),
            String::from_utf8_lossy(&part.expected),
            "{what}: the patch applied cleanly and staged content the selection does not \
             describe, at {}\n{}",
            part.path,
            show(&text)
        );
    }
    repo.restore_index(&before);

    let (ok, output) =
        repo.try_git_stdin(&["apply", "--cached", "--whitespace=nowarn", "-"], &text);
    assert!(
        ok,
        "{what}: git apply --cached refused our patch:\n{output}\n{}",
        show(&text)
    );
    let theirs = repo.index_state();
    repo.restore_index(&before);

    assert_eq!(ours, theirs, "{what}: index differs\n{}", show(&text));
    Outcome::Compared { paths: ready.len() }
}

/// Apply through `Repository::apply(ApplyLocation::Index)`, then read back both the index
/// listing (with the `git` binary) and the staged blob's bytes for each path.
fn apply_with_libgit2(
    repo: &TempRepo,
    text: &[u8],
    paths: &[&str],
) -> Result<(String, Vec<Vec<u8>>), String> {
    let git_repo = git2::Repository::open(&repo.root).map_err(|e| e.to_string())?;
    let parsed = git2::Diff::from_buffer(text).map_err(|e| e.to_string())?;
    git_repo
        .apply(&parsed, git2::ApplyLocation::Index, None)
        .map_err(|e| format!("{:?}: {}", e.class(), e.message()))?;
    let mut staged = Vec::with_capacity(paths.len());
    for path in paths {
        staged.push(
            diff::index_blob(&git_repo, path)
                .map_err(|e| e.to_string())?
                .map(|oid| {
                    git_repo
                        .find_blob(oid)
                        .map(|b| b.content().to_vec())
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
        );
    }
    drop(git_repo);
    Ok((repo.index_state(), staged))
}

/// Whole-file staging has to be indistinguishable from `git add`.
///
/// This is what makes a refusal safe rather than a dead end: everything patch synthesis will
/// not describe is still stageable, exactly, through the index API — which is the whole reason
/// [`stage`] has two mechanisms.
fn assert_whole_matches_git_add(repo: &TempRepo, path: &str, what: &str) {
    let before = repo.save_index();
    let selection = PathSelection {
        path: path.to_string(),
        selection: Selection::Whole,
        rev: None,
    };
    stage::stage(&repo.root, std::slice::from_ref(&selection))
        .unwrap_or_else(|e| panic!("{what}: staging {path} whole: {e}"));
    let ours = repo.index_state();

    repo.restore_index(&before);
    repo.git(&["add", "--", path]);
    let theirs = repo.index_state();
    repo.restore_index(&before);

    assert_eq!(
        ours, theirs,
        "{what}: whole-file staging of {path} diverged from `git add`"
    );
}

/// The blob `chosen` describes, built from the pre-image and the diff and *not* from the patch
/// under test.
///
/// This is the independent oracle. Walking the old file and keeping context, keeping selected
/// additions, dropping unselected ones and keeping unselected deletions is the definition of
/// what partial staging means; the patch is only a way to say it to git.
///
/// The second return value is whether the result is a file that can exist at all. A line
/// without a terminator can only be the last one, so a reconstruction that puts one in the
/// middle means the selection is unrepresentable and `synthesize` must refuse it — which is
/// exactly the `\ No newline` case, stated here in terms of content rather than of markers.
fn expected_blob(pre: &[u8], file: &RawFile, chosen: &BTreeSet<(usize, usize)>) -> (Vec<u8>, bool) {
    let lines = split_keeping_terminators(pre);
    let mut kept: Vec<&[u8]> = Vec::new();
    let mut cursor = 0usize;

    for (index, hunk) in file.hunks.iter().enumerate() {
        // libgit2 reports the line *before* an empty old range, so a pure insertion at
        // `old_start` lands after that line; a non-empty range starts one earlier, 0-based.
        let start = if hunk.old_lines > 0 {
            hunk.old_start as usize - 1
        } else {
            hunk.old_start as usize
        };
        kept.extend(
            lines[cursor.min(lines.len())..start.min(lines.len())]
                .iter()
                .copied(),
        );
        for (line, raw) in hunk.lines.iter().enumerate() {
            let keep = match raw.origin {
                b'+' => chosen.contains(&(index, line)),
                b'-' => !chosen.contains(&(index, line)),
                _ => true,
            };
            if keep {
                kept.push(&raw.content);
            }
        }
        cursor = start + hunk.old_lines as usize;
    }
    kept.extend(lines[cursor.min(lines.len())..].iter().copied());

    let representable = kept
        .iter()
        .enumerate()
        .all(|(i, piece)| i + 1 == kept.len() || piece.ends_with(b"\n"));
    (kept.concat(), representable)
}

/// Split into lines, each still carrying its terminator; the last may have none.
fn split_keeping_terminators(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            out.push(&bytes[start..=index]);
            start = index + 1;
        }
    }
    if start < bytes.len() {
        out.push(&bytes[start..]);
    }
    out
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
/// writes `-5`, so an agreement test alone cannot tell whether the header format is right. It
/// runs over the same generator, so it also covers the headers only some categories have —
/// `old mode`/`new mode` in particular, which a rewritten header would drop.
#[test]
fn selecting_everything_reproduces_libgit2s_own_patch_bytes() {
    let corpus = Corpus::new("identity");

    for seed in 0..120u64 {
        let mut rng = Rng::new(seed ^ 0xfeed);
        corpus.reset();
        let case = generate(&corpus, &mut rng);
        if case.category.refusal().is_some() {
            continue;
        }

        let git_repo = git2::Repository::open(&corpus.repo.root).expect("open");
        let what = case.describe(seed);
        for part in &case.parts {
            let Some(file) = part_delta(&git_repo, part, case.request, &what) else {
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
                "{what} did not round-trip at {}",
                part.path
            );
        }
    }
}
