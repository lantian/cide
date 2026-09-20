//! The bytes behind [`TaskAttachment`]: where they live, how they get there, how they leave.
//! (M39)
//!
//! `.cide/tasks.json` carries the *record* — name, size, kind, who, when — and this module owns
//! the *file*, at `<root>/.cide/attachments/<task>/<attachment>/<name>`. The split is
//! `crate::image`'s rule applied to a committed file: bytes never ride in JSON, so a screenshot
//! attached to a task costs the tracker one short object and the repository one tracked file.
//!
//! # One road in
//!
//! Every gesture that attaches a file — the panel's picker, a pasted screenshot, a file dropped
//! from the desktop, the New task dialog, an agent's `cide_task_attach` — ends in [`import`] or
//! [`import_bytes`]. They differ only in whether the caller holds a path or the bytes. That is
//! what keeps the checks in one place: the size cap, the regular-file test, the sniff that decides
//! [`AttachmentKind`], and the name sanitiser all run here or nowhere.
//!
//! # Two passes, so a refusal costs nothing
//!
//! [`import`] validates *every* source before it copies *any*. A comment written with three
//! screenshots must land with three or not at all — `AttachTarget::NewComment`'s doc — and an
//! agent told "the second path is not a file" should find the tracker exactly as it was. A copy
//! that fails midway removes what it wrote for the same reason.
//!
//! # A directory per attachment
//!
//! The id is a path component so two attachments on one task may share a name (`screenshot.png`
//! twice, from two runs) and so the real name survives on disk for the OS opener, which chooses
//! an application by extension. It is also why [`TaskAttachmentId`] must never be re-minted by
//! `repair`: the bytes are filed under the old one.
//!
//! # Staging
//!
//! A screenshot pasted into the comment composer or the New task dialog has no task or comment
//! to be imported under yet. `task_attachment_stage_clipboard` writes it under
//! [`staging_dir`] and hands the path back; when the comment or task lands, [`import`] recognises
//! a source inside that directory and **consumes** it rather than copying it, so an abandoned
//! draft is the only way a staged file outlives its gesture — and [`sweep_staging`] collects
//! those at launch.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cide_core::{CoreError, Result, document, image, persist};
use cide_ipc::{AttachmentKind, TaskAttachment, TaskAttachmentId, TaskAuthor, TaskId};

/// The most one attachment may weigh.
///
/// Tied to the editor's cap rather than chosen on its own, and asserted rather than aliased so a
/// later change to either is a conscious one: an attachment above `MAX_IMAGE_BYTES` could be
/// attached and never previewed, and one above `MAX_FILE_BYTES` attached and never opened in a
/// tab. One number for "a file cide will hold" is a number a person can remember.
pub const MAX_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
const _: () = assert!(
    MAX_ATTACHMENT_BYTES == document::MAX_FILE_BYTES,
    "the attachment cap must stay the editor's cap — see the doc on MAX_ATTACHMENT_BYTES"
);
const _: () = assert!(MAX_ATTACHMENT_BYTES == image::MAX_IMAGE_BYTES);

/// How much of a file [`import`] looks at to decide its kind. `image::sniff`'s own reach.
const HEADER_BYTES: usize = 8 * 1024;

/// `<root>/.cide/tasks/<task>/attachments` — where a task's files live since M68.
pub fn dir(root: &Path, task: &TaskId) -> PathBuf {
    root.join(cide_ipc::TASKS_DIR_RELATIVE)
        .join(task.as_str())
        .join(cide_ipc::ATTACHMENTS_LEAF)
}

/// `<root>/.cide/attachments/<task>` — the pre-M68 layout. Read, never written.
pub fn legacy_dir(root: &Path, task: &TaskId) -> PathBuf {
    root.join(cide_ipc::ATTACHMENTS_DIR).join(task.as_str())
}

