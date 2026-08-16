//! Reading and writing the text files the editor opens. (M9)
//!
//! Deliberately not part of `cide-fs`, which is the *index*: a gitignore-aware walk over a
//! whole repository, with a watcher and a windowed row model. This is the other half of the
//! same subject and shares none of its machinery — one file at a time, by absolute path,
//! with no cache and no state.
//!
//! Two rules shape everything below, and both exist because the alternative silently
//! destroys a file the user did not ask to change:
//!
//! * The text crosses the wire byte-for-byte. Line endings are the editor's problem to
//!   restore, because CodeMirror is the thing that eats them, and a normalising read here
//!   would take away the only record of what they were.
//! * The write is atomic *and* follows symlinks. Atomic alone would replace a symlinked
//!   file with a regular one — a very common shape in a source tree — so the link is
//!   resolved first and the temp file is created beside the real target.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cide_ipc::{FileDoc, FileStamp};

use crate::error::{CoreError, Result};

/// The largest file the editor will open.
///
/// The frontend degrades highlighting well past the point where it is useful, but a buffer
/// still costs several times the file's size in DOM, undo history and JS strings, and a
/// webview that runs out of memory takes every terminal in the window with it. 32 MiB is
/// far above any hand-written source file and far below the size at which that happens; a
/// user who genuinely wants to look at a 200 MB log is better served by a pager.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// How much of a file is sniffed for the NUL byte that says it is not text.
///
/// A whole-file scan would be exact and would also read 32 MiB to reject an ELF binary
/// whose fifth byte already said so. Every binary format in practice has structure in its
/// header; a file that is genuinely text for 8 KiB and binary afterwards is a corrupt text
/// file, and opening it is the honest outcome.
const SNIFF_BYTES: usize = 8 * 1024;

/// Read a file as a text document.
///
/// Fails rather than guessing for the three cases that are not editable text: too large,
/// not UTF-8, and binary. `CoreError::Io` carries the sentence in each case, because the
/// frontend's only reasonable response to all three is to say so.
pub fn read(path: &Path) -> Result<FileDoc> {
    let meta = fs::metadata(path)?;
    if meta.is_dir() {
        return Err(CoreError::Io(format!(
            "{} is a directory, not a file",
            path.display()
        )));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(CoreError::Io(format!(
            "{} is {} MiB; the editor opens files up to {} MiB",
            path.display(),
            meta.len() / (1024 * 1024),
            MAX_FILE_BYTES / (1024 * 1024)
        )));
    }

    // Read once, into a byte buffer, and decide afterwards. `fs::read_to_string` would do
    // the UTF-8 check but reports a failure that cannot be told apart from an IO error, and
    // "this file is not text" and "this disk is failing" want different sentences.
    let mut bytes = Vec::with_capacity(meta.len() as usize + 1);
    File::open(path)?.read_to_end(&mut bytes)?;

    if looks_binary(&bytes) {
        return Err(CoreError::Io(format!(
            "{} looks like a binary file",
            path.display()
        )));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| CoreError::Io(format!("{} is not valid UTF-8", path.display())))?;

    Ok(FileDoc {
        path: path.to_path_buf(),
        text,
        writable: !meta.permissions().readonly(),
        stamp: stamp_of(&meta),
    })
}

/// The "is this still the file I read" token, from metadata already in hand.
///
/// `None` rather than a zero for a filesystem that will not answer, and the distinction is
/// load-bearing: [`write_if_unchanged`] treats `None` on either side as "no precondition to
/// check" and writes. A zero would compare equal to the next unavailable stamp and would
/// silently claim the file had not moved.
///
/// Nanoseconds since the epoch, saturating at both ends. A pre-epoch mtime is a real thing on
/// a badly restored archive; clamping it to 0 makes it equal to every other pre-epoch mtime,
/// which costs a missed conflict on files nobody edits and never a false one.
pub fn stamp_of(meta: &fs::Metadata) -> Option<FileStamp> {
    let modified = meta.modified().ok()?;
    let nanos = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    Some(FileStamp {
        mtime_nanos: nanos,
        len: meta.len(),
    })
}

