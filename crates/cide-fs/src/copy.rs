//! Copy, cut and paste of the files themselves. (M8, the tree's clipboard)
//!
//! The file tree could make, rename and trash entries but not duplicate or move them, so
//! Ctrl+C/Ctrl+V in the panel did nothing at all. This is the disk half of that gesture. The
//! frontend half — what is on the clipboard, which folder a paste lands in, what the row says
//! afterwards — is `ui/src/sidebar/clipboardModel.ts`.
//!
//! # The four decisions, and what lost
//!
//! **A collision is asked about, and the default answer is still the rename.** Pasting
//! `main.rs` into a directory that already has one produces `main copy.rs`, then
//! `main copy 2.rs` — unless the caller sent a [`PasteChoice::Replace`] for that source, which
//! is the only thing in this module that can destroy a file the user already had.
//!
//! It shipped without the question, and the answer to
//!
//! > *"Paste collisions - yes, should be a confirmation"*
//!
//! is [`plan`]: it answers every collision *before* anything is written, so the dialog can ask
//! all of its questions and only then call [`paste_with`]. That ordering is the feature. Asking
//! *during* the write is the version where a user who backs out at the fourth of seven
//! questions is left with three files already on disk and a dialog apologising for it; here,
//! cancelling means the command was never sent and nothing was written at all.
//!
//! The two rejected shapes are still rejected. *Always overwrite* destroys a file that is not
//! the one the user was pointing at. *Always refuse* makes the ordinary gesture — paste a file
//! beside itself to duplicate it — impossible, which is how a paste ends up feeling broken; so
//! a paste into the source's **own folder** is a duplicate and is never asked about at all.
//! [`PastedEntry::renamed`] still carries the rename back, because the row the user was looking
//! at is still there holding the older file.
//!
//! The renaming is not a check-then-create. The name is *claimed* with `create_new`/
//! `create_dir`, which fail atomically when something is already there, and the loop moves to
//! the next candidate — so two pastes racing each other, or a `git checkout` landing between
//! the check and the write, cannot end with one clobbering the other. That is the whole reason
//! the reservation exists rather than a `Path::exists` call.
//!
//! **An overwrite is as deliberate as the refusal it replaces.** Nothing here ever writes over
//! a path the caller did not name in a decision, and no overwrite is a truncate-in-place: the
//! new bytes go to a uniquely named sibling and arrive by `rename(2)`, so the destination holds
//! either the old file or the new one and never half of the new one. A copy that failed a
//! megabyte in would otherwise leave the user's file destroyed *and* the replacement
//! incomplete, which is worse than either outcome the dialog offered.
//!
//! **Replacing a folder with a folder is a merge, and that word is load-bearing.** The
//! contents are laid into the existing directory: a file in both is overwritten, a file only in
//! the destination is left exactly where it is. The other reading — remove the destination and
//! put the source in its place — deletes files that were never on screen and never mentioned,
//! and no count in a dialog can make that safe. Replacing a file **with** a directory (or the
//! reverse) is not a replacement of anything and is refused outright: [`FsError::CannotReplace`],
//! raised by [`plan`] before the question is even asked — and raised again, over the *whole*
//! pair with no walk budget, before [`paste_with`] writes anything. The dialog's counts may stop
//! early and say so; the refusal may not, because a merge abandoned at the mismatch has already
//! overwritten every file above it.
//!
//! **A cut moves nothing until it is pasted.** [`PasteMode`] travels with the paste; Ctrl+X
//! only writes to a clipboard in the frontend. A cut that is never pasted therefore costs
//! exactly nothing, which is the property the gesture lives or dies by.
//!
//! **A directory is copied recursively, and never into itself.** `dest.starts_with(source)`
//! is refused before anything is created — obeying it is not a bad paste but an unbounded one
//! that fills the disk.
//!
//! **A symlink is copied as a symlink, not as what it points at.** Following it would
//! duplicate whatever is on the other end — plausibly gigabytes, plausibly a directory outside
//! the project — and would silently turn a link into a copy that no longer tracks its target.
//! `cp -a` does the same. The target string is copied verbatim, so a *relative* link that is
//! pasted somewhere else may now point at nothing; that is what the link said, and rewriting
//! it would be inventing an intent.
//!
//! # What this deliberately does not do
//!
//! * **No undo.** A copy leaves new files the user can trash; a cut is a `rename(2)` with no
//!   record. The one thing that is undoable is the *cross-device* cut, whose source goes to
//!   the freedesktop trash rather than being unlinked (see [`place`]).
//! * **No progress and no cancel.** The whole paste is one blocking call on one worker. A
//!   `node_modules` therefore blocks the paste, not the window — `fs_paste` is `async` over
//!   `spawn_blocking` like every other handler in `cmd::fs`.
//! * **Times are not preserved.** Permissions are (`std::fs::copy` carries the mode, and
//!   directories get theirs set explicitly); mtime/atime are not, exactly like `cp` without
//!   `-p`. Preserving them needs `utimensat` and a second libc surface for a property nothing
//!   in this app reads.
//! * **Hard links become separate files**, and extended attributes are dropped. Both are
//!   `cp`'s default behaviour too.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

use cide_ipc::{PasteChoice, PasteCollision, PasteDecision, PasteMode, PastedEntry};

use crate::error::{FsError, Result};
use crate::ops;

/// How many `name copy N` candidates are tried before giving up.
///
/// A thousand is far past any real gesture and still finite: without a cap, a directory that
/// somehow held every candidate would spin forever inside a blocking worker, which presents as
/// a hung paste with no error — the failure mode this whole crate keeps writing comments about.
const MAX_CANDIDATES: u32 = 1000;

/// How many entries a merge preview reads before it stops counting and says so.
///
/// [`plan`] runs before the user has agreed to anything, so it must not be the expensive half
/// of the gesture: walking a `node_modules` on both sides to produce a number nobody reads past
/// the first two digits is a dialog that takes a second to open. Past this the counts become
/// lower bounds and [`PasteCollision::truncated`] says so — which is a true answer, unlike an
/// exact number that took a second to compute or a wrong one that took none.
///
/// **This bounds the dialog's numbers and nothing else.** [`check_replaceable`] walks the pair
/// with no budget at all, because a *refusal* that gives up early is not a lower bound — it is
/// a wrong answer that lets [`merge_into`] discover the mismatch after it has already
/// overwritten everything above it. A budget is affordable for a count nobody has agreed to
/// yet; it is not affordable for the check that decides whether a byte gets written.
const PREVIEW_LIMIT: u32 = 4096;

/// The budget [`check_replaceable`] walks with: none.
///
/// Its own name rather than a literal at the call site, because the mistake it prevents is a
/// one-word edit that no fixture small enough to be a test can catch — the folder pair has to
/// hold more than [`PREVIEW_LIMIT`] entries before a bounded refusal starts lying, and building
/// one of those is seconds of I/O for a walk whose order the test cannot control. So the
/// constant is what the test pins, and this is where the reason lives.
const REFUSAL_LIMIT: u32 = u32::MAX;

/// How many of the files a merge would overwrite are named individually.
///
/// Six, because the dialog is a list the user reads, not a manifest. The rest are the count.
const PREVIEW_SAMPLE: usize = 6;

/// Paste `sources` into `dest_dir`, renaming every collision.
///
/// The four-argument form, kept because it is what every caller that has no dialog wants and
/// what the collision tests are written against: no decision means [`PasteChoice::KeepBoth`],
/// which is the behaviour that shipped. Nothing reachable from here can overwrite.
pub fn paste(
    roots: &[PathBuf],
    sources: &[PathBuf],
    dest_dir: &Path,
    mode: PasteMode,
) -> Result<Vec<PastedEntry>> {
    paste_with(roots, sources, dest_dir, mode, &[])
}

/// Paste `sources` into `dest_dir`, answering collisions with `decisions`.
///
/// Every source is checked before any source moves, so a selection with one bad path in it
/// pastes nothing rather than half of itself — the same rule `ops::delete`'s caller follows,
/// and for the same reason: a partial result the caller was told nothing about is what leaves
/// a tree drawing files that are not there. A [`PasteChoice::Replace`] whose two sides are not
/// the same kind is refused in that same pass, so the file-for-a-folder case costs nothing
/// rather than stopping half way down a list.
///
/// A source with no decision is [`PasteChoice::KeepBoth`]. That default is the safety property:
/// this function overwrites **only** what it was explicitly told to, so a caller that forgets
/// to pass its decisions along renames rather than destroys.
///
/// Once the work starts a failure part way through cannot be undone. What did land is carried
/// in [`FsError::PartialPaste`] rather than discarded, unless *nothing* landed, in which case
/// the caller gets the real error instead of a wrapper claiming a partial success of zero.
pub fn paste_with(
    roots: &[PathBuf],
    sources: &[PathBuf],
    dest_dir: &Path,
    mode: PasteMode,
    decisions: &[PasteDecision],
) -> Result<Vec<PastedEntry>> {
    check_dest(roots, dest_dir)?;
    for source in sources {
        check_source(roots, source, dest_dir, mode)?;
        if matches!(choice_for(decisions, source), PasteChoice::Replace) {
            check_replaceable(source, dest_dir)?;
        }
    }

    let mut done: Vec<PastedEntry> = Vec::with_capacity(sources.len());
    for source in sources {
        match paste_one(source, dest_dir, mode, choice_for(decisions, source)) {
            Ok(entry) => done.push(entry),
            // The first source failing is not a partial anything. Reporting it as one would
            // wrap a perfectly clear "already exists" in a sentence about zero paths.
            Err(err) if done.is_empty() => return Err(err),
            Err(err) => {
                tracing::warn!(
                    failed = %source.display(),
                    already_pasted = done.len(),
                    "a multi-path paste stopped part way through"
                );
                return Err(FsError::PartialPaste {
                    pasted: done.iter().map(|e| e.dest.display().to_string()).collect(),
                    error: err.to_string(),
                });
            }
        }
    }
    Ok(done)
}

