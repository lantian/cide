//! Reading a path's properties: OS stat and, for a text file, what one pass over its bytes says.
//! (M70)
//!
//! The backend of the properties card. [`cide_ipc::properties`] is the shape of the answer and
//! argues the wire decisions; this is the reading, and it is built around three rules that are
//! each one convenient call away from being broken.
//!
//! # 1. `symlink_metadata`, never `metadata`
//!
//! [`crate::document::stamp_at`] canonicalises before it stats, which is right for a buffer —
//! a buffer's identity is the file it really edits — and fatal here. Reusing it would put the
//! **target's** size, mode, owner and mtime under the *link's* name and path, with nothing on
//! screen to suggest the substitution. There is no error, no gap and nothing logged: the card
//! is simply, confidently about a different file.
//!
//! So [`read`] stats the link itself and resolves the destination only into
//! `symlink_target`/`symlink_broken`, which are rows the reader can see.
//!
//! # 2. A file this large is described, not read
//!
//! The text pass reads the whole file, so it carries [`crate::document::MAX_FILE_BYTES`] — the
//! editor's own cap, reused rather than re-chosen, so "the properties card counted it" and "the
//! editor will open it" cannot drift apart. Past it, `text` is `None` and `text_skipped` carries
//! the sentence. An absent row tells the reader nothing they can act on; *"too large to count
//! (over 32 MiB)"* tells them the file is fine and the card is the thing with a limit.
//!
//! # 3. The line rules are `lineEndings.ts`'s rules
//!
//! `ui/src/editor/lineEndings.ts` already decides what a line is and what an ending is, for the
//! status bar. This counts the same three break shapes and calls more than one of them `Mixed`,
//! because the alternative is a status bar and a properties card disagreeing about the same file
//! six inches apart on one screen, with no way from the outside to tell which is lying.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use cide_ipc::properties::{DirSummary, FileProperties, LineEnding, Owner, PathKind, TextFacts};

use crate::document::MAX_FILE_BYTES;
use crate::error::{CoreError, Result};

/// How many entries [`dir_summary`] may look at before it gives up and says so.
///
/// Chosen to be large enough that an ordinary source directory is exact and small enough that
/// `node_modules/` cannot hang the card. `cide_fs::copy`'s preview budget is the precedent, and
/// the sentence there applies unchanged: a count that stops early is honest when it says so and
/// simply wrong when it does not.
pub const DIR_WALK_LIMIT: u32 = 50_000;

/// Everything one `symlink_metadata` — and, for a plausible text file, one read — can say.
///
/// The only error is a path that cannot be stat'ed at all. Everything else degrades into a
/// `None` with a sentence beside it, because a properties card that refuses to open is a worse
/// answer than one that admits a gap.
pub fn read(path: &Path) -> Result<FileProperties> {
    // `symlink_metadata`, for the reason in this module's header. Every field below is the
    // link's own when the path is a link.
    let meta = fs::symlink_metadata(path)?;
    let file_type = meta.file_type();

    let kind = if file_type.is_symlink() {
        PathKind::Symlink
    } else if file_type.is_dir() {
        PathKind::Dir
    } else if file_type.is_file() {
        PathKind::File
    } else {
        PathKind::Other
    };

    // `read_link` rather than `canonicalize`: the card shows what the link actually says, so a
    // relative target stays relative and a broken one still has a target to show. Canonicalising
    // would answer `None` for precisely the dangling link the row is most useful for.
    let (symlink_target, symlink_broken) = if matches!(kind, PathKind::Symlink) {
        let target = fs::read_link(path).ok();
        // Existence is asked through `metadata`, which *does* follow — the one place in this
        // module where following is the question rather than the mistake.
        let broken = fs::metadata(path).is_err();
        (target, Some(broken))
    } else {
        (None, None)
    };

    let (text, text_skipped) = match kind {
        PathKind::File => read_text_facts(path, meta.len()),
        // A directory has no lines, and saying so is noise rather than information: the card
        // draws the entry counts instead. Same for a fifo. A symlink's text facts would be the
        // target's, which is rule 1 in a second place.
        _ => (None, None),
    };

    Ok(FileProperties {
        name: file_name(path),
        path: path.to_path_buf(),
        kind,
        len: meta.len(),
        modified_unix_ms: system_time_ms(meta.modified().ok()),
        changed_unix_ms: changed_unix_ms(&meta),
        created_unix_ms: system_time_ms(meta.created().ok()),
        readonly: meta.permissions().readonly(),
        mode: mode_of(&meta),
        mode_string: mode_of(&meta).map(|m| mode_string(m, kind)),
        owner: owner_of(&meta),
        symlink_target,
        symlink_broken,
        text,
        text_skipped,
    })
}