/// Where one attachment's bytes are, **as they are actually on disk**.
///
/// The current path when it exists, the pre-M68 one when only that does, and the current one when
/// neither does — so a caller that is about to *write* gets the right answer and a caller that is
/// about to read gets the file. [`TaskAttachment::legacy_relative_path`] carries the argument for why
/// the fallback exists: a conversion that was interrupted leaves some files moved and some not, and a
/// reader that knew only the new path would answer *no such attachment* for the remainder while the
/// bytes sat safely on disk.
///
/// One extra `stat` on a path that is normally absent, on the read of a thumbnail. That is the whole
/// cost of making a partial move harmless.
pub fn path_of(root: &Path, task: &TaskId, attachment: &TaskAttachment) -> PathBuf {
    let current = root.join(attachment.relative_path(task));
    if current.exists() {
        return current;
    }
    let legacy = root.join(attachment.legacy_relative_path(task));
    if legacy.exists() {
        return legacy;
    }
    current
}

/// Move every task's attachment directory into the task's own directory. Answers how many moved.
///
/// The bytes half of the schema 1 → 2 conversion, and the only part of it that is not JSON. Called by
/// `TaskStore::open` when the ladder ran, once, before the index is flushed.
///
/// # Every failure here is survivable, and none of them aborts the conversion
///
/// A rename that fails is logged and skipped, and the file stays exactly where it was — which
/// [`path_of`] still finds. That is the whole reason the fallback exists: the honest outcome of a
/// half-finished move is a tracker where some attachments are in the new place and some in the old,
/// all of them readable, converging on the next open.
///
/// `rename` and not copy-then-delete: it is atomic per directory within a filesystem, and both paths
/// are inside `.cide/` so there is no cross-device case to handle. If one ever arises the rename fails
/// with `EXDEV`, which lands in the skip-and-log path above rather than half-copying anything.
pub fn relocate(root: &Path, file: &cide_ipc::TaskFile) -> usize {
    let mut moved = 0;
    for row in &file.tasks {
        let from = legacy_dir(root, &row.id);
        if !from.is_dir() {
            continue;
        }
        let to = dir(root, &row.id);
        if to.exists() {
            // Already there. Either a previous conversion got this far, or a hand-arranged tree
            // already used the new layout; either way the old directory is not ours to merge in, so
            // it is left for a person to look at.
            tracing::warn!(
                from = %from.display(),
                to = %to.display(),
                "both attachment layouts exist for this task; left the old one alone"
            );
            continue;
        }
        if let Some(parent) = to.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(path = %parent.display(), %error, "could not make a task's directory; its attachments stay where they are");
            continue;
        }
        match fs::rename(&from, &to) {
            Ok(()) => moved += 1,
            Err(error) => {
                tracing::warn!(
                    from = %from.display(),
                    to = %to.display(),
                    %error,
                    "could not move a task's attachments; they stay where they are and are still read"
                );
            }
        }
    }
    // Non-recursive, so it disappears only once every task's directory has moved out of it — and
    // never takes anything with it if one did not.
    let _ = fs::remove_dir(root.join(cide_ipc::ATTACHMENTS_DIR));
    moved
}

/// Where a file waits between a paste and the comment or task it will belong to.
///
/// Under the profile's state directory and not under the project: a staged file belongs to one
/// user's unfinished gesture, and the project directory is the team's. `persist::state_dir` is
/// profile-aware, so a `dev` instance stages beside its own workspace.
pub fn staging_dir() -> PathBuf {
    persist::state_dir().join("attach-staging")
}

/// Whether a source path is one [`import`] should consume rather than copy.
pub fn is_staged(path: &Path) -> bool {
    path.starts_with(staging_dir())
}

/// [`is_staged`] against a given staging root, so a test can stage under its own directory.
fn is_staged_under(path: &Path, staging: &Path) -> bool {
    path.starts_with(staging)
}

/// Remove staged files older than `older_than`, answering how many entries went.
///
/// Called once at launch. A staged file is meant to live for seconds; one that is a day old
/// belongs to a dialog somebody closed, and nothing else will ever look at it. Best effort and
/// silent: a directory that cannot be read is a directory with nothing to sweep.
pub fn sweep_staging(older_than: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(staging_dir()) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut swept = 0;
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > older_than);
        if stale && std::fs::remove_dir_all(entry.path()).is_ok() {
            swept += 1;
        }
    }
    swept
}