/// Every name this paste would land on that is already taken — **and nothing is written**.
///
/// The read half of the gesture, run first so the dialog has something to ask about. It repeats
/// [`paste_with`]'s own checks so that a refusal the user cannot answer (outside the project, a
/// folder into itself) arrives *instead of* a question rather than after one; the frontend
/// reports it the same way it reports a rejected paste.
///
/// An empty answer means "go ahead, nothing is in the way". That is the common case, and it is
/// the reason this is a separate call rather than a flag on `fs_paste`: the ordinary paste stays
/// one round trip with no dialog in it.
///
/// Racy by nature, and deliberately not defended against here. A name can be taken between this
/// answer and the paste, and a name that was taken can be freed. Both are handled where they
/// have to be — [`paste_one`] claims names atomically, and a `Replace` whose destination has
/// vanished falls back to claiming the free name rather than inventing a file to overwrite.
pub fn plan(
    roots: &[PathBuf],
    sources: &[PathBuf],
    dest_dir: &Path,
    mode: PasteMode,
) -> Result<Vec<PasteCollision>> {
    check_dest(roots, dest_dir)?;
    for source in sources {
        check_source(roots, source, dest_dir, mode)?;
    }
    let mut out = Vec::new();
    for source in sources {
        if let Some(collision) = collision_for(source, dest_dir, PREVIEW_LIMIT)? {
            out.push(collision);
        }
    }
    Ok(out)
}

/// The answer for one source, defaulting to the rename.
fn choice_for(decisions: &[PasteDecision], source: &Path) -> PasteChoice {
    decisions
        .iter()
        .find(|decision| decision.source == source)
        .map_or(PasteChoice::KeepBoth, |decision| decision.choice)
}

/// Refuse a `Replace` that is not one, before anything is written.
///
/// The kind mismatch is checked at the top *and* everywhere inside a merge, by walking the pair
/// as [`plan`] does — but with **no budget**, which is the difference that makes it a guarantee.
/// Doing it here rather than discovering it half way through the recursion is the difference
/// between a refusal and a directory left half merged: by the time `merge_into` meets a file
/// where it expected a folder, it has already overwritten everything above it.
///
/// [`PREVIEW_LIMIT`] must not reach this walk. It exists so the *dialog* opens quickly, and a
/// count that stops early is honest about it ([`PasteCollision::truncated`]); a refusal that
/// stops early is simply wrong, and a folder pair with more entries than the budget would then
/// be merged right up to the mismatch and abandoned there with a `CannotReplace` that reads as
/// "nothing happened".
fn check_replaceable(source: &Path, dest_dir: &Path) -> Result<()> {
    match collision_for(source, dest_dir, REFUSAL_LIMIT)? {
        Some(collision) => match collision.blocked {
            Some(why) => Err(FsError::CannotReplace(why)),
            None => Ok(()),
        },
        // Nothing is there to replace. Not an error: the name may have been freed since the
        // question was asked, and `paste_one` will claim it like any other free name.
        None => Ok(()),
    }
}

/// What is in the way of `source` landing in `dest_dir`, or `None`.
///
/// `limit` caps how many entries a folder-on-folder preview reads. [`plan`] passes
/// [`PREVIEW_LIMIT`] so the dialog opens at once; [`check_replaceable`] passes
/// [`REFUSAL_LIMIT`], because the `blocked` it reads has to be the whole answer rather than the
/// first 4096 entries of it.
fn collision_for(source: &Path, dest_dir: &Path, limit: u32) -> Result<Option<PasteCollision>> {
    // A paste into the folder the source is already in is never a collision, and this is the
    // line that keeps *duplicate a file beside itself* a single gesture. The file would collide
    // with itself; asking "replace main.rs with main.rs?" is a question with no good answer, and
    // the only honest one — Keep both — is what the rename already does.
    if source.parent() == Some(dest_dir) {
        return Ok(None);
    }
    let name = name_of(source)?;
    let dest = dest_dir.join(name);
    let Some(existing) = optional_meta(&dest)? else {
        return Ok(None);
    };
    // `symlink_metadata` on both sides: a symlink is a thing in its own right here, so a link
    // sitting on the name is a collision, and a link is never "a directory" — replacing one
    // with a folder would be the mismatch below, which is exactly right.
    let meta = std::fs::symlink_metadata(source).map_err(|e| FsError::io(source, e))?;

    let mut collision = PasteCollision {
        source: source.to_path_buf(),
        dest,
        name: name.to_string(),
        merge: false,
        blocked: None,
        replaces: 0,
        keeps: 0,
        sample: Vec::new(),
        truncated: false,
    };
    if meta.is_dir() != existing.is_dir() {
        collision.blocked = Some(mismatch_message(name, meta.is_dir()));
        return Ok(Some(collision));
    }
    if meta.is_dir() {
        collision.merge = true;
        let mut seen = 0;
        // Cloned out first: the walk fills in the counts on `collision`, so it cannot also
        // borrow the destination path out of it.
        let into = collision.dest.clone();
        preview_merge(source, &into, "", &mut collision, &mut seen, limit)?;
    } else {
        collision.replaces = 1;
    }
    Ok(Some(collision))
}

/// The sentence a kind mismatch is refused with. Names which side is which.
fn mismatch_message(what: &str, source_is_dir: bool) -> String {
    let (from, to) = if source_is_dir {
        ("a folder", "a file")
    } else {
        ("a file", "a folder")
    };
    format!(
        "“{what}” is {from} and what is already there is {to} — replacing one with the other \
         would delete something nobody named. Keep both, or remove it yourself first."
    )
}

/// Count what merging `source` into `dest` would overwrite and what it would leave alone.
///
/// Both are directories. The destination's entries are read into a map and *removed* as the
/// source's names are matched against them, so what is left at the end is precisely the set the
/// merge does not touch — which is the number that makes the answer safe to give.
fn preview_merge(
    source: &Path,
    dest: &Path,
    rel: &str,
    out: &mut PasteCollision,
    seen: &mut u32,
    limit: u32,
) -> Result<()> {
    let mut existing: HashMap<OsString, bool> = HashMap::new();
    for entry in std::fs::read_dir(dest).map_err(|e| FsError::io(dest, e))? {
        let entry = entry.map_err(|e| FsError::io(dest, e))?;
        let kind = entry
            .file_type()
            .map_err(|e| FsError::io(&entry.path(), e))?;
        existing.insert(entry.file_name(), kind.is_dir());
    }

    for entry in std::fs::read_dir(source).map_err(|e| FsError::io(source, e))? {
        // One refusal is enough: the dialog cannot offer Replace either way, and walking on to
        // find a second one only makes the answer slower.
        if out.blocked.is_some() {
            return Ok(());
        }
        if *seen >= limit {
            out.truncated = true;
            return Ok(());
        }
        *seen += 1;
        let entry = entry.map_err(|e| FsError::io(source, e))?;
        let name = entry.file_name();
        let kind = entry
            .file_type()
            .map_err(|e| FsError::io(&entry.path(), e))?;
        let Some(dest_is_dir) = existing.remove(&name) else {
            // A name only the source has. The merge creates it and destroys nothing.
            continue;
        };
        let child_rel = join_rel(rel, &name);
        if kind.is_dir() != dest_is_dir {
            out.blocked = Some(mismatch_message(&child_rel, kind.is_dir()));
            return Ok(());
        }
        if kind.is_dir() {
            preview_merge(
                &entry.path(),
                &dest.join(&name),
                &child_rel,
                out,
                seen,
                limit,
            )?;
        } else {
            out.replaces += 1;
            if out.sample.len() < PREVIEW_SAMPLE {
                out.sample.push(child_rel);
            }
        }
    }

    // Whatever is left in the map is destination-only, at this level. Counted, not descended
    // into: this number is here to say "your files are still there", and a recursive count over
    // an untouched `node_modules` would drown that in five digits.
    out.keeps += u32::try_from(existing.len()).unwrap_or(u32::MAX);
    Ok(())
}

/// `src/main.rs` from `src` and `main.rs`, for the dialog to print.
///
/// Lossy on purpose and only here: this string is *shown*, never opened. Every path this module
/// acts on is a real `Path`, and `name_of` still refuses a non-UTF-8 name at the top of a paste.
fn join_rel(rel: &str, name: &OsString) -> String {
    let leaf = name.to_string_lossy();
    if rel.is_empty() {
        leaf.into_owned()
    } else {
        format!("{rel}/{leaf}")
    }
}

/// `symlink_metadata`, with "nothing is there" as a value rather than an error.
fn optional_meta(path: &Path) -> Result<Option<Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(FsError::io(path, err)),
    }
}