/// The last component, or the whole path when there is no last component.
///
/// `/` and `C:\` have no file name, and a card headed with an empty string is a card that looks
/// broken. Computed here rather than in the webview so the heading and the tree row cannot
/// disagree about what a path is called.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Count lines and decide the ending, or say why not.
///
/// Returns the pair that goes into `text`/`text_skipped`: exactly one of them is `Some`.
fn read_text_facts(path: &Path, len: u64) -> (Option<TextFacts>, Option<String>) {
    if len > MAX_FILE_BYTES {
        return (
            None,
            Some(format!(
                "Too large to count — {} MiB, and the limit is {} MiB.",
                len / (1024 * 1024),
                MAX_FILE_BYTES / (1024 * 1024)
            )),
        );
    }

    let mut bytes = Vec::with_capacity(len as usize + 1);
    match fs::File::open(path).and_then(|mut f| f.read_to_end(&mut bytes)) {
        Ok(_) => {}
        // A file that cannot be read is an ordinary outcome — a permission bit, a vanished
        // path, a stalled mount — and it must not fail the whole card, which is still correct
        // about the size, the mode and the owner it already has.
        Err(e) => return (None, Some(format!("Could not be read — {e}."))),
    }

    // `document::looks_binary`'s rule, and deliberately its exact rule: a NUL in the first
    // 8 KiB. Counting "lines" in a binary is meaningless, and a card claiming a `.png` has
    // 4,312 lines is worse than one that says it is binary.
    if crate::document::looks_binary(&bytes) {
        return (None, Some("Binary — no lines to count.".to_string()));
    }

    (Some(text_facts(&bytes)), None)
}

/// The counting itself, over bytes already in hand. Split out so a test can drive it.
///
/// One pass, and it must stay one pass: `\r\n` is counted as a single break and neither of its
/// halves may also be counted, or every CRLF file reports twice the lines it has.
pub fn text_facts(bytes: &[u8]) -> TextFacts {
    let (mut lf, mut crlf, mut cr) = (0u64, 0u64, 0u64);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lf += 1;
                i += 1;
            }
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    crlf += 1;
                    i += 2;
                } else {
                    cr += 1;
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    let kinds = u8::from(lf > 0) + u8::from(crlf > 0) + u8::from(cr > 0);
    let ending = match (kinds, lf > 0, crlf > 0, cr > 0) {
        (0, ..) => LineEnding::None,
        (1, true, ..) => LineEnding::Lf,
        (1, _, true, _) => LineEnding::Crlf,
        (1, .., true) => LineEnding::Cr,
        _ => LineEnding::Mixed,
    };

    TextFacts {
        // One more than the breaks, which is what a text editor shows: a file ending in a
        // newline has a final empty line, and `ui/src/editor/lineEndings.ts::countLines` returns
        // the same number for the same bytes. Dropping it here would make the card disagree with
        // the editor's own gutter by one on nearly every file in the repository.
        lines: lf + crlf + cr + 1,
        ending,
        utf8: std::str::from_utf8(bytes).is_ok(),
    }
}