/// The file's stamp right now, or `None` if it cannot be read.
pub fn stamp_at(path: &Path) -> Option<FileStamp> {
    // Through `canonicalize`, so a symlinked buffer compares the stamp of the file the write
    // will actually land on rather than the link's own. `document::write` resolves the same way,
    // and the two disagreeing would be a precondition checked against a different inode.
    let target = fs::canonicalize(path).ok()?;
    stamp_of(&fs::metadata(&target).ok()?)
}

/// Whether a file's leading bytes say it is not text.
///
/// A NUL is the test every editor uses and the only one that is close to reliable: UTF-8
/// text never contains one, and every common binary format has one within its first few
/// hundred bytes. It is deliberately *not* a heuristic over the ratio of control
/// characters, which rejects legitimate files full of terminal escapes.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0)
}

/// Write a document back, atomically, without replacing a symlink with a regular file.
///
/// The temp-then-rename dance is the same one `persist::save_atomic` does for the workspace
/// file, and it is repeated rather than shared because the two differ in the parts that
/// matter here: this one resolves symlinks, and it carries the original file's permission
/// bits across, which a freshly created temp file would otherwise silently reset to the
/// process umask. Losing the executable bit on a shell script is a real bug and a quiet one.
pub fn write(path: &Path, text: &str) -> Result<()> {
    // `canonicalize` on the *target*, so editing a symlinked file writes through the link
    // rather than replacing it. It also requires the file to exist, which is correct here:
    // this only ever saves a buffer that was read from disk.
    let target = fs::canonicalize(path)?;
    let perms = fs::metadata(&target).map(|m| m.permissions()).ok();

    let tmp = temp_path(&target);
    let write_result = (|| -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        if let Some(perms) = perms.clone() {
            file.set_permissions(perms)?;
        }
        file.write_all(text.as_bytes())?;
        // Without this the rename can publish a name whose bytes have not reached the disk,
        // and a crash leaves a file that exists and is empty — strictly worse than the
        // half-written one the atomic write was meant to prevent.
        file.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }

    if let Err(e) = fs::rename(&tmp, &target) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// [`write`], but only if the file on disk is still the one the buffer was read from.
///
/// Returns the file's new stamp on success, so the caller's token moves with the write and the
/// *next* save compares against what this one produced rather than against what the file was
/// half a minute ago.
///
/// `expect: None` writes unconditionally, which is what an explicit Ctrl+S passes: that is the
/// user deciding, and a modal in front of a keystroke they typed on purpose is a modal for the
/// wrong half of the problem. Autosave passes the token it was handed.
///
/// # Why a compare here and not a watcher subscription
///
/// See [`cide_ipc::FileStamp`]. In short: our own write emits a watcher event, so a subscription
/// needs a self-write suppression window, and a suppression window is a race with a timer in it.
///
/// # What this deliberately does not close
///
/// The gap between the `stat` here and the `rename` below. Another process can replace the file
/// in it, and nothing short of taking a lock the rest of the system does not honour would stop
/// that. What this closes is the case that actually happens: a `cargo fmt` that ran *while the
/// user was looking at their terminal*, seconds or minutes before the blur that triggers the
/// save. Narrowing a race from minutes to microseconds is the whole of what is on offer, and it
/// is worth having.
pub fn write_if_unchanged(
    path: &Path,
    text: &str,
    expect: Option<FileStamp>,
) -> Result<Option<FileStamp>> {
    if let Some(expect) = expect {
        // `None` here is a filesystem that will not answer, not a mismatch — see `stamp_of`.
        // Refusing to save because a `stat` was unhelpful would be worse than the race.
        if let Some(actual) = stamp_at(path)
            && actual != expect
        {
            return Err(CoreError::FileChanged {
                path: path.display().to_string(),
            });
        }
    }
    write(path, text)?;
    Ok(stamp_at(path))
}

/// A sibling temp file, unique to one call.
///
/// A sibling because `rename` is only atomic within one filesystem, and `/tmp` is very often
/// a different one. Unique because two windows can save the same file at the same moment,
/// and a shared temp name would let one `File::create` truncate the other's bytes before
/// either rename ran.
fn temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(0);

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cide-doc-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn reads_text_verbatim_including_crlf() {
        let path = tempdir().join("crlf.txt");
        fs::write(&path, b"one\r\ntwo\r\n").expect("write");

        let doc = read(&path).expect("read");
        // The whole point: nothing between the disk and the editor is allowed to normalise.
        assert_eq!(doc.text, "one\r\ntwo\r\n");
        assert!(doc.writable);
    }

    /*
     * The precondition autosave writes with. Four cases, and the fourth is the whole feature.
     *
     * Every one of these is about *autosave*, not about Ctrl+S: an explicit save passes `None`
     * and is unaffected by all of it.
     */

    #[test]
    fn a_write_with_no_precondition_is_unconditional() {
        // What Ctrl+S passes. The user typed the keystroke on purpose, and a modal in front of
        // it would be a modal for the wrong half of the problem.
        let path = tempdir().join("uncond.txt");
        fs::write(&path, b"before").expect("seed");
        let stamp = write_if_unchanged(&path, "after", None).expect("writes");
        assert_eq!(fs::read_to_string(&path).expect("read"), "after");
        assert!(stamp.is_some(), "and the new stamp comes back");
    }

    #[test]
    fn a_matching_precondition_writes_and_hands_back_the_new_stamp() {
        let path = tempdir().join("match.txt");
        fs::write(&path, b"before").expect("seed");
        let opened = read(&path).expect("read").stamp;
        assert!(
            opened.is_some(),
            "an ordinary temp file has a readable mtime"
        );

        let after = write_if_unchanged(&path, "after", opened).expect("writes");
        assert_eq!(fs::read_to_string(&path).expect("read"), "after");
        assert_ne!(
            after, opened,
            "the token has to move with the write, or the *next* autosave compares against what \
             the file was when the tab opened and refuses for ever"
        );
    }

    #[test]
    fn a_stale_precondition_refuses_and_leaves_the_file_alone() {
        // THE ONE THAT MATTERS. This is `cargo fmt` in a shell pane, followed by the user
        // clicking back into the editor — which with autosave-on-blur is a *save*, and without
        // this check is a silent overwrite of the formatting.
        let path = tempdir().join("stale.txt");
        fs::write(&path, b"before").expect("seed");
        let opened = read(&path).expect("read").stamp;

        // Something else rewrites it. The length differs, so this is caught even on a
        // filesystem whose mtime granularity swallowed the interval.
        fs::write(&path, b"formatted by something else").expect("another process writes it");

        let refusal = write_if_unchanged(&path, "the buffer", opened).expect_err("refused");
        assert!(
            matches!(refusal, CoreError::FileChanged { .. }),
            "and it is a TAGGED refusal, not an `Io(String)`: the frontend has to tell \
             \"the file moved under you\" from \"the disk is full\" without matching on prose, \
             got {refusal:?}"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "formatted by something else",
            "and nothing was written"
        );
    }

    #[test]
    fn a_length_preserving_change_is_still_caught() {
        // Mtime alone would be enough here, and length alone would not — which is why the stamp
        // carries both. A `sed -i s/foo/bar/` is exactly this shape.
        let path = tempdir().join("samelen.txt");
        fs::write(&path, b"foofoo").expect("seed");
        let opened = read(&path).expect("read").stamp;
        // Far enough apart that no filesystem's mtime granularity can swallow it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, b"barbar").expect("another process writes it");

        assert!(
            write_if_unchanged(&path, "the buffer", opened).is_err(),
            "a same-length rewrite is the ordinary `sed -i` and must not slip through"
        );
    }

    #[test]
    fn a_file_that_cannot_be_stamped_is_written_rather_than_refused() {
        // `stamp_of` answers `None` for a filesystem that will not report an mtime, and the
        // whole point of the `Option` is that `None` means "no precondition to check" rather
        // than "assume the worst". Refusing to save because a `stat` was unhelpful would be
        // strictly worse than the race it guards against — the user loses their edit either
        // way, and one of the two ways is cide's own doing.
        let path = tempdir().join("nostamp.txt");
        fs::write(&path, b"before").expect("seed");
        assert!(write_if_unchanged(&path, "after", None).is_ok());
        assert_eq!(fs::read_to_string(&path).expect("read"), "after");
    }

    #[test]
    fn a_stamp_survives_the_round_trip_that_a_read_and_a_write_make_of_it() {
        // The two ends have to agree about *which* inode they are stamping. `write` resolves
        // symlinks with `canonicalize` so that editing a symlinked file writes through the link;
        // `stamp_at` therefore has to canonicalize too, or the precondition is compared against
        // the link's own metadata and every save through a symlink refuses.
        let dir = tempdir();
        let real = dir.join("real.txt");
        let link = dir.join("link.txt");
        fs::write(&real, b"before").expect("seed");
        let _ = fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        #[cfg(not(unix))]
        return;

        let opened = read(&link).expect("read").stamp;
        write_if_unchanged(&link, "after", opened)
            .expect("a save through a symlink is not a conflict");
        assert_eq!(fs::read_to_string(&real).expect("read"), "after");
        assert!(
            fs::symlink_metadata(&link).expect("stat").is_symlink(),
            "the link survived"
        );
    }

    #[test]
    fn rejects_binary_and_non_utf8() {
        let dir = tempdir();

        let binary = dir.join("binary.bin");
        fs::write(&binary, b"\x7fELF\0\0\0\0rest").expect("write");
        assert!(matches!(read(&binary), Err(CoreError::Io(_))));

        // No NUL, so the sniff passes and the UTF-8 decode is what has to catch it —
        // 0x80 is a continuation byte with nothing to continue.
        let latin1 = dir.join("latin1.txt");
        fs::write(&latin1, b"caf\x80 au lait").expect("write");
        let error = read(&latin1).expect_err("must reject");
        assert!(format!("{error}").contains("UTF-8"), "{error}");
    }

    #[test]
    fn round_trips_through_an_atomic_write() {
        let path = tempdir().join("round.txt");
        fs::write(&path, "before\n").expect("seed");

        write(&path, "after\r\nlines\r\n").expect("write");
        assert_eq!(read(&path).expect("read").text, "after\r\nlines\r\n");

        // The temp file is a sibling, so a failure to clean it up would show here rather
        // than accumulating dotfiles beside the user's source.
        let strays: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("read dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "left temp files behind");
    }

    #[test]
    fn rejects_a_directory() {
        let dir = tempdir();
        let error = read(&dir).expect_err("a directory is not a document");
        assert!(format!("{error}").contains("is a directory"), "{error}");
    }

    /// Pins what `writable` actually answers, because the tempting reading of it is wrong.
    ///
    /// It is `Permissions::readonly()` inverted — the mode bits and nothing else. It does
    /// not know about ownership or about a read-only mount, so the field must not be
    /// described (or relied on) as "this process can write this file".
    ///
    /// **Nor does it know about dependency caches**, and that is now deliberate rather than an
    /// oversight. `cargo` unpacks a crate mode 644, so the mode bits say a registry source is
    /// writable and it is not a file cide may write — the rule that says so is
    /// `crate::toolchain::read_only_reason`, and it is applied in `cide_app::cmd::file`, where
    /// the project's roots are known. Keeping it out of here is what lets a user who opened a
    /// vendored crate *as a project root* still save in it.
    #[cfg(unix)]
    #[test]
    fn writable_follows_the_mode_bits() {
        use std::os::unix::fs::PermissionsExt;

        let path = tempdir().join("readonly.txt");
        fs::write(&path, "text\n").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).expect("chmod");
        assert!(!read(&path).expect("read").writable);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(read(&path).expect("read").writable);
    }

    #[cfg(unix)]
    #[test]
    fn write_preserves_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;

        let path = tempdir().join("script.sh");
        fs::write(&path, "#!/bin/sh\necho one\n").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");

        write(&path, "#!/bin/sh\necho two\n").expect("write");

        let mode = fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "lost the mode across the rename");
    }

    #[cfg(unix)]
    #[test]
    fn write_follows_a_symlink_instead_of_replacing_it() {
        let dir = tempdir();
        let target = dir.join("real.txt");
        let link = dir.join("link.txt");
        fs::write(&target, "before\n").expect("seed");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        write(&link, "after\n").expect("write");

        assert_eq!(fs::read_to_string(&target).expect("read"), "after\n");
        assert!(
            fs::symlink_metadata(&link)
                .expect("stat")
                .file_type()
                .is_symlink(),
            "the atomic rename clobbered the symlink"
        );
    }
}
