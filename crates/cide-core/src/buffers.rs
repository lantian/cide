//! Unsaved editor buffers, kept on disk so a crash or a power cut does not take them. (M117)
//!
//! # Why this exists
//!
//! An editor buffer lives in exactly one place: the `EditorView` inside the webview. Autosave
//! writes it to the file after a minute of idleness or five minutes of continuous typing, and
//! until then nothing outside that view holds a copy — so a power cut, an OOM kill or a crash
//! took up to five minutes of work, and an autosave the policy *refused* (a conflict bar up, an
//! agent's diff open on the file) took everything since the last save. Every other piece of
//! state this app keeps across a restart was already on disk within a second.
//!
//! The editor snapshots a dirty buffer here a moment after the typing pauses (the policy is
//! `ui/src/editor/hotExit.ts`), clears it when the buffer is saved, reverted or discarded, and
//! on the next open of the same file offers it back as a dirty buffer over the disk's text.
//!
//! # Shape
//!
//! One file per buffer, `<state>/buffers/<blake3(path)>.json`, holding the path, the text and
//! the file's stamp as the buffer last agreed with it. One file rather than a JSON sidecar
//! beside a `.txt`, because a snapshot is replaced whole and one atomic write is the only way a
//! crash cannot pair the new text with the old stamp. Keyed by the path because a file has one
//! tab (`tab_open_file` reuses it), and hashed because a path is not a file name.
//!
//! Written through [`persist::write_atomic`]: 0600, since a buffer is whatever the user typed —
//! a `.env`, a key — and fsynced, since surviving a power cut is the point.
//!
//! # What is not here
//!
//! Merge-pane decisions and Excalidraw scenes — each has unsaved state too, of another shape.
//! A backup for them is a follow-up, not a variant of this one.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cide_ipc::{BufferBackup, FileStamp};

use crate::{Result, persist};

/// How old a backup may grow before the sweep takes it whatever else is true. A buffer
/// nobody has reopened for a month is not coming back.
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// `<state>/buffers`, under the profile's state directory like everything else.
pub fn dir() -> PathBuf {
    persist::state_dir().join("buffers")
}

fn file_for(dir: &Path, path: &str) -> PathBuf {
    let hash = blake3::hash(path.as_bytes()).to_hex();
    dir.join(format!("{}.json", &hash[..32]))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Keep one buffer's unsaved text.
pub fn write(dir: &Path, path: &str, text: String, base_stamp: Option<FileStamp>) -> Result<()> {
    let backup = BufferBackup {
        path: path.to_string(),
        text,
        base_stamp,
        saved_unix_ms: now_unix_ms(),
    };
    let bytes = serde_json::to_vec(&backup)?;
    persist::write_atomic(&file_for(dir, path), &bytes)
}

/// The kept text for `path`, if there is one and it is readable and really is `path`'s.
///
/// An unreadable file answers `None` and is left where it is rather than quarantined: the next
/// snapshot of that buffer replaces it, and the sweep ages it out otherwise. A hash collision
/// is checked for, because answering one file's text for another would be the one way this
/// feature could *destroy* work rather than keep it.
pub fn read(dir: &Path, path: &str) -> Option<BufferBackup> {
    let bytes = std::fs::read(file_for(dir, path)).ok()?;
    let backup: BufferBackup = serde_json::from_slice(&bytes)
        .inspect_err(|error| tracing::warn!(path, %error, "an unreadable buffer backup"))
        .ok()?;
    (backup.path == path).then_some(backup)
}

/// Forget `path`'s kept text — it was saved, reverted or discarded. Absent is success.
pub fn clear(dir: &Path, path: &str) -> Result<()> {
    match std::fs::remove_file(file_for(dir, path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Remove every backup no open tab could ask for, and every one older than `max_age`. Answers
/// how many went.
///
/// Called once at launch with `wanted` answering "does some File tab — open or in a closed
/// project's remembered layout — name this path". A backup for a path no tab names can never
/// be offered: the only road back to it is opening the file, and a file opened afresh after
/// its tab was closed with Discard must not resurrect what the user threw away.
pub fn sweep(dir: &Path, wanted: impl Fn(&str) -> bool, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = now_unix_ms();
    let max_age_ms = max_age.as_millis() as u64;
    let mut removed = 0;
    for entry in entries.flatten() {
        let file = entry.path();
        if file.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let keep = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BufferBackup>(&bytes).ok())
            .is_some_and(|backup| {
                now.saturating_sub(backup.saved_unix_ms) <= max_age_ms && wanted(&backup.path)
            });
        if !keep && std::fs::remove_file(&file).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-buffers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_buffer_is_kept_read_back_and_cleared() {
        let dir = scratch("roundtrip");
        let stamp = FileStamp {
            mtime_nanos: 7,
            len: 3,
        };
        write(&dir, "/p/a.rs", "fn main() {}\n".into(), Some(stamp)).expect("write");
        let back = read(&dir, "/p/a.rs").expect("kept");
        assert_eq!(back.text, "fn main() {}\n");
        assert_eq!(back.base_stamp, Some(stamp));
        assert!(read(&dir, "/p/b.rs").is_none(), "another file's text");

        clear(&dir, "/p/a.rs").expect("clear");
        assert!(read(&dir, "/p/a.rs").is_none());
        clear(&dir, "/p/a.rs").expect("clearing nothing is fine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sweep_keeps_only_what_a_tab_could_ask_for() {
        let dir = scratch("sweep");
        write(&dir, "/p/open.rs", "a".into(), None).expect("write");
        write(&dir, "/p/closed.rs", "b".into(), None).expect("write");
        std::fs::write(dir.join("garbage.json"), b"{").expect("plant");

        let removed = sweep(&dir, |path| path == "/p/open.rs", MAX_AGE);
        assert_eq!(removed, 2, "the unwanted one and the unreadable one");
        assert!(read(&dir, "/p/open.rs").is_some());
        assert!(read(&dir, "/p/closed.rs").is_none());

        // And age alone is enough: a backup planted a month and a day old goes, wanted or not.
        let old = BufferBackup {
            path: "/p/old.rs".into(),
            text: "c".into(),
            base_stamp: None,
            saved_unix_ms: now_unix_ms() - MAX_AGE.as_millis() as u64 - 86_400_000,
        };
        std::fs::write(
            file_for(&dir, "/p/old.rs"),
            serde_json::to_vec(&old).expect("encode"),
        )
        .expect("plant");
        assert_eq!(sweep(&dir, |_| true, MAX_AGE), 1);
        assert!(read(&dir, "/p/old.rs").is_none());
        assert!(read(&dir, "/p/open.rs").is_some(), "a young one stays");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