/// Walk a directory, counting entries and bytes, and stop at [`DIR_WALK_LIMIT`].
///
/// Never follows a symlink: a link into a parent would make this walk forever, and a link's own
/// size is what the entry costs. `truncated` is the whole honesty of the answer — see
/// [`DirSummary`].
///
/// An unreadable subdirectory is **skipped, not fatal**. A `target/` with one root-owned
/// directory in it should still report the rest; the alternative is a card that refuses to
/// describe a directory because one entry inside it was private.
pub fn dir_summary(path: &Path) -> Result<DirSummary> {
    let mut out = DirSummary::default();
    let mut seen: u32 = 0;
    walk_dir(path, &mut out, &mut seen)?;
    Ok(out)
}

fn walk_dir(path: &Path, out: &mut DirSummary, seen: &mut u32) -> Result<()> {
    // Only the top call's failure is an error; recursion swallows its own (see the caller
    // below), so this `?` reports "you cannot read this directory" for the directory the user
    // actually right-clicked and for no other.
    let entries = fs::read_dir(path)?;

    for entry in entries.flatten() {
        if *seen >= DIR_WALK_LIMIT {
            out.truncated = true;
            return Ok(());
        }
        *seen += 1;

        // `symlink_metadata` again, and here it is what bounds the walk: following a link is how
        // a directory containing `link -> ..` becomes an infinite descent.
        let Ok(meta) = entry
            .metadata()
            .or_else(|_| entry.path().symlink_metadata())
        else {
            continue;
        };
        let file_type = meta.file_type();

        if file_type.is_dir() {
            out.dirs += 1;
            // Deliberately ignored: an unreadable child contributes nothing and stops nothing.
            let _ = walk_dir(&entry.path(), out, seen);
            if out.truncated {
                return Ok(());
            }
        } else {
            out.files += 1;
            out.bytes += meta.len();
        }
    }
    Ok(())
}