/// The folder a paste lands in has to be inside the project and has to still be a folder.
///
/// `is_dir` follows symlinks deliberately: a project that keeps `docs` as a link to somewhere
/// else inside itself is a directory as far as anyone using the tree is concerned. Containment
/// is still textual (`ops::check_within`), so the link's *name* is what is checked — see the
/// note there for why resolving it would break projects that legitimately contain one.
fn check_dest(roots: &[PathBuf], dest_dir: &Path) -> Result<()> {
    ops::check_within(roots, dest_dir)?;
    if !dest_dir.is_dir() {
        return Err(FsError::Io {
            path: dest_dir.display().to_string(),
            message: "is not a directory — a paste needs a folder to paste into".to_string(),
        });
    }
    Ok(())
}

/// Everything about one source that must be true *before* anything is created.
fn check_source(roots: &[PathBuf], source: &Path, dest_dir: &Path, mode: PasteMode) -> Result<()> {
    ops::check_within(roots, source)?;
    // A root may be *copied* — duplicating a checkout beside itself is a real thing to want —
    // but never moved, exactly as rename and delete refuse it. A root that moved would leave
    // the workspace pointing at a path with nothing at the end of it, and the tree does not
    // even remove a root node.
    if matches!(mode, PasteMode::Cut) {
        ops::check_not_root(roots, source)?;
    }

    // `symlink_metadata`, so a *link* is a thing to be copied rather than a hole where its
    // target used to be. A dangling one is still a real directory entry and still pastes.
    let meta = std::fs::symlink_metadata(source).map_err(|e| FsError::io(source, e))?;
    name_of(source)?;

    if meta.is_dir() && dest_dir.starts_with(source) {
        return Err(FsError::IntoItself(format!(
            "{} cannot be pasted into {}, which is inside it",
            source.display(),
            dest_dir.display()
        )));
    }
    Ok(())
}

/// The last component, as UTF-8.
///
/// Non-UTF-8 names are refused rather than lossily converted, which is the same rule the walk
/// applies: `Index` drops those entries, so a path that reached here with one is not a path the
/// tree could have offered, and `to_string_lossy` would name a different file than the one
/// asked for.
fn name_of(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| FsError::InvalidPath(path.display().to_string()))
}

fn paste_one(
    source: &Path,
    dest_dir: &Path,
    mode: PasteMode,
    choice: PasteChoice,
) -> Result<PastedEntry> {
    let meta = std::fs::symlink_metadata(source).map_err(|e| FsError::io(source, e))?;
    let name = name_of(source)?;

    // A cut pasted into the folder it is already in is a move to where it already is: nothing
    // to do, and *not* a duplicate. Answering it with `main copy.rs` would be a file the user
    // did not ask for, and answering it with an error would be a refusal of something that is
    // already true. Reported as a paste whose destination is its source, so the caller can
    // clear its clipboard and select the row like any other.
    if matches!(mode, PasteMode::Cut) && source.parent() == Some(dest_dir) {
        return Ok(PastedEntry {
            source: source.to_path_buf(),
            dest: source.to_path_buf(),
            renamed: false,
            skipped: 0,
            replaced: 0,
        });
    }

    let directory = meta.is_dir();

    // The one branch that can destroy something, and it is reachable only from a decision the
    // caller sent. `source.parent() != dest_dir` guards the duplicate gesture a second time:
    // `plan` never reports that as a collision, so a Replace arriving for one is a caller bug —
    // and honouring it would be a file copying over itself through a temporary, which is the
    // one way this module could lose a file with nothing left to blame.
    if matches!(choice, PasteChoice::Replace) && source.parent() != Some(dest_dir) {
        let dest = dest_dir.join(name);
        if optional_meta(&dest)?.is_some() {
            let mut tally = Tally::default();
            replace_onto(source, &dest, &meta, mode, &mut tally)?;
            return Ok(PastedEntry {
                source: source.to_path_buf(),
                dest,
                renamed: false,
                skipped: tally.skipped,
                replaced: tally.replaced,
            });
        }
        // The name came free between the question and the answer. Fall through and claim it the
        // ordinary way rather than conjuring a destination to overwrite.
    }

    let (dest, renamed) = reserve(dest_dir, name, directory)?;
    match place(source, &dest, &meta, mode) {
        Ok(skipped) => Ok(PastedEntry {
            source: source.to_path_buf(),
            dest,
            renamed,
            skipped,
            replaced: 0,
        }),
        Err(err) => {
            // Safe to remove without looking: the reservation *created* this path, so
            // everything under it arrived in the last few milliseconds from this function.
            // Leaving it would put a half-copied directory in the user's tree under a name
            // that reads as a finished one.
            let _ = if directory {
                std::fs::remove_dir_all(&dest)
            } else {
                std::fs::remove_file(&dest)
            };
            Err(err)
        }
    }
}

/// Claim a name in `dir`, answering where it landed and whether it had to be changed.
///
/// The claim is the creation: `create_dir` and `create_new` both fail with `EEXIST` in the
/// kernel, so nothing between the test and the write can lose a file. A `Path::exists` check
/// followed by a copy is the version of this that overwrites under a race.
fn reserve(dir: &Path, name: &str, directory: bool) -> Result<(PathBuf, bool)> {
    for attempt in 0..MAX_CANDIDATES {
        let candidate = dir.join(candidate_name(name, attempt, directory));
        if claim(&candidate, directory)? {
            return Ok((candidate, attempt > 0));
        }
    }
    Err(FsError::Io {
        path: dir.display().to_string(),
        message: format!("already holds {MAX_CANDIDATES} entries named after “{name}”"),
    })
}

/// Take `path` for a paste, answering whether it was free.
///
/// The claim *is* the creation, which is the whole safety property of this module: `create_dir`
/// and `create_new` fail with `EEXIST` in the kernel, so nothing between a test and a write can
/// lose a file. A `Path::exists` check followed by a copy is the version of this that overwrites
/// under a race — including against a *dangling* symlink sitting on the name, which `exists`
/// reports as absent and the kernel does not.
fn claim(path: &Path, directory: bool) -> Result<bool> {
    let made = if directory {
        std::fs::create_dir(path)
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(|_| ())
    };
    match made {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(FsError::io(path, err)),
    }
}

/// The name to try on the `attempt`-th go: `main.rs`, `main copy.rs`, `main copy 2.rs`.
///
/// A space and the word *copy*, like Finder and like VS Code, rather than `main(1).rs` or
/// `main.rs.1`: the suffix has to survive being read out loud and has to keep the extension
/// where the shell and the editor look for it, which rules out anything appended at the end.
///
/// The split is at the **last** dot, and only when it is not the first character — so
/// `.gitignore` copies to `.gitignore copy` rather than to ` copy.gitignore`, and `a.tar.gz`
/// to `a.tar copy.gz`. A *directory* is never split at all: `foo.bar` as a folder is a folder
/// whose name happens to contain a dot, and `foo copy.bar` would be a rename rather than a
/// copy of it.
pub fn candidate_name(name: &str, attempt: u32, directory: bool) -> String {
    if attempt == 0 {
        return name.to_string();
    }
    let suffix = if attempt == 1 {
        " copy".to_string()
    } else {
        format!(" copy {attempt}")
    };
    let dot = if directory {
        None
    } else {
        name.rfind('.').filter(|at| *at > 0)
    };
    match dot {
        Some(at) => format!("{}{}{}", &name[..at], suffix, &name[at..]),
        None => format!("{name}{suffix}"),
    }
}

/// Put `source` at the reserved `dest`, and answer how many entries were skipped.
///
/// `dest` already exists as the empty placeholder [`reserve`] made — an empty directory for a
/// directory, an empty file for everything else. That is what makes each branch here able to
/// finish without a second existence check.
fn place(source: &Path, dest: &Path, meta: &Metadata, mode: PasteMode) -> Result<u32> {
    if matches!(mode, PasteMode::Copy) {
        return copy_onto(source, dest, meta);
    }

    // `rename(2)` replaces the empty placeholder atomically — for a directory too, because
    // Linux allows renaming over an *empty* directory. So the destination name is never
    // unclaimed at any instant, and the move is one syscall for a tree of any size.
    match std::fs::rename(source, dest) {
        Ok(()) => Ok(0),
        Err(err) if is_cross_device(&err) => {
            // The roots of one project can straddle a mount point, and `rename` cannot. Copy,
            // then remove the source — through the **trash**, not `unlink`. The copy's success
            // is only as good as the return codes that reported it, and the difference between
            // a bug here and a user's directory being gone is one recoverable step. See
            // `crate::trash` for why there is a trash at all.
            let skipped = copy_onto(source, dest, meta)?;
            crate::trash::move_to_trash(source)?;
            Ok(skipped)
        }
        Err(err) => Err(FsError::io(source, err)),
    }
}