/// One path component out of whatever a caller named a file.
///
/// Sanitised rather than refused, because the name is *taken* from a file that already exists
/// and a person dropped it on the card: a refusal would be cide arguing with their desktop. What
/// goes: separators (either kind — a name from one platform can be dropped on another), every
/// control character (NUL above all, and a newline would break the line `cide_task_get` prints
/// the name on), leading and trailing whitespace, and the two names that mean a directory. What
/// is left empty becomes `attachment`, so the record always has a last path component.
pub fn sanitize_name(original: &str) -> String {
    let cleaned: String = original
        .chars()
        .map(|c| match c {
            '/' | '\\' => '_',
            other if other.is_control() => '_',
            other => other,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "attachment".to_string()
    } else {
        trimmed.to_string()
    }
}

/// A source that passed every check and has been read, before anything is written.
struct Checked {
    name: String,
    bytes: Vec<u8>,
    /// The staged directory to delete once the copy is in place, for a source under
    /// [`staging_dir`]. `None` for an ordinary file, which is left exactly where it was.
    consume: Option<PathBuf>,
}

/// The refusal an unusable source gets, naming the path — the sentence is read by a person in
/// a toast or by the model that will make the next call, and both need to know *which* file.
fn refuse(path: &Path, why: impl std::fmt::Display) -> CoreError {
    CoreError::Io(format!("{}: {why}", path.display()))
}

fn check(source: &Path, staging: &Path) -> Result<Checked> {
    // Canonicalised first: a refusal about a symlink should name the file behind it, and a
    // relative path the caller resolved wrongly should say what it resolved to.
    let real = std::fs::canonicalize(source).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            refuse(source, "no such file")
        } else {
            refuse(source, e)
        }
    })?;
    let meta = std::fs::metadata(&real).map_err(|e| refuse(&real, e))?;
    if !meta.is_file() {
        return Err(refuse(&real, "not a regular file"));
    }
    if meta.len() == 0 {
        return Err(refuse(&real, "is empty"));
    }
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(refuse(
            &real,
            format!(
                "is {} MiB, over the {} MiB limit for an attachment",
                meta.len() / (1024 * 1024),
                MAX_ATTACHMENT_BYTES / (1024 * 1024)
            ),
        ));
    }
    let bytes = std::fs::read(&real).map_err(|e| refuse(&real, e))?;
    let name = sanitize_name(
        &real
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
    );
    // A staged file sits in a directory of its own under the staging root, and it is that
    // directory that goes. Decided on the path as *given*, not the canonical one: the staging
    // root is under a state directory that may itself be a symlink, and the caller named the
    // path cide handed it.
    let consume = is_staged_under(source, staging).then(|| {
        source
            .parent()
            .filter(|parent| *parent != staging)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| source.to_path_buf())
    });
    Ok(Checked {
        name,
        bytes,
        consume,
    })
}

/// Copy every source under the task, answering one record per source in the order given.
///
/// All-or-nothing, per the module header: the first refusal is returned before a byte is copied,
/// and a copy that fails removes every record's bytes written so far. Staged sources are consumed
/// only after every copy succeeded, so a refusal leaves the caller's staged files where they were.
pub fn import(
    root: &Path,
    task: &TaskId,
    sources: &[PathBuf],
    author: &TaskAuthor,
) -> Result<Vec<TaskAttachment>> {
    import_with(root, task, sources, author, &staging_dir())
}

/// [`import`] with the staging root named, which is what lets a test stage without touching the
/// profile's real state directory. `staging` is compared textually against each source as given.
pub fn import_with(
    root: &Path,
    task: &TaskId,
    sources: &[PathBuf],
    author: &TaskAuthor,
    staging: &Path,
) -> Result<Vec<TaskAttachment>> {
    if sources.is_empty() {
        return Err(CoreError::Io("nothing to attach".to_string()));
    }
    let checked: Vec<Checked> = sources
        .iter()
        .map(|source| check(source, staging))
        .collect::<Result<_>>()?;

    let now = persist::now_ms();
    let mut placed: Vec<TaskAttachment> = Vec::with_capacity(checked.len());
    for item in &checked {
        match place(root, task, &item.name, &item.bytes, author, now) {
            Ok(record) => placed.push(record),
            Err(error) => {
                for record in &placed {
                    remove(root, task, record);
                }
                return Err(error);
            }
        }
    }
    for item in &checked {
        if let Some(dir) = &item.consume
            && let Err(error) = std::fs::remove_dir_all(dir)
        {
            tracing::warn!(path = %dir.display(), %error, "a staged attachment could not be removed after import");
        }
    }
    Ok(placed)
}