/// `-rw-r--r--`, `drwxr-xr-x`, `lrwxrwxrwx`.
///
/// Rendered in Rust so there is **one producer**. The frontend could build it from `mode` and
/// then there would be two implementations of a ten-character string with setuid, setgid and
/// sticky in it — two answers that agree until the day they do not, on a row people read to
/// decide whether a script will run.
pub fn mode_string(mode: u32, kind: PathKind) -> String {
    let mut s = String::with_capacity(10);
    s.push(match kind {
        PathKind::Dir => 'd',
        PathKind::Symlink => 'l',
        PathKind::File => '-',
        // `ls` distinguishes p/s/c/b here. This does not: the kind enum does not carry which,
        // and a wrong letter is worse than a neutral one.
        PathKind::Other => '?',
    });

    // Owner, group, other. The special bits replace the *execute* character rather than adding
    // an eleventh, which is what `ls` does: `rws` is setuid-and-executable, `rwS` is setuid
    // without it, and the difference is exactly the thing somebody opens this row to see.
    let triples = [
        (mode >> 6, 0o4000, 's', 'S'),
        (mode >> 3, 0o2000, 's', 'S'),
        (mode, 0o1000, 't', 'T'),
    ];
    for (bits, special, set_exec, set_noexec) in triples {
        s.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        s.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        let exec = bits & 0o1 != 0;
        s.push(match (mode & special != 0, exec) {
            (true, true) => set_exec,
            (true, false) => set_noexec,
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    // Masked to the permission and special bits. The raw `st_mode` also carries the file type,
    // which `PathKind` already reports and which would make `mode` render as a six-digit octal
    // nobody recognises.
    Some(meta.mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_of(_meta: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn changed_unix_ms(meta: &fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    // ctime is seconds plus nanoseconds and is *not* a `SystemTime` on this API, so it is
    // assembled rather than converted. A different fact from mtime: a `chmod` moves this and not
    // that, which is the question somebody reading the permissions row usually has.
    meta.ctime()
        .checked_mul(1000)
        .map(|ms| ms + i64::from(meta.ctime_nsec() as i32) / 1_000_000)
}

#[cfg(not(unix))]
fn changed_unix_ms(_meta: &fs::Metadata) -> Option<i64> {
    None
}

#[cfg(unix)]
fn owner_of(meta: &fs::Metadata) -> Option<Owner> {
    use std::os::unix::fs::MetadataExt;
    let (uid, gid) = (meta.uid(), meta.gid());
    Some(Owner {
        uid,
        gid,
        user: user_name(uid),
        group: group_name(gid),
    })
}

#[cfg(not(unix))]
fn owner_of(_meta: &fs::Metadata) -> Option<Owner> {
    None
}

/// The login name for a uid, or `None`.
///
/// `None` is an ordinary answer, not a failure: a uid with no passwd entry is the normal state
/// inside a container and on any NFS mount whose directory service is unreachable. The card
/// renders the number either way, so a missing name costs a word.
///
/// `getpwuid_r` and not `getpwuid`: the latter returns a pointer into a static buffer that the
/// next call from any thread overwrites, and this runs on a blocking pool with several threads.
#[cfg(unix)]
fn user_name(uid: u32) -> Option<String> {
    // SAFETY: `getpwuid_r` writes into the caller's buffers and reports via the out-pointer;
    // `result` is left null when there is no entry, which is the case this returns `None` for.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0_i8; 2048];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            uid as libc::uid_t,
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if rc != 0 || result.is_null() || pwd.pw_name.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(pwd.pw_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// The group name for a gid. [`user_name`]'s rules exactly.
#[cfg(unix)]
fn group_name(gid: u32) -> Option<String> {
    // SAFETY: as `user_name`.
    unsafe {
        let mut grp: libc::group = std::mem::zeroed();
        let mut buf = vec![0_i8; 2048];
        let mut result: *mut libc::group = std::ptr::null_mut();
        let rc = libc::getgrgid_r(
            gid as libc::gid_t,
            &mut grp,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if rc != 0 || result.is_null() || grp.gr_name.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(grp.gr_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Milliseconds since the epoch, for a time the filesystem may not have.
///
/// Milliseconds and not nanoseconds, because this value is **formatted** by the webview rather
/// than handed back as a token — [`cide_ipc::properties`]'s header has the whole argument, and
/// [`cide_ipc::FileStamp`]'s has the outage that made it worth writing down.
///
/// Pre-epoch times are real (an unpacked archive can carry one) and are handled by going through
/// `duration_since` in both directions rather than saturating to 1970, which would render as a
/// confident and wrong date.
fn system_time_ms(time: Option<std::time::SystemTime>) -> Option<i64> {
    use std::time::UNIX_EPOCH;
    let time = time?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).ok(),
        Err(e) => i64::try_from(e.duration().as_millis()).ok().map(|ms| -ms),
    }
}

/// A path the properties command will not answer about, or `Ok(())`.
///
/// There is deliberately no root containment check here — unlike `cmd::file`'s read path, this
/// answers only *about* a path and never returns its contents, so the file-picker's
/// "open something outside the project" gesture has nothing to leak. What it does refuse is an
/// empty path, which `symlink_metadata` reports as a confusing "No such file or directory (os
/// error 2)" about nothing at all.
pub fn check_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(CoreError::Io("No path given".to_string()));
    }
    Ok(())
}

/// The absolute form of `path`, for a card opened from a relative one. Best-effort.
pub fn absolutise(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory per test, `document.rs`'s helper with a counter added.
    ///
    /// No `tempfile` dependency in this crate, and adding one for six tests is not worth a new
    /// entry in the root manifest.
    fn tempdir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NONCE: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cide-props-{}-{tag}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn counts_lines_the_way_an_editor_does() {
        // One more than the breaks, so a trailing newline leaves an empty final line. This is
        // `countLines` in `ui/src/editor/lineEndings.ts`, and the two must not drift: the status
        // bar and this card describe the same file on the same screen.
        assert_eq!(text_facts(b"").lines, 1);
        assert_eq!(text_facts(b"a").lines, 1);
        assert_eq!(text_facts(b"a\n").lines, 2);
        assert_eq!(text_facts(b"a\nb").lines, 2);
        assert_eq!(text_facts(b"a\nb\n").lines, 3);
    }

    #[test]
    fn a_crlf_is_one_break_and_not_two() {
        // The bug this pins is a two-pass counter that counts the `\r` and the `\n` separately
        // and reports twice the lines for every file checked out on Windows.
        let f = text_facts(b"a\r\nb\r\nc");
        assert_eq!(f.lines, 3);
        assert_eq!(f.ending, LineEnding::Crlf);
    }

    #[test]
    fn a_lone_cr_is_its_own_ending() {
        let f = text_facts(b"a\rb\rc");
        assert_eq!(f.lines, 3);
        assert_eq!(f.ending, LineEnding::Cr);
    }

    #[test]
    fn more_than_one_shape_is_mixed_and_never_the_commonest() {
        // Reporting the majority would make a file that a tool half-converted look clean, which
        // is the one case somebody opens this row to detect.
        assert_eq!(text_facts(b"a\nb\r\nc").ending, LineEnding::Mixed);
        assert_eq!(text_facts(b"a\rb\nc").ending, LineEnding::Mixed);
    }

    #[test]
    fn no_break_is_none_and_not_lf() {
        // `None` and `Lf` are different claims: one file has no ending, the other has one.
        assert_eq!(text_facts(b"").ending, LineEnding::None);
        assert_eq!(text_facts(b"no break here").ending, LineEnding::None);
    }

    #[test]
    fn invalid_utf8_is_reported_rather_than_refused() {
        // `document::read` refuses such a file outright, so this row is the only place in cide
        // that can explain why it will not open.
        assert!(!text_facts(&[0xff, 0xfe, b'a']).utf8);
        assert!(text_facts(b"plain").utf8);
    }

    #[test]
    fn mode_strings_match_ls() {
        assert_eq!(mode_string(0o644, PathKind::File), "-rw-r--r--");
        assert_eq!(mode_string(0o755, PathKind::Dir), "drwxr-xr-x");
        assert_eq!(mode_string(0o777, PathKind::Symlink), "lrwxrwxrwx");
        assert_eq!(mode_string(0o600, PathKind::File), "-rw-------");
        assert_eq!(mode_string(0o000, PathKind::File), "----------");
    }

    #[test]
    fn the_special_bits_replace_the_execute_character() {
        // `ls`'s rule, and the reason it is worth copying: `rws` and `rwS` differ by whether the
        // thing actually runs, and a renderer that appended a flag instead would lose that.
        assert_eq!(mode_string(0o4755, PathKind::File), "-rwsr-xr-x");
        assert_eq!(mode_string(0o4644, PathKind::File), "-rwSr--r--");
        assert_eq!(mode_string(0o2755, PathKind::File), "-rwxr-sr-x");
        assert_eq!(mode_string(0o2745, PathKind::File), "-rwxr-Sr-x");
        assert_eq!(mode_string(0o1777, PathKind::Dir), "drwxrwxrwt");
        assert_eq!(mode_string(0o1776, PathKind::Dir), "drwxrwxrwT");
    }

    #[test]
    fn a_pre_epoch_time_is_negative_and_not_zero() {
        use std::time::{Duration, UNIX_EPOCH};
        let before = UNIX_EPOCH - Duration::from_secs(60);
        assert_eq!(system_time_ms(Some(before)), Some(-60_000));
        assert_eq!(system_time_ms(Some(UNIX_EPOCH)), Some(0));
        assert_eq!(system_time_ms(None), None);
    }

    #[test]
    fn a_symlink_reports_itself_and_not_its_target() {
        // The whole reason this module does not reuse `document::stamp_at`. A card built on a
        // following stat shows the target's size, mode and mtime under the link's name, and
        // there is no symptom at all.
        let dir = tempdir("link");
        let target = dir.join("target.txt");
        fs::write(&target, b"0123456789").expect("write");
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let props = read(&link).expect("read");
        assert_eq!(props.kind, PathKind::Symlink);
        assert_ne!(props.len, 10, "reported the target's length");
        assert_eq!(props.symlink_target.as_deref(), Some(target.as_path()));
        assert_eq!(props.symlink_broken, Some(false));
        // And no text facts, because they would be the target's too.
        assert!(props.text.is_none());
        assert_eq!(props.name, "link.txt");
    }

    #[test]
    fn a_dangling_symlink_still_describes_itself() {
        let dir = tempdir("dangling");
        let link = dir.join("dangling");
        std::os::unix::fs::symlink(dir.join("gone"), &link).expect("symlink");

        let props = read(&link).expect("read");
        assert_eq!(props.kind, PathKind::Symlink);
        assert_eq!(props.symlink_broken, Some(true));
        assert!(props.symlink_target.is_some());
    }

    #[test]
    fn a_binary_says_so_rather_than_counting_lines() {
        let dir = tempdir("binary");
        let file = dir.join("a.bin");
        fs::write(&file, [0x00, 0x01, b'\n', b'\n']).expect("write");

        let props = read(&file).expect("read");
        assert!(props.text.is_none());
        assert!(
            props
                .text_skipped
                .as_deref()
                .is_some_and(|s| s.contains("Binary")),
            "{:?}",
            props.text_skipped
        );
    }

    #[test]
    fn exactly_one_of_text_and_its_excuse_is_present() {
        // The property the card's rendering depends on: every absent fact has a sentence.
        let dir = tempdir("facts");
        let file = dir.join("a.txt");
        fs::write(&file, b"one\ntwo\n").expect("write");

        let props = read(&file).expect("read");
        assert!(props.text.is_some() != props.text_skipped.is_some());
        assert_eq!(props.text.expect("facts").lines, 3);
        assert_eq!(props.kind, PathKind::File);
        assert_eq!(props.len, 8);
    }

    #[test]
    fn a_directory_has_no_text_facts_and_no_apology_for_them() {
        // A directory has no lines, and a sentence saying so would be noise on every folder.
        let dir = tempdir("dir");
        let props = read(&dir).expect("read");
        assert_eq!(props.kind, PathKind::Dir);
        assert!(props.text.is_none());
        assert!(props.text_skipped.is_none());
    }

    #[test]
    fn a_directory_walk_counts_files_dirs_and_bytes() {
        let dir = tempdir("walk");
        fs::write(dir.join("a"), b"12345").expect("write");
        fs::create_dir(dir.join("sub")).expect("mkdir");
        fs::write(dir.join("sub/b"), b"123").expect("write");

        let s = dir_summary(&dir).expect("walk");
        assert_eq!((s.files, s.dirs, s.bytes), (2, 1, 8));
        assert!(!s.truncated);
    }

    #[test]
    fn a_directory_walk_does_not_follow_a_link_into_its_own_parent() {
        // Without the non-following stat in the walk this test does not fail — it never returns.
        let dir = tempdir("loop");
        fs::create_dir(dir.join("sub")).expect("mkdir");
        std::os::unix::fs::symlink("..", dir.join("sub/up")).expect("symlink");

        let s = dir_summary(&dir).expect("walk");
        assert!(!s.truncated);
        assert_eq!(s.dirs, 1);
    }

    #[test]
    fn an_empty_path_is_refused_with_a_sentence() {
        assert!(check_path(Path::new("")).is_err());
        assert!(check_path(Path::new("/tmp")).is_ok());
    }
}