/// EXDEV — the source and the destination are on different filesystems.
///
/// By errno rather than by `ErrorKind`: `ErrorKind::CrossesDevices` is still unstable, and
/// matching the message text is how this breaks under a translated libc.
fn is_cross_device(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

/// What one *Replace* did, accumulated through the recursion.
#[derive(Default)]
struct Tally {
    /// Existing files overwritten. The number the panel says afterwards.
    replaced: u32,
    /// Sockets, fifos and devices left behind, exactly as a fresh copy leaves them.
    skipped: u32,
}

/// Overwrite `dest`, which exists, with `source`.
///
/// The kind is checked once more here even though [`check_replaceable`] already refused a
/// mismatch: between that pass and this one a `git checkout` can turn a file into a directory,
/// and the whole point of the refusal is that nothing guesses which of the two to keep.
fn replace_onto(
    source: &Path,
    dest: &Path,
    meta: &Metadata,
    mode: PasteMode,
    tally: &mut Tally,
) -> Result<()> {
    let existing = std::fs::symlink_metadata(dest).map_err(|e| FsError::io(dest, e))?;
    if meta.is_dir() != existing.is_dir() {
        return Err(FsError::CannotReplace(mismatch_message(
            name_of(dest)?,
            meta.is_dir(),
        )));
    }
    if meta.is_dir() {
        merge_into(source, dest, mode, tally)
    } else {
        replace_leaf(source, dest, meta, mode, tally)
    }
}

/// Lay `source`'s contents into the existing directory `dest`.
///
/// **A merge, not a swap.** A name only the destination has is not touched and not counted; a
/// name only the source has is created; a name both have is recursed into when both are folders
/// and overwritten when both are leaves. The rejected alternative — `remove_dir_all(dest)` and
/// then an ordinary paste — is four lines shorter and deletes every file the user had in there
/// that the source happens not to contain, which is a set nothing ever showed them.
///
/// There is no rollback. Half a merge cannot be undone, because undoing it means restoring
/// files this process overwrote and it does not keep them. That is why the question is asked
/// before the first byte is written and why a mismatch is refused in [`check_replaceable`]
/// rather than discovered here.
fn merge_into(source: &Path, dest: &Path, mode: PasteMode, tally: &mut Tally) -> Result<()> {
    for entry in std::fs::read_dir(source).map_err(|e| FsError::io(source, e))? {
        let entry = entry.map_err(|e| FsError::io(source, e))?;
        let kind = entry
            .file_type()
            .map_err(|e| FsError::io(&entry.path(), e))?;
        if !(kind.is_dir() || kind.is_file() || kind.is_symlink()) {
            // A fifo, a socket, a device node — skipped for the reason `copy_dir_contents`
            // skips them: reading one blocks until somebody writes to the other end.
            tally.skipped += 1;
            continue;
        }

        let name = entry.file_name();
        let child = entry.path();
        let target = dest.join(&name);
        let child_meta = std::fs::symlink_metadata(&child).map_err(|e| FsError::io(&child, e))?;
        let directory = child_meta.is_dir();

        if claim(&target, directory)? {
            match place(&child, &target, &child_meta, mode) {
                Ok(skipped) => tally.skipped += skipped,
                Err(err) => {
                    // Safe to remove unlooked-at: the claim created this path a moment ago, so
                    // nothing under it is the user's.
                    let _ = if directory {
                        std::fs::remove_dir_all(&target)
                    } else {
                        std::fs::remove_file(&target)
                    };
                    return Err(err);
                }
            }
            continue;
        }

        let existing = std::fs::symlink_metadata(&target).map_err(|e| FsError::io(&target, e))?;
        if directory && existing.is_dir() {
            merge_into(&child, &target, mode, tally)?;
        } else if directory != existing.is_dir() {
            return Err(FsError::CannotReplace(mismatch_message(
                &name.to_string_lossy(),
                directory,
            )));
        } else {
            replace_leaf(&child, &target, &child_meta, mode, tally)?;
        }
    }

    if matches!(mode, PasteMode::Cut) {
        // Every entry left by its own `rename`, so the folder is empty unless something was
        // skipped. `remove_dir` refuses a non-empty one, which is exactly the check wanted: the
        // leftovers are reported through `skipped` and the user still has them.
        let _ = std::fs::remove_dir(source);
    }
    Ok(())
}

/// Overwrite one existing non-directory `dest` with `source`.
fn replace_leaf(
    source: &Path,
    dest: &Path,
    meta: &Metadata,
    mode: PasteMode,
    tally: &mut Tally,
) -> Result<()> {
    tally.replaced += 1;
    if matches!(mode, PasteMode::Cut) {
        return move_over(source, dest, meta);
    }
    if meta.file_type().is_symlink() {
        // Already a temporary-plus-rename, so it lands on an occupied name in one step.
        return link_onto(source, dest);
    }
    copy_over(source, dest)
}

/// A cut's overwrite: one `rename(2)`, which replaces the destination atomically.
fn move_over(source: &Path, dest: &Path, meta: &Metadata) -> Result<()> {
    match std::fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device(&err) => {
            // Same fallback as `place`, and the same reason the source goes to the trash rather
            // than being unlinked: the copy's success is only as good as the return codes that
            // reported it.
            if meta.file_type().is_symlink() {
                link_onto(source, dest)?;
            } else {
                copy_over(source, dest)?;
            }
            crate::trash::move_to_trash(source)?;
            Ok(())
        }
        Err(err) => Err(FsError::io(source, err)),
    }
}

/// A sibling name no other write in this process is using: `.main.rs.cide-paste-4021-7.tmp`.
///
/// The pid alone is not enough, and the difference is a corrupted file rather than a lost race.
/// `fs_paste` is `async` over `spawn_blocking`, so two pastes — two windows, two panels — run
/// concurrently in **one** process; two overwrites of the same name in the same folder would
/// then have picked the same temporary, `std::fs::copy` into it from both sides, and `rename`
/// the interleaving onto the destination. That is a third file the user never had, arriving
/// under a guarantee that says the destination holds the old bytes or the new ones. The counter
/// makes the name unique per write, which is what the temporary was always assumed to be.
///
/// Leading dot so the tree's filter hides it for the milliseconds it exists.
fn temp_name(name: &str, what: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.cide-{what}-{}-{seq}.tmp", std::process::id())
}

/// Copy `source` over an existing `dest`, through a sibling and a `rename(2)`.
///
/// Not `std::fs::copy(source, dest)`, which truncates the destination first: a copy that fails
/// a megabyte in would leave the user's file destroyed *and* the replacement incomplete — a
/// third outcome the dialog never offered. Through a temporary, `dest` holds the old file or
/// the new one and nothing in between, and a failure leaves the old one exactly as it was.
///
/// The temporary is a sibling so that the `rename` is within one filesystem; `/tmp` is
/// routinely a different mount and would turn the atomic step back into a copy.
fn copy_over(source: &Path, dest: &Path) -> Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| FsError::InvalidPath(dest.display().to_string()))?;
    let tmp = dir.join(temp_name(name_of(dest)?, "paste"));
    let _ = std::fs::remove_file(&tmp);
    if let Err(err) = std::fs::copy(source, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(FsError::io(source, err));
    }
    if let Err(err) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(FsError::io(dest, err));
    }
    Ok(())
}

/// Copy `source` onto the placeholder at `dest`. Returns the count of skipped entries.
fn copy_onto(source: &Path, dest: &Path, meta: &Metadata) -> Result<u32> {
    if meta.file_type().is_symlink() {
        link_onto(source, dest)?;
        return Ok(0);
    }
    if !meta.is_dir() {
        // Truncates the placeholder and carries the permission bits, which is why the
        // placeholder being an ordinary empty file is enough.
        std::fs::copy(source, dest).map_err(|e| FsError::io(source, e))?;
        return Ok(0);
    }

    let mut skipped = 0;
    copy_dir_contents(source, dest, &mut skipped)?;
    // After the contents, not before: a directory whose mode is `r-xr-xr-x` cannot be written
    // into, so setting it first would make the copy fail on the folder's own children.
    let _ = std::fs::set_permissions(dest, meta.permissions());
    Ok(skipped)
}

/// Recreate a symlink at `dest`, which currently holds an empty placeholder file.
///
/// Through a uniquely named sibling and a `rename(2)`, the same shape as `ops::write`: the
/// obvious version — remove the placeholder, then `symlink` — unclaims the name for as long as
/// the two calls take, and the whole point of the reservation is that the name is never
/// unclaimed. `rename` replaces the placeholder atomically.
fn link_onto(source: &Path, dest: &Path) -> Result<()> {
    let target = std::fs::read_link(source).map_err(|e| FsError::io(source, e))?;
    #[cfg(unix)]
    {
        let dir = dest
            .parent()
            .ok_or_else(|| FsError::InvalidPath(dest.display().to_string()))?;
        let tmp = dir.join(temp_name(name_of(dest)?, "link"));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&target, &tmp).map_err(|e| FsError::io(&tmp, e))?;
        if let Err(err) = std::fs::rename(&tmp, dest) {
            let _ = std::fs::remove_file(&tmp);
            return Err(FsError::io(dest, err));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // No `symlink` without picking file-or-directory, which needs the target to exist.
        // Copying through the link is the honest fallback on a platform this app does not
        // ship to yet.
        std::fs::copy(source, dest).map_err(|e| FsError::io(source, e))?;
        Ok(())
    }
}