/// Copy bytes the caller already holds — the clipboard road — under the task.
pub fn import_bytes(
    root: &Path,
    task: &TaskId,
    name: &str,
    bytes: &[u8],
    author: &TaskAuthor,
) -> Result<TaskAttachment> {
    if bytes.is_empty() {
        return Err(CoreError::Io(format!("{name}: is empty")));
    }
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(CoreError::Io(format!(
            "{name}: is {} MiB, over the {} MiB limit for an attachment",
            bytes.len() as u64 / (1024 * 1024),
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        )));
    }
    place(
        root,
        task,
        &sanitize_name(name),
        bytes,
        author,
        persist::now_ms(),
    )
}

/// Mint the record and write its bytes, in that order: the id decides the path.
///
/// `persist::write_atomic_with_mode` rather than `std::fs::copy`, for the mode (`SHARED_MODE`,
/// the same as the tracker's — this file is committed) and for the rename: a crash mid-copy
/// leaves no half-file under a name the record will claim. No collision loop is needed — the
/// id-named directory is fresh by construction.
fn place(
    root: &Path,
    task: &TaskId,
    name: &str,
    bytes: &[u8],
    author: &TaskAuthor,
    now: u64,
) -> Result<TaskAttachment> {
    let head = &bytes[..bytes.len().min(HEADER_BYTES)];
    let kind = if image::sniff(head).is_some() {
        AttachmentKind::Image
    } else {
        AttachmentKind::File
    };
    let record = TaskAttachment {
        id: TaskAttachmentId::new(),
        name: name.to_string(),
        bytes: bytes.len() as u64,
        kind,
        added_by: author.clone(),
        added_unix_ms: now,
        deleted: false,
    };
    let path = path_of(root, task, &record);
    if let Err(error) = persist::write_atomic_with_mode(&path, bytes, persist::SHARED_MODE) {
        remove(root, task, &record);
        return Err(CoreError::Io(format!(
            "{} could not be written: {error}",
            path.display()
        )));
    }
    Ok(record)
}

/// Delete one attachment's bytes and whatever directories that leaves empty.
///
/// Best effort, and never an error: this runs *after* a tombstone has landed in the tracker, and
/// the record is the truth. A directory that would not go is disk hygiene a person can do by
/// hand, not a failure to report as though the detach had not happened — it has.
pub fn remove(root: &Path, task: &TaskId, attachment: &TaskAttachment) {
    let task_dir = dir(root, task);
    let own = task_dir.join(attachment.id.as_str());
    match std::fs::remove_dir_all(&own) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %own.display(), %error, "an attachment's directory could not be removed");
        }
    }
    // `remove_dir` refuses a non-empty directory, which is exactly the check wanted: the task's
    // directory goes only when this was its last attachment, and the attachments root only when
    // this was the project's last.
    let _ = std::fs::remove_dir(&task_dir);
    let _ = std::fs::remove_dir(root.join(cide_ipc::ATTACHMENTS_DIR));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_one_path_component_and_is_never_empty() {
        // Separators of both kinds, because a name can come from any desktop and land on any.
        assert_eq!(sanitize_name("a/b\\c.png"), "a_b_c.png");
        assert_eq!(sanitize_name("  spaced out.txt  "), "spaced out.txt");
        assert_eq!(
            sanitize_name("Pasted image 2026-09-04 at 10.02.11.png"),
            "Pasted image 2026-09-04 at 10.02.11.png"
        );
        for empty in ["", "   ", ".", ".."] {
            assert_eq!(sanitize_name(empty), "attachment", "{empty:?}");
        }
        assert_eq!(sanitize_name("nul\0here"), "nul_here");
        assert_eq!(sanitize_name("two\nlines.txt"), "two_lines.txt");
    }
}
