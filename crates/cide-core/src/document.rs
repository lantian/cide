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

use cide_ipc::FileDoc;

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
    })
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