/// Copy every child of `from` into `to`, which is an empty directory this function owns.
///
/// No collision handling in here at all, and that is the point: `to` was created empty by
/// [`reserve`] a moment ago, so every name in it is one this walk put there. Only the *top*
/// of a paste can collide with something a user already has.
fn copy_dir_contents(from: &Path, to: &Path, skipped: &mut u32) -> Result<()> {
    let entries = std::fs::read_dir(from).map_err(|e| FsError::io(from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| FsError::io(from, e))?;
        let kind = entry
            .file_type()
            .map_err(|e| FsError::io(&entry.path(), e))?;
        let child = entry.path();
        let dest = to.join(entry.file_name());

        if kind.is_symlink() {
            // Recreated as a link, so a `node_modules/.bin` full of them stays a tree of links
            // rather than becoming a tree of copies of whatever they pointed at.
            let target = std::fs::read_link(&child).map_err(|e| FsError::io(&child, e))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dest).map_err(|e| FsError::io(&dest, e))?;
            #[cfg(not(unix))]
            {
                let _ = target;
                *skipped += 1;
            }
        } else if kind.is_dir() {
            std::fs::create_dir(&dest).map_err(|e| FsError::io(&dest, e))?;
            copy_dir_contents(&child, &dest, skipped)?;
            if let Ok(meta) = entry.metadata() {
                let _ = std::fs::set_permissions(&dest, meta.permissions());
            }
        } else if kind.is_file() {
            std::fs::copy(&child, &dest).map_err(|e| FsError::io(&child, e))?;
        } else {
            // A fifo, a socket, a device node. Reading a fifo blocks until somebody writes to
            // it, so "copy it anyway" is a hung paste; refusing the whole directory over one
            // would make a project containing a `.sock` uncopyable. Counted and reported.
            *skipped += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    fn roots_of(dir: &Path) -> Vec<PathBuf> {
        vec![dir.to_path_buf()]
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Every name in `dir`, sorted — for asserting that a read wrote nothing.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The name arithmetic, which is the whole of the collision policy.
    ///
    /// `ui/src/sidebar/clipboardModel.ts` has the same table so the panel can *say* what a
    /// paste is about to be called; this is the copy that decides it.
    #[test]
    fn a_collision_becomes_a_copy_rather_than_an_overwrite_or_a_refusal() {
        assert_eq!(candidate_name("main.rs", 0, false), "main.rs");
        assert_eq!(candidate_name("main.rs", 1, false), "main copy.rs");
        assert_eq!(candidate_name("main.rs", 2, false), "main copy 2.rs");
        // The extension has to stay an extension, or the copy stops being a Rust file — which
        // is what appending the suffix at the end would do.
        assert_eq!(candidate_name("a.tar.gz", 1, false), "a.tar copy.gz");
        // A leading dot is the whole name, not an extension.
        assert_eq!(candidate_name(".gitignore", 1, false), ".gitignore copy");
        assert_eq!(candidate_name("Makefile", 1, false), "Makefile copy");
        // A directory is never split: `foo.bar` names a folder, and `foo copy.bar` would be a
        // different folder rather than a copy of that one.
        assert_eq!(candidate_name("foo.bar", 1, true), "foo.bar copy");
        assert_eq!(candidate_name("src", 1, true), "src copy");
    }

    #[test]
    fn pasting_a_file_beside_itself_duplicates_it_and_never_clobbers() {
        let dir = scratch("copy-collision");
        let roots = roots_of(dir.path());
        std::fs::write(dir.join("main.rs"), "original").unwrap();

        let first = paste(&roots, &[dir.join("main.rs")], dir.path(), PasteMode::Copy).unwrap();
        assert_eq!(first[0].dest, dir.join("main copy.rs"));
        assert!(first[0].renamed, "the caller has to be told the name moved");

        let second = paste(&roots, &[dir.join("main.rs")], dir.path(), PasteMode::Copy).unwrap();
        assert_eq!(second[0].dest, dir.join("main copy 2.rs"));

        // The assertion the whole policy exists for: the file that was already there is
        // untouched, twice over.
        assert_eq!(read(&dir.join("main.rs")), "original");
        assert_eq!(read(&dir.join("main copy.rs")), "original");
        assert_eq!(read(&dir.join("main copy 2.rs")), "original");
    }

    #[test]
    fn a_directory_is_copied_recursively_with_its_contents() {
        let dir = scratch("copy-recursive");
        let roots = roots_of(dir.path());
        std::fs::create_dir_all(dir.join("src/deep/deeper")).unwrap();
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "a").unwrap();
        std::fs::write(dir.join("src/deep/b.rs"), "b").unwrap();
        std::fs::write(dir.join("src/deep/deeper/c.rs"), "c").unwrap();

        let out = paste(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/src"));
        assert!(!out[0].renamed);
        assert_eq!(out[0].skipped, 0);

        assert_eq!(read(&dir.join("into/src/a.rs")), "a");
        assert_eq!(read(&dir.join("into/src/deep/b.rs")), "b");
        assert_eq!(read(&dir.join("into/src/deep/deeper/c.rs")), "c");
        // And the original is still whole — this is a copy, not a move.
        assert_eq!(read(&dir.join("src/a.rs")), "a");
    }

    /// A collision **one level down** is not a collision at all.
    ///
    /// Only the top of a paste can meet something the user already had; everything under it
    /// lands in a directory that was created empty a moment earlier. The wrong version of this
    /// renames nested files too, and produces `src copy/main copy.rs`.
    #[test]
    fn only_the_top_of_a_paste_is_renamed_never_the_children() {
        let dir = scratch("copy-nested-collision");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "new").unwrap();
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::create_dir(dir.join("into/src")).unwrap();
        std::fs::write(dir.join("into/src/main.rs"), "already here").unwrap();

        let out = paste(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/src copy"));
        assert_eq!(read(&dir.join("into/src copy/main.rs")), "new");
        // The whole point: the file that was already there was not touched by a paste that
        // renamed the folder above it.
        assert_eq!(read(&dir.join("into/src/main.rs")), "already here");
    }

    #[test]
    fn a_directory_cannot_be_pasted_into_itself_or_into_anything_inside_it() {
        let dir = scratch("copy-into-itself");
        let roots = roots_of(dir.path());
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "a").unwrap();

        assert!(matches!(
            paste(
                &roots,
                &[dir.join("src")],
                &dir.join("src"),
                PasteMode::Copy
            ),
            Err(FsError::IntoItself(_))
        ));
        assert!(matches!(
            paste(
                &roots,
                &[dir.join("src")],
                &dir.join("src/deep"),
                PasteMode::Copy
            ),
            Err(FsError::IntoItself(_))
        ));
        // Refused *before* anything was created, so the recursion never started.
        assert!(!dir.join("src/deep/src").exists());
        assert!(!dir.join("src/src").exists());
        assert!(!dir.join("src/deep/deep").exists());

        // The sibling whose name merely shares a prefix is not inside it. `starts_with` is
        // component-wise, and the string version of this refuses a perfectly good paste.
        std::fs::create_dir(dir.join("srcx")).unwrap();
        paste(
            &roots,
            &[dir.join("src")],
            &dir.join("srcx"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(read(&dir.join("srcx/src/a.rs")), "a");
    }

    /// A symlink is copied **as a link**, and its target is not followed.
    ///
    /// The wrong version of this is invisible in the tree and enormous on disk: a link to a
    /// 2 GB directory outside the project copies 2 GB into it, and the copy stops tracking
    /// whatever the link was for.
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_copied_as_a_link_not_as_its_target() {
        let dir = scratch("copy-symlink");
        let roots = roots_of(dir.path());
        std::fs::write(dir.join("real.rs"), "real").unwrap();
        std::os::unix::fs::symlink(dir.join("real.rs"), dir.join("link.rs")).unwrap();
        std::os::unix::fs::symlink("nowhere-at-all", dir.join("broken.rs")).unwrap();
        std::fs::create_dir(dir.join("into")).unwrap();

        paste(
            &roots,
            &[dir.join("link.rs")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        let copied = dir.join("into/link.rs");
        assert!(
            copied.symlink_metadata().unwrap().file_type().is_symlink(),
            "the copy is a regular file, so the link was followed"
        );
        assert_eq!(std::fs::read_link(&copied).unwrap(), dir.join("real.rs"));

        // A dangling link is a real directory entry and pastes like any other. It also has no
        // target to follow, so the "just copy the file" version of this fails outright here.
        paste(
            &roots,
            &[dir.join("broken.rs")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_link(dir.join("into/broken.rs")).unwrap(),
            PathBuf::from("nowhere-at-all")
        );

        // And a link *inside* a copied directory keeps being a link.
        std::fs::create_dir(dir.join("pkg")).unwrap();
        std::os::unix::fs::symlink("../real.rs", dir.join("pkg/alias.rs")).unwrap();
        paste(
            &roots,
            &[dir.join("pkg")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert!(
            dir.join("into/pkg/alias.rs")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    /// A link whose name is taken still gets a new name rather than replacing what is there.
    ///
    /// This is the branch that used to unlink the placeholder and re-create it; the test is
    /// here because the atomic form has no observable difference except under a race, and
    /// without it the rename-through-a-temporary is a comment nothing holds.
    #[cfg(unix)]
    #[test]
    fn a_colliding_symlink_is_renamed_and_leaves_no_temporary_behind() {
        let dir = scratch("copy-symlink-collision");
        let roots = roots_of(dir.path());
        std::fs::write(dir.join("real.rs"), "real").unwrap();
        std::os::unix::fs::symlink("real.rs", dir.join("link.rs")).unwrap();

        let out = paste(&roots, &[dir.join("link.rs")], dir.path(), PasteMode::Copy).unwrap();
        assert_eq!(out[0].dest, dir.join("link copy.rs"));
        assert_eq!(
            std::fs::read_link(dir.join("link copy.rs")).unwrap(),
            PathBuf::from("real.rs")
        );
        assert!(
            std::fs::read_link(dir.join("link.rs")).is_ok(),
            "the original link was replaced instead of being left alone"
        );
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("cide-link"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_path_outside_the_project_root_is_refused_at_both_ends() {
        let dir = scratch("copy-outside");
        let outside = scratch("copy-elsewhere");
        let roots = roots_of(dir.path());
        std::fs::write(outside.join("secret"), "s").unwrap();
        std::fs::write(dir.join("mine.rs"), "m").unwrap();

        // A source from outside: the frontend can only ever send back a path the backend gave
        // it, so this is a bug or an attempt at one.
        assert!(matches!(
            paste(
                &roots,
                &[outside.join("secret")],
                dir.path(),
                PasteMode::Copy
            ),
            Err(FsError::OutsideProject(_))
        ));
        assert!(!dir.join("secret").exists());

        // And a destination outside it, which is the direction that *writes* somewhere it was
        // never allowed to.
        assert!(matches!(
            paste(
                &roots,
                &[dir.join("mine.rs")],
                outside.path(),
                PasteMode::Copy
            ),
            Err(FsError::OutsideProject(_))
        ));
        assert!(!outside.join("mine.rs").exists());

        // The climb, which is the shape a frontend bug actually produces.
        assert!(matches!(
            paste(
                &roots,
                &[dir.join("mine.rs")],
                &dir.join(".."),
                PasteMode::Copy
            ),
            Err(FsError::InvalidPath(_))
        ));
    }

    #[test]
    fn a_cut_moves_the_source_and_leaves_nothing_behind() {
        let dir = scratch("copy-cut");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("main.rs"), "body").unwrap();

        let out = paste(
            &roots,
            &[dir.join("main.rs")],
            &dir.join("into"),
            PasteMode::Cut,
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/main.rs"));
        assert_eq!(read(&dir.join("into/main.rs")), "body");
        assert!(!dir.join("main.rs").exists(), "the source survived a cut");
    }

    #[test]
    fn a_cut_into_a_name_that_is_taken_is_renamed_rather_than_overwriting() {
        let dir = scratch("copy-cut-collision");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("main.rs"), "moving").unwrap();
        std::fs::write(dir.join("into/main.rs"), "already here").unwrap();

        let out = paste(
            &roots,
            &[dir.join("main.rs")],
            &dir.join("into"),
            PasteMode::Cut,
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/main copy.rs"));
        assert!(out[0].renamed);
        // `rename(2)` on its own replaces the destination silently — this is the assertion that
        // the reservation is what the move goes through.
        assert_eq!(read(&dir.join("into/main.rs")), "already here");
        assert_eq!(read(&dir.join("into/main copy.rs")), "moving");
        assert!(!dir.join("main.rs").exists());
    }

    /// A cut pasted back into its own folder does nothing at all.
    ///
    /// Not a duplicate (that would be a file nobody asked for) and not an error (it is already
    /// true). The clipboard's owner still gets an entry, so it can clear itself and select the
    /// row exactly as it would after a real move.
    #[test]
    fn a_cut_pasted_where_it_already_is_is_a_no_op() {
        let dir = scratch("copy-cut-same-folder");
        let roots = roots_of(dir.path());
        std::fs::write(dir.join("main.rs"), "body").unwrap();

        let out = paste(&roots, &[dir.join("main.rs")], dir.path(), PasteMode::Cut).unwrap();
        assert_eq!(out[0].dest, dir.join("main.rs"));
        assert!(!out[0].renamed);
        assert_eq!(read(&dir.join("main.rs")), "body");
        assert!(
            !dir.join("main copy.rs").exists(),
            "a no-op made a duplicate"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_project_root_can_be_copied_but_never_cut() {
        let dir = scratch("copy-root");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();

        // Moving a root would leave the workspace pointing at nothing, and `Index` will not
        // even remove the row. Refused where rename and delete refuse it.
        assert!(matches!(
            paste(
                &roots,
                &[dir.path().to_path_buf()],
                &dir.join("into"),
                PasteMode::Cut
            ),
            Err(FsError::IsRoot(_))
        ));
        // And copying one into itself is the recursion guard, not the root rule.
        assert!(matches!(
            paste(
                &roots,
                &[dir.path().to_path_buf()],
                &dir.join("into"),
                PasteMode::Copy
            ),
            Err(FsError::IntoItself(_))
        ));
    }

    #[test]
    fn a_paste_into_something_that_is_not_a_directory_is_refused_with_a_reason() {
        let dir = scratch("copy-bad-dest");
        let roots = roots_of(dir.path());
        std::fs::write(dir.join("a.rs"), "a").unwrap();
        std::fs::write(dir.join("notadir"), "x").unwrap();

        let Err(FsError::Io { message, .. }) = paste(
            &roots,
            &[dir.join("a.rs")],
            &dir.join("notadir"),
            PasteMode::Copy,
        ) else {
            panic!("a file was accepted as a paste destination");
        };
        assert!(message.contains("not a directory"), "{message}");
        assert_eq!(read(&dir.join("notadir")), "x");
    }

    #[test]
    fn a_source_that_vanished_is_reported_and_pastes_nothing() {
        let dir = scratch("copy-gone");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("stays.rs"), "s").unwrap();

        // Both sources are checked before either is copied, so the good one does not land.
        assert!(matches!(
            paste(
                &roots,
                &[dir.join("stays.rs"), dir.join("gone.rs")],
                &dir.join("into"),
                PasteMode::Copy
            ),
            Err(FsError::Io { .. })
        ));
        assert!(
            !dir.join("into/stays.rs").exists(),
            "a selection with one bad path in it pasted half of itself"
        );
    }

    /// Permissions survive a copy; the executable bit is the one anybody notices.
    #[cfg(unix)]
    #[test]
    fn a_copy_keeps_the_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("copy-perms");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        paste(
            &roots,
            &[dir.join("run.sh")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        let mode = std::fs::metadata(dir.join("into/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "{mode:o}");
    }

    /// A fifo inside a directory is skipped and counted, not copied and not fatal.
    ///
    /// Copying it means `read(2)` blocking until somebody writes to the other end, which from
    /// the user's side is a paste that hangs with no message. Refusing the directory instead
    /// would make a project holding one `.sock` uncopyable.
    #[cfg(unix)]
    #[test]
    fn a_special_file_is_skipped_and_counted_rather_than_hanging_the_paste() {
        let dir = scratch("copy-fifo");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("pkg")).unwrap();
        std::fs::write(dir.join("pkg/a.rs"), "a").unwrap();
        std::fs::create_dir(dir.join("into")).unwrap();

        let path = std::ffi::CString::new(dir.join("pkg/pipe").to_string_lossy().as_bytes())
            .expect("a scratch path has no NUL in it");
        // SAFETY: a plain libc call with a NUL-terminated path this test just built.
        let made = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "could not make a fifo to test with");

        let out = paste(
            &roots,
            &[dir.join("pkg")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(out[0].skipped, 1, "the fifo was not reported as skipped");
        assert_eq!(read(&dir.join("into/pkg/a.rs")), "a");
        assert!(!dir.join("into/pkg/pipe").exists());
    }

    // --- the confirmation: asking first, and the three answers ---------------------------
    //
    // > *"Paste collisions - yes, should be a confirmation"*
    //
    // The dialog is `ui/src/chrome/PasteConfirm.tsx` and its wording is pinned by
    // `check-fs-clipboard.mjs`. What is pinned *here* is the half a dialog cannot promise:
    // that the question can be asked without writing anything, that Replace is reachable only
    // by asking for it, and that backing out leaves the disk exactly as it was.

    fn replace(source: PathBuf) -> PasteDecision {
        PasteDecision {
            source,
            choice: PasteChoice::Replace,
        }
    }

    /// `plan` reads, and only reads.
    ///
    /// The whole ordering rests on this: the questions are answered before the first byte is
    /// written, so *Cancel* is a command that is never sent rather than an apology for three
    /// files that already landed. A `plan` that created so much as a placeholder would make
    /// cancelling a thing with consequences.
    #[test]
    fn planning_a_paste_writes_nothing_at_all() {
        let dir = scratch("plan-writes-nothing");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("main.rs"), "new").unwrap();
        std::fs::write(dir.join("into/main.rs"), "already here").unwrap();
        std::fs::write(dir.join("fresh.rs"), "f").unwrap();

        let before: Vec<String> = names_in(&dir.join("into"));
        let collisions = plan(
            &roots,
            &[dir.join("main.rs"), dir.join("fresh.rs")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();

        assert_eq!(collisions.len(), 1, "only the taken name is a question");
        assert_eq!(collisions[0].name, "main.rs");
        assert_eq!(collisions[0].dest, dir.join("into/main.rs"));
        assert_eq!(collisions[0].replaces, 1);
        assert!(!collisions[0].merge);
        assert!(collisions[0].blocked.is_none());

        // The destination is byte-for-byte what it was, and nothing new appeared beside it —
        // no placeholder, no `.tmp`, no `main copy.rs` reserved in advance.
        assert_eq!(read(&dir.join("into/main.rs")), "already here");
        assert_eq!(names_in(&dir.join("into")), before);
    }

    /// The answer the user asked for, and the one that shipped instead.
    #[test]
    fn replace_overwrites_and_keep_both_still_renames() {
        let dir = scratch("replace-file");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("main.rs"), "new").unwrap();
        std::fs::write(dir.join("into/main.rs"), "old").unwrap();

        // Keep both is the default, and is what an empty decision list means.
        let kept = paste(
            &roots,
            &[dir.join("main.rs")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(kept[0].dest, dir.join("into/main copy.rs"));
        assert_eq!(kept[0].replaced, 0);
        assert_eq!(read(&dir.join("into/main.rs")), "old");

        let out = paste_with(
            &roots,
            &[dir.join("main.rs")],
            &dir.join("into"),
            PasteMode::Copy,
            &[replace(dir.join("main.rs"))],
        )
        .unwrap();
        assert_eq!(
            out[0].dest,
            dir.join("into/main.rs"),
            "the name did not move"
        );
        assert!(!out[0].renamed);
        assert_eq!(out[0].replaced, 1, "the caller has to be told what it cost");
        assert_eq!(read(&dir.join("into/main.rs")), "new");
        // The rename from the first paste is still there. Replace replaced one file, not the
        // folder's history.
        assert_eq!(read(&dir.join("into/main copy.rs")), "new");
        assert!(
            names_in(&dir.join("into"))
                .iter()
                .all(|n| !n.contains("tmp")),
            "the temporary the overwrite goes through was left behind"
        );
    }

    /// A decision names *one* source, and reaches no further than that.
    ///
    /// The bug this rules out is a policy flag: `overwrite: true` on the command would answer
    /// for every path in the selection, including the ones the user was never asked about.
    #[test]
    fn a_decision_replaces_only_the_source_it_names() {
        let dir = scratch("replace-scoped");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("a.rs"), "new a").unwrap();
        std::fs::write(dir.join("b.rs"), "new b").unwrap();
        std::fs::write(dir.join("into/a.rs"), "old a").unwrap();
        std::fs::write(dir.join("into/b.rs"), "old b").unwrap();

        let out = paste_with(
            &roots,
            &[dir.join("a.rs"), dir.join("b.rs")],
            &dir.join("into"),
            PasteMode::Copy,
            &[replace(dir.join("a.rs"))],
        )
        .unwrap();

        assert_eq!(read(&dir.join("into/a.rs")), "new a");
        assert_eq!(
            read(&dir.join("into/b.rs")),
            "old b",
            "b was never asked about"
        );
        assert_eq!(out[1].dest, dir.join("into/b copy.rs"));
        assert_eq!(out[1].replaced, 0);
    }

    /// *Apply to all remaining* is a list of decisions, and it covers exactly the rest.
    ///
    /// The frontend builds the list (`ui/src/chrome/pasteConfirmModel.ts`); this is the half that
    /// has to honour it — three answers arriving as three entries and three overwrites, with
    /// nothing renamed and nothing asked about twice.
    #[test]
    fn answering_the_rest_at_once_replaces_every_one_of_them() {
        let dir = scratch("replace-all");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        let sources: Vec<PathBuf> = ["a.rs", "b.rs", "c.rs"]
            .iter()
            .map(|name| {
                std::fs::write(dir.join(name), format!("new {name}")).unwrap();
                std::fs::write(dir.join("into").join(name), "old").unwrap();
                dir.join(name)
            })
            .collect();

        let decisions: Vec<PasteDecision> = sources.iter().cloned().map(replace).collect();
        let out = paste_with(
            &roots,
            &sources,
            &dir.join("into"),
            PasteMode::Copy,
            &decisions,
        )
        .unwrap();

        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|e| e.replaced == 1 && !e.renamed));
        assert_eq!(read(&dir.join("into/a.rs")), "new a.rs");
        assert_eq!(read(&dir.join("into/c.rs")), "new c.rs");
        assert_eq!(
            names_in(&dir.join("into")).len(),
            3,
            "a rename crept in beside an overwrite"
        );
    }

    /// Replacing a folder with a folder **merges**, and the count says so beforehand.
    ///
    /// The rejected reading is `remove_dir_all(dest)` followed by a plain paste. It is shorter,
    /// it is what "replace" sounds like, and it deletes every file in the destination that the
    /// source happens not to contain — a set the user was never shown. `keeps` is the number
    /// that makes the difference visible in the dialog.
    #[test]
    fn replacing_a_folder_merges_and_leaves_what_only_the_destination_had() {
        let dir = scratch("replace-merge");
        let roots = roots_of(dir.path());
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "new main").unwrap();
        std::fs::write(dir.join("src/added.rs"), "brand new").unwrap();
        std::fs::write(dir.join("src/deep/nested.rs"), "new nested").unwrap();

        std::fs::create_dir_all(dir.join("into/src/deep")).unwrap();
        std::fs::write(dir.join("into/src/main.rs"), "old main").unwrap();
        std::fs::write(dir.join("into/src/theirs.rs"), "only theirs").unwrap();
        std::fs::write(dir.join("into/src/deep/nested.rs"), "old nested").unwrap();
        std::fs::write(dir.join("into/src/deep/keep.rs"), "only theirs, deeper").unwrap();

        let collisions = plan(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert!(collisions[0].merge, "a folder on a folder is a merge");
        assert_eq!(collisions[0].replaces, 2, "main.rs and deep/nested.rs");
        assert_eq!(collisions[0].keeps, 2, "theirs.rs and deep/keep.rs");
        assert!(!collisions[0].truncated);
        assert!(collisions[0].sample.contains(&"main.rs".to_string()));
        assert!(collisions[0].sample.contains(&"deep/nested.rs".to_string()));

        let out = paste_with(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
            &[replace(dir.join("src"))],
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/src"));
        assert_eq!(
            out[0].replaced, 2,
            "the count is the promise the dialog made"
        );

        assert_eq!(read(&dir.join("into/src/main.rs")), "new main");
        assert_eq!(read(&dir.join("into/src/deep/nested.rs")), "new nested");
        assert_eq!(read(&dir.join("into/src/added.rs")), "brand new");
        // The whole reason merge won: these two are not in the source and are still here.
        assert_eq!(read(&dir.join("into/src/theirs.rs")), "only theirs");
        assert_eq!(
            read(&dir.join("into/src/deep/keep.rs")),
            "only theirs, deeper"
        );
        assert!(
            !dir.join("into/src copy").exists(),
            "a replace also left a rename behind"
        );
    }

    /// A file for a folder is not a replacement of anything, and is refused before any write.
    #[test]
    fn swapping_a_file_for_a_folder_is_refused_rather_than_guessed() {
        let dir = scratch("replace-mismatch");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "a").unwrap();
        std::fs::write(dir.join("into/src"), "a FILE called src").unwrap();

        // The dialog is told first, so it can offer Keep both and Cancel and nothing else.
        let collisions = plan(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        let blocked = collisions[0].blocked.as_deref().expect("a refusal");
        assert!(blocked.contains("folder"), "{blocked}");

        // And the domain refuses it even if the answer arrives anyway — the dialog is the
        // courtesy, this is the guarantee.
        assert!(matches!(
            paste_with(
                &roots,
                &[dir.join("src")],
                &dir.join("into"),
                PasteMode::Copy,
                &[replace(dir.join("src"))],
            ),
            Err(FsError::CannotReplace(_))
        ));
        assert_eq!(read(&dir.join("into/src")), "a FILE called src");

        // Keep both is still available, and is what the dialog offers instead.
        let out = paste(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/src copy"));
    }

    /// The same refusal, one level down inside a merge.
    ///
    /// This is the one the recursion would otherwise discover half way through, with everything
    /// above it already overwritten. `plan` walks the pair, so the dialog never offers Replace
    /// and the paste refuses before the first byte.
    #[test]
    fn a_mismatch_inside_a_merge_blocks_the_replace_before_it_starts() {
        let dir = scratch("replace-mismatch-nested");
        let roots = roots_of(dir.path());
        std::fs::create_dir_all(dir.join("src/mod")).unwrap();
        std::fs::write(dir.join("src/mod/a.rs"), "a").unwrap();
        std::fs::write(dir.join("src/top.rs"), "new top").unwrap();
        std::fs::create_dir_all(dir.join("into/src")).unwrap();
        std::fs::write(dir.join("into/src/mod"), "a FILE where a folder is").unwrap();
        std::fs::write(dir.join("into/src/top.rs"), "old top").unwrap();

        let collisions = plan(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        let blocked = collisions[0].blocked.as_deref().expect("a refusal");
        assert!(
            blocked.contains("mod"),
            "the offending path is named: {blocked}"
        );

        assert!(matches!(
            paste_with(
                &roots,
                &[dir.join("src")],
                &dir.join("into"),
                PasteMode::Copy,
                &[replace(dir.join("src"))],
            ),
            Err(FsError::CannotReplace(_))
        ));
        // Nothing above the mismatch was touched, which is the point of refusing in the
        // pre-flight pass rather than when the recursion trips over it.
        assert_eq!(read(&dir.join("into/src/top.rs")), "old top");
        assert_eq!(read(&dir.join("into/src/mod")), "a FILE where a folder is");
    }

    /// The preview's budget must not become the refusal's budget.
    ///
    /// `plan` stops counting at [`PREVIEW_LIMIT`] so the dialog opens at once, and says so with
    /// `truncated`. Reading `blocked` off that same bounded walk is what made a folder pair
    /// bigger than the budget merge right up to the file-versus-folder clash and *then* refuse:
    /// the caller saw a `CannotReplace` that reads as "nothing happened" while the destination
    /// was half overwritten and, for a cut, the source half emptied. So `check_replaceable`
    /// walks with no budget, and this pins the two apart with a budget of zero — the smallest
    /// truncation there is, and the one every real one behaves like.
    #[test]
    fn a_truncated_preview_never_becomes_permission_to_merge() {
        let dir = scratch("replace-truncated-preview");
        let roots = roots_of(dir.path());
        std::fs::create_dir_all(dir.join("src/mod")).unwrap();
        std::fs::write(dir.join("src/mod/a.rs"), "a").unwrap();
        std::fs::write(dir.join("src/top.rs"), "new top").unwrap();
        std::fs::create_dir_all(dir.join("into/src")).unwrap();
        std::fs::write(dir.join("into/src/mod"), "a FILE where a folder is").unwrap();
        std::fs::write(dir.join("into/src/top.rs"), "old top").unwrap();

        // What the dialog would be handed if the walk gave up immediately: an honest
        // `truncated`, and no idea that a refusal is waiting one level down.
        let peeked = collision_for(&dir.join("src"), &dir.join("into"), 0)
            .unwrap()
            .expect("a folder on a folder");
        assert!(peeked.truncated, "a zero budget has to report itself");
        assert!(
            peeked.blocked.is_none(),
            "the budget stopped before the mismatch, which is the whole premise"
        );

        // The paste refuses anyway, and refuses *first*: `top.rs` is what it was.
        let out = paste_with(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Copy,
            &[replace(dir.join("src"))],
        );
        assert!(matches!(out, Err(FsError::CannotReplace(_))), "{out:?}");
        assert_eq!(
            read(&dir.join("into/src/top.rs")),
            "old top",
            "the merge started before the refusal and overwrote a file"
        );
        assert!(!dir.join("into/src/mod").is_dir());

        // And the budget the production refusal walks with. Pinned as a constant rather than
        // through a fixture: a folder pair has to hold more than PREVIEW_LIMIT entries before
        // a bounded refusal starts lying, `read_dir` order decides whether it lies on any given
        // run, and a test that fails four times in five is not a test.
        assert_eq!(
            REFUSAL_LIMIT,
            u32::MAX,
            "the refusal walk must not inherit the preview's budget"
        );
    }

    /// A cut answered *Replace* moves over the destination and leaves nothing behind.
    #[test]
    fn a_cut_can_replace_and_the_source_is_gone_afterwards() {
        let dir = scratch("replace-cut");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("main.rs"), "moving").unwrap();
        std::fs::write(dir.join("into/main.rs"), "old").unwrap();

        let out = paste_with(
            &roots,
            &[dir.join("main.rs")],
            &dir.join("into"),
            PasteMode::Cut,
            &[replace(dir.join("main.rs"))],
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/main.rs"));
        assert_eq!(out[0].replaced, 1);
        assert_eq!(read(&dir.join("into/main.rs")), "moving");
        assert!(!dir.join("main.rs").exists(), "the source survived a cut");
        assert_eq!(names_in(&dir.join("into")).len(), 1);
    }

    /// A cut folder answered *Replace* merges, and the emptied source folder goes.
    #[test]
    fn a_cut_folder_merges_and_removes_what_it_emptied() {
        let dir = scratch("replace-cut-merge");
        let roots = roots_of(dir.path());
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "moving").unwrap();
        std::fs::write(dir.join("src/deep/x.rs"), "moving deep").unwrap();
        std::fs::create_dir_all(dir.join("into/src")).unwrap();
        std::fs::write(dir.join("into/src/main.rs"), "old").unwrap();
        std::fs::write(dir.join("into/src/theirs.rs"), "kept").unwrap();

        paste_with(
            &roots,
            &[dir.join("src")],
            &dir.join("into"),
            PasteMode::Cut,
            &[replace(dir.join("src"))],
        )
        .unwrap();

        assert_eq!(read(&dir.join("into/src/main.rs")), "moving");
        assert_eq!(read(&dir.join("into/src/deep/x.rs")), "moving deep");
        assert_eq!(read(&dir.join("into/src/theirs.rs")), "kept");
        assert!(
            !dir.join("src").exists(),
            "a cut left the folder it emptied behind"
        );
    }

    /// The duplicate gesture is never a question.
    ///
    /// Pasting a file into the folder it is already in is how anybody duplicates one, and the
    /// only honest answer to "replace main.rs with main.rs?" is the rename. So it is not
    /// reported as a collision at all, and a `Replace` sent for one anyway is ignored rather
    /// than copying a file over itself through a temporary.
    #[test]
    fn duplicating_a_file_beside_itself_is_never_asked_about() {
        let dir = scratch("replace-duplicate");
        let roots = roots_of(dir.path());
        std::fs::write(dir.join("main.rs"), "body").unwrap();

        let collisions = plan(&roots, &[dir.join("main.rs")], dir.path(), PasteMode::Copy).unwrap();
        assert!(collisions.is_empty(), "{collisions:?}");

        let out = paste_with(
            &roots,
            &[dir.join("main.rs")],
            dir.path(),
            PasteMode::Copy,
            &[replace(dir.join("main.rs"))],
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("main copy.rs"));
        assert_eq!(out[0].replaced, 0);
        assert_eq!(read(&dir.join("main.rs")), "body");
    }

    /// A `Replace` whose destination vanished between the question and the answer.
    ///
    /// It claims the free name like any other paste rather than erroring or inventing a file to
    /// overwrite. The window is small and real: a `git checkout` between the dialog opening and
    /// the user clicking is all it takes.
    #[test]
    fn a_replace_whose_destination_disappeared_just_pastes() {
        let dir = scratch("replace-vanished");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("main.rs"), "new").unwrap();

        let out = paste_with(
            &roots,
            &[dir.join("main.rs")],
            &dir.join("into"),
            PasteMode::Copy,
            &[replace(dir.join("main.rs"))],
        )
        .unwrap();
        assert_eq!(out[0].dest, dir.join("into/main.rs"));
        assert_eq!(out[0].replaced, 0, "nothing was there to replace");
        assert!(!out[0].renamed);
    }

    /// A multi-source paste that stops part way says what already landed.
    ///
    /// Not the dialog's cancel — that writes nothing, because every question is answered before
    /// the first byte. This is the other half: once the writing has started it cannot be undone,
    /// and the failure carries the destinations that already exist so the panel can say so
    /// instead of reporting a clean failure over a folder that is half full of new files.
    #[test]
    fn a_paste_that_stops_part_way_reports_what_already_landed() {
        let dir = scratch("replace-partial");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("main.rs"), "moving").unwrap();
        std::fs::write(dir.join("into/main.rs"), "old").unwrap();

        // The same source twice: both pass the pre-flight checks, the first cut moves it, and
        // the second finds nothing there. A duplicated selection is a frontend bug, and this is
        // what the user is told when one happens rather than "nothing was pasted".
        let err = paste_with(
            &roots,
            &[dir.join("main.rs"), dir.join("main.rs")],
            &dir.join("into"),
            PasteMode::Cut,
            &[replace(dir.join("main.rs"))],
        )
        .expect_err("the second copy of the source is gone");

        let FsError::PartialPaste { pasted, .. } = err else {
            panic!("a paste that half-happened was reported as if nothing had: {err:?}");
        };
        assert_eq!(pasted, vec![dir.join("into/main.rs").display().to_string()]);
        assert_eq!(read(&dir.join("into/main.rs")), "moving");
    }

    /// A folder pasted into itself is refused by `plan` too, not asked about.
    ///
    /// The dialog must never be the thing that surfaces a refusal the user cannot answer: a
    /// question with only wrong answers in it is worse than the rejection it was hiding.
    #[test]
    fn the_plan_refuses_what_the_paste_refuses() {
        let dir = scratch("plan-refusals");
        let roots = roots_of(dir.path());
        let outside = scratch("plan-elsewhere");
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(outside.join("secret"), "s").unwrap();

        assert!(matches!(
            plan(
                &roots,
                &[dir.join("src")],
                &dir.join("src/deep"),
                PasteMode::Copy
            ),
            Err(FsError::IntoItself(_))
        ));
        assert!(matches!(
            plan(
                &roots,
                &[outside.join("secret")],
                dir.path(),
                PasteMode::Copy
            ),
            Err(FsError::OutsideProject(_))
        ));
        assert!(matches!(
            plan(
                &roots,
                &[dir.path().to_path_buf()],
                &dir.join("src"),
                PasteMode::Cut
            ),
            Err(FsError::IsRoot(_))
        ));
    }

    #[test]
    fn several_sources_paste_in_order_and_report_one_entry_each() {
        let dir = scratch("copy-many");
        let roots = roots_of(dir.path());
        std::fs::create_dir(dir.join("into")).unwrap();
        std::fs::write(dir.join("a.rs"), "a").unwrap();
        std::fs::create_dir(dir.join("b")).unwrap();
        std::fs::write(dir.join("b/inner.rs"), "i").unwrap();

        let out = paste(
            &roots,
            &[dir.join("a.rs"), dir.join("b")],
            &dir.join("into"),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(out.len(), 2, "one entry per source, not per file on disk");
        assert_eq!(out[0].source, dir.join("a.rs"));
        assert_eq!(out[1].dest, dir.join("into/b"));
        assert_eq!(read(&dir.join("into/b/inner.rs")), "i");
    }
}
