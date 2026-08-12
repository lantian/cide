//! Copy, cut and paste of the files themselves. (M8, the tree's clipboard)
//!
//! The file tree could make, rename and trash entries but not duplicate or move them, so
//! Ctrl+C/Ctrl+V in the panel did nothing at all. This is the disk half of that gesture. The
//! frontend half — what is on the clipboard, which folder a paste lands in, what the row says
//! afterwards — is `ui/src/sidebar/clipboardModel.ts`.
//!
//! # The four decisions, and what lost
//!
//! **A collision never overwrites, and never refuses.** Pasting `main.rs` into a directory
//! that already has one produces `main copy.rs`, then `main copy 2.rs`. The two alternatives
//! were both worse from here: *overwrite* destroys a file that is not the one the user was
//! pointing at, and this layer has no way to ask — a confirmation dialog belongs to a webview
//! that has already sent the command. *Refuse* is safe and makes the ordinary gesture (paste
//! a file beside itself to duplicate it) impossible, which is how a paste ends up feeling
//! broken. Finder and VS Code both rename; so does this, and [`PastedEntry::renamed`] carries
//! the fact back so the panel can say what happened rather than leaving the user to spot it.
//!
//! The renaming is not a check-then-create. The name is *claimed* with `create_new`/
//! `create_dir`, which fail atomically when something is already there, and the loop moves to
//! the next candidate — so two pastes racing each other, or a `git checkout` landing between
//! the check and the write, cannot end with one clobbering the other. That is the whole reason
//! the reservation exists rather than a `Path::exists` call.
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

use std::fs::Metadata;
use std::path::{Path, PathBuf};

use cide_ipc::{PasteMode, PastedEntry};

use crate::error::{FsError, Result};
use crate::ops;

/// How many `name copy N` candidates are tried before giving up.
///
/// A thousand is far past any real gesture and still finite: without a cap, a directory that
/// somehow held every candidate would spin forever inside a blocking worker, which presents as
/// a hung paste with no error — the failure mode this whole crate keeps writing comments about.
const MAX_CANDIDATES: u32 = 1000;

/// Paste `sources` into `dest_dir`.
///
/// Every source is checked before any source moves, so a selection with one bad path in it
/// pastes nothing rather than half of itself — the same rule `ops::delete`'s caller follows,
/// and for the same reason: a partial result the caller was told nothing about is what leaves
/// a tree drawing files that are not there.
///
/// Once the work starts a failure part way through cannot be undone. What did land is carried
/// in [`FsError::PartialPaste`] rather than discarded, unless *nothing* landed, in which case
/// the caller gets the real error instead of a wrapper claiming a partial success of zero.
pub fn paste(
    roots: &[PathBuf],
    sources: &[PathBuf],
    dest_dir: &Path,
    mode: PasteMode,
) -> Result<Vec<PastedEntry>> {
    check_dest(roots, dest_dir)?;
    for source in sources {
        check_source(roots, source, dest_dir, mode)?;
    }

    let mut done: Vec<PastedEntry> = Vec::with_capacity(sources.len());
    for source in sources {
        match paste_one(source, dest_dir, mode) {
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

fn paste_one(source: &Path, dest_dir: &Path, mode: PasteMode) -> Result<PastedEntry> {
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
        });
    }

    let directory = meta.is_dir();
    let (dest, renamed) = reserve(dest_dir, name, directory)?;
    match place(source, &dest, &meta, mode) {
        Ok(skipped) => Ok(PastedEntry {
            source: source.to_path_buf(),
            dest,
            renamed,
            skipped,
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
        let made = if directory {
            std::fs::create_dir(&candidate)
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
                .map(|_| ())
        };
        match made {
            Ok(()) => return Ok((candidate, attempt > 0)),
            // Includes a *dangling* symlink sitting on the name, which `Path::exists` reports
            // as absent. It is a real directory entry and the kernel says so.
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(FsError::io(&candidate, err)),
        }
    }
    Err(FsError::Io {
        path: dir.display().to_string(),
        message: format!("already holds {MAX_CANDIDATES} entries named after “{name}”"),
    })
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
        let name = name_of(dest)?;
        let tmp = dir.join(format!(".{}.cide-link-{}.tmp", name, std::process::id()));
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
        std::fs::set_permissions(&dir.join("run.sh"), std::fs::Permissions::from_mode(0o755))
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
