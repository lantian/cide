//! Durable state: `workspace.json`, and where it lives.
//!
//! Two asymmetries shape this module. Writes are atomic because a file truncated by a crash
//! halfway through a save is the one failure that costs the user a layout with no way back.
//! Reads never fail because a broken file must never stop the app from starting — a layout
//! is recreatable, a launch loop is not.
//!
//! Nothing here owns a thread or a timer. [`Debouncer`] is polled by its caller, which keeps
//! this module free of an async runtime and so linkable from `cide-headless`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cide_ipc::{RecentProject, ViewPosition, Workspace};
use parking_lot::Mutex;
use serde_json::Value;

use crate::{CoreError, Result};

/// How long a burst of mutations is allowed to accumulate before it is written.
///
/// Long enough that dragging a splitter does not write once per frame, short enough that a
/// kill -9 shortly after a deliberate change still finds the change on disk.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Where mutable, recreatable state goes: `$XDG_STATE_HOME/cide`, else `~/.local/state/cide`.
///
/// State rather than config because `workspace.json` is written by the app, not edited by
/// the user, and putting it in the config directory invites it into dotfile repositories.
pub fn state_dir() -> PathBuf {
    xdg_dir(std::env::var_os("XDG_STATE_HOME"), ".local/state")
}

/// Where user-authored config goes: `$XDG_CONFIG_HOME/cide`, else `~/.config/cide`.
pub fn config_dir() -> PathBuf {
    xdg_dir(std::env::var_os("XDG_CONFIG_HOME"), ".config")
}

/// The persisted workspace tree.
pub fn workspace_path() -> PathBuf {
    state_dir().join("workspace.json")
}

/// The user's keybinding overrides. Defaults are compiled in; this file holds only diffs.
pub fn keymap_path() -> PathBuf {
    config_dir().join("keymap.json")
}

/// Projects the user has opened, most recent first.
///
/// A **separate file** from `workspace.json`, and it has to be: `workspace.json` records what
/// is open, and `close_project` removes a project from it. The entry a recent-projects list
/// most needs is precisely the one that has just been taken out of there, so storing the list
/// inside the workspace would forget a project at the instant it became recent. Keeping it in
/// `Settings` loses for the same reason plus one more — settings are a user's preferences, and
/// a list the app appends to on every open is not a preference.
///
/// Beside `workspace.json` in the state directory, not in config, on the same argument
/// [`state_dir`] already makes: it is written by the app and nobody would want it in a
/// dotfiles repository.
pub fn recent_path() -> PathBuf {
    state_dir().join("recent.json")
}

/// Resolve one XDG base directory, appending `cide`.
///
/// A relative value is ignored, as the spec requires: it would resolve against the working
/// directory and scatter a `.local/state/cide` into whatever project the user launched from.
fn xdg_dir(configured: Option<OsString>, fallback: &str) -> PathBuf {
    if let Some(configured) = configured {
        let base = PathBuf::from(configured);
        if base.is_absolute() {
            return base.join("cide");
        }
    }
    home().join(fallback).join("cide")
}

/// The user's home directory.
///
/// With `HOME` unset there is no correct answer, so state goes somewhere throwaway rather
/// than into the current directory — losing a layout beats writing dot-directories into an
/// arbitrary tree the user did not offer.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Read the workspace at `path`, falling back to [`Workspace::default`] for anything that
/// cannot be understood.
///
/// This function does not fail and does not panic. A corrupt, truncated, empty,
/// wrong-schema or unreadable file is moved aside as `workspace.corrupt-<n>.json` first, so
/// the user can recover it by hand and the next launch is not stuck rejecting the same
/// bytes forever.
pub fn load(path: &Path) -> Workspace {
    match read_workspace(path) {
        Ok(Some(mut workspace)) => {
            // The stored `display_path` was abbreviated against whatever `$HOME` wrote the
            // file, which is not necessarily the one reading it.
            crate::workspace::refresh_display_paths(&mut workspace);
            // `dirty` describes a buffer, and buffers do not survive the process — only the
            // path is stored. A restored tab is showing the file as it is on disk, so the
            // flag has to start false or `close_tab` refuses it over edits that no longer
            // exist. See `workspace::clear_dirty_flags`.
            crate::workspace::clear_dirty_flags(&mut workspace);
            // `tab_mru` arrived after `Project` did and is `#[serde(default)]`, so every file
            // written before it reads back with an empty order — which `workspace::validate`
            // refuses, and `WorkspaceState::load` answers a refusal by discarding the whole
            // workspace. Repairing here is what keeps an upgrade from costing the user their
            // layout; see `workspace::repair_tab_mru` for what it will and will not invent.
            crate::workspace::repair_tab_mru(&mut workspace);
            workspace
        }
        // First launch, or the user deleted the file. Nothing to warn about, and nothing to
        // move aside.
        Ok(None) => Workspace::default(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "workspace file unusable; starting from defaults"
            );
            // Quarantine only what is actually unintelligible. A file we could not *read* —
            // a lock held elsewhere, EACCES under the wrong user, a full-disk EIO — is
            // probably fine and will be readable next launch; renaming it would turn a
            // transient failure into permanent data loss and present it to the user as a
            // reset they did not ask for.
            if matches!(error, CoreError::Serde(_)) {
                quarantine(path);
            }
            Workspace::default()
        }
    }
}

/// `Ok(None)` means there is no file yet, which is not an error.
fn read_workspace(path: &Path) -> Result<Option<Workspace>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let value: Value = serde_json::from_str(&raw)?;

    // `serde_json::Value` holds object keys in a sorted map, and `projects` insertion order
    // *is* the header tab order: reading through `Value` would re-sort the user's project
    // tabs by uuid on every launch. A document already at the current schema is therefore
    // deserialised straight from the text; only one that must be migrated goes through
    // `Value` at all, and migrating it is already a rewrite.
    if is_current_schema(&value) {
        return Ok(Some(serde_json::from_str(&raw)?));
    }
    migrate(value).map(Some)
}

/// Whether a raw document can be read as-is, without passing through [`migrate`].
fn is_current_schema(value: &Value) -> bool {
    value.get("schemaVersion").and_then(Value::as_u64) == Some(Workspace::CURRENT_SCHEMA as u64)
}

/// Bring a raw workspace document up to [`Workspace::CURRENT_SCHEMA`].
///
/// This takes JSON rather than a `Workspace` precisely so it can read `schemaVersion`
/// before committing to a shape the file may no longer have.
///
/// Two documents are refused rather than guessed at. One with no `schemaVersion` is pre-1
/// and predates any format worth honouring. One from a *newer* schema may carry fields this
/// build would silently drop on the next save, and a downgrade that quietly discards half
/// the user's layout is worse than a visible reset.
///
/// Key order **is** preserved through here, and it has to be: `projects` insertion order is
/// the header tab order, and a plain `serde_json::Value` sorts object keys into a `BTreeMap`.
/// That is why the workspace's `serde_json` is built with the `preserve_order` feature —
/// turned on for exactly this, the first migration this ladder has ever run. A user upgrading
/// into schema 2 keeps their tab strip in the order they left it.
pub fn migrate(value: Value) -> Result<Workspace> {
    const CURRENT: u64 = Workspace::CURRENT_SCHEMA as u64;

    let Some(version) = value.get("schemaVersion").and_then(Value::as_u64) else {
        return Err(CoreError::Serde(
            "workspace has no schemaVersion; pre-1 files are not supported".into(),
        ));
    };

    // A ladder: each supported older schema gets an arm that rewrites the document one step
    // forward and re-enters here. Adding version 3 is an arm, not a restructuring.
    match version {
        CURRENT => Ok(serde_json::from_value(value)?),
        1 => migrate(v1_to_v2(value)),
        2 => migrate(v2_to_v3(value)),
        v if v > CURRENT => Err(CoreError::Serde(format!(
            "workspace schema {v} is newer than this build's {CURRENT}; refusing to downgrade it"
        ))),
        v => Err(CoreError::Serde(format!(
            "workspace schema {v} is no longer supported"
        ))),
    }
}

/// Schema 1 → 2: write down which children the proxy settings reached.
///
/// # Why this writes a value that is also the default
///
/// Nothing here is needed to make a schema-1 document *load*. `ProxySettings` has
/// `#[serde(default)]`, and `ProxyScope::default()` is by construction exactly what schema 1
/// meant: both pane kinds proxied, cide's own `git` left with whatever environment cide itself
/// has. Deleting this function would change no behaviour today.
///
/// It is here for the day the default moves. The scope exists so that a user can say "proxy
/// `claude` and nothing else", and the obvious next request is for a fresh install to start
/// that way. The moment `ProxyScope::default()` changes, a document that *relies* on the
/// default is silently re-scoped — a corporate laptop's `git push` moves onto cide's proxy, or
/// off it, because of a constant edited for an unrelated reason, with nothing on that user's
/// disk to explain the change and no error anywhere when it stops working. Writing the value
/// makes an existing user's scope a fact rather than an inference, so that a later change to
/// the default is a change to *new installs* and to nothing else.
///
/// # What it refuses to touch
///
/// A `settings` or `settings.proxy` that is not an object is left exactly as found and allowed
/// to fail in `from_value` below, with serde's own message. Overwriting it would turn "your
/// settings block is corrupt" into "your proxy configuration silently became the default",
/// which is the wrong of the two answers to give someone who hand-edited the file.
fn v1_to_v2(mut value: Value) -> Value {
    if let Some(root) = value.as_object_mut() {
        root.insert("schemaVersion".into(), Value::from(2u32));

        // `entry` rather than a get-or-insert dance, and it creates the intermediate objects:
        // both keys are legitimately absent from a document written by a user who never opened
        // Settings, since every field of `Settings` defaults.
        let settings = root
            .entry("settings")
            .or_insert_with(|| Value::Object(Default::default()));
        if let Some(settings) = settings.as_object_mut() {
            let proxy = settings
                .entry("proxy")
                .or_insert_with(|| Value::Object(Default::default()));
            if let Some(proxy) = proxy.as_object_mut() {
                // Serialized from the type rather than written as a JSON literal here: a
                // literal is a second spelling of `ProxyScope`'s wire names, and the first
                // rename would leave this migration writing a key nothing reads — which
                // deserialises back to the default and looks, from every angle, like it
                // worked.
                if let Ok(scope) = serde_json::to_value(cide_ipc::ProxyScope::default()) {
                    proxy.insert("scope".into(), scope);
                }
            }
        }
    }
    value
}

/// Schema 2 → 3: write down that autosave is on.
///
/// # Why this is not left to `#[serde(default)]`
///
/// `EditorSettings::autosave` defaults to `true`, so a schema-2 document loads perfectly well
/// without this and behaves exactly as a fresh install does. Deleting this function would change
/// no behaviour today — which is the same sentence [`v1_to_v2`] opens with, and the argument is
/// the same one, sharpened by what the field controls.
///
/// Autosave **writes the user's files on a timer**. A defaulted field means every existing
/// workspace's answer to "may cide do that" is an inference from a constant in this build. The
/// obvious next request for a feature like this is "make it opt-in", and on the day somebody
/// grants it, a defaulted field turns autosave off for every user who had come to rely on it —
/// with nothing on their disk to explain why the file they alt-tabbed away from is suddenly
/// dirty again, and no error anywhere. Writing the value makes an upgraded user's setting a fact
/// rather than a consequence of a constant, so a later change to the default is a change to new
/// installs and to nothing else.
///
/// # What it refuses to touch
///
/// A `settings` or `settings.editor` that is not an object is left exactly as found and allowed
/// to fail in `from_value` with serde's own message — the same refusal [`v1_to_v2`] makes, and
/// for the same reason: "your settings block is corrupt" and "your editor configuration silently
/// became the default" are different answers, and only one of them is honest.
///
/// An `autosave` key that is somehow already present is **left alone**. It cannot occur in a
/// document this build wrote (schema 2 predates the field), but a hand-edited file or a
/// downgrade-then-upgrade could carry one, and overwriting a value the user typed is exactly
/// what this whole ladder exists to avoid.
fn v2_to_v3(mut value: Value) -> Value {
    if let Some(root) = value.as_object_mut() {
        root.insert("schemaVersion".into(), Value::from(3u32));

        let settings = root
            .entry("settings")
            .or_insert_with(|| Value::Object(Default::default()));
        if let Some(settings) = settings.as_object_mut() {
            let editor = settings
                .entry("editor")
                .or_insert_with(|| Value::Object(Default::default()));
            if let Some(editor) = editor.as_object_mut() {
                // Serialized from the type rather than written as `Value::Bool(true)`, for the
                // reason `v1_to_v2` gives about its own literal: the default is the type's to
                // state, and a literal here is a second copy of it that the first change to
                // `EditorSettings::default()` silently invalidates.
                let default_on = cide_ipc::EditorSettings::default().autosave;
                editor
                    .entry("autosave")
                    .or_insert_with(|| Value::from(default_on));
            }
        }
    }
    value
}

/// Write `ws` to `path` so that a crash leaves either the old file or the new one, never a
/// half-written one.
///
/// Creates parent directories as needed.
pub fn save_atomic(path: &Path, ws: &Workspace) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(ws)?)
}

/// The publish-by-rename half of [`save_atomic`], over bytes the caller has already encoded.
///
/// Split out when `recent.json` arrived rather than copied, because everything below the
/// encoding is the part that is easy to get subtly wrong — the sibling temp, the `sync_all`
/// before the rename, the directory `fsync` after it, the 0600 mode, and removing the temp on
/// each failure path. A second copy would have started identical and drifted on whichever of
/// those a later edit forgot.
///
/// `pub` because that drift happened: `cide_app::lifecycle` wrote its `screens.json` sidecar
/// with a plain `fs::write`, reasoning only about atomicity — which it was entitled to trade
/// away — and silently gave up the 0600 with it, publishing a screenful of the user's shell
/// output at 0644. The mode is the part of this function nobody should be re-deciding, so the
/// function itself is the thing to reach for rather than the parts of it worth copying.
pub fn write_atomic(path: &Path, json: &[u8]) -> Result<()> {
    let dir = parent_dir(path);
    fs::create_dir_all(dir)?;

    // The temp file is a sibling of the target: `rename` is only atomic within a single
    // filesystem, and the system temp directory is routinely a different one.
    let tmp = temp_path(path);

    let write = (|| -> io::Result<()> {
        let mut file = create_private(&tmp)?;
        file.write_all(json)?;
        // The rename publishes the new name; without this the bytes behind it may not have
        // reached the disk, and the crash leaves an intact name over empty contents.
        file.sync_all()
    })();
    if let Err(e) = write {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }

    if let Err(e) = fs::rename(&tmp, path) {
        // Leaving the temp behind would accumulate one file per failed save.
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }

    // The rename is a directory modification of its own, and an unsynced one can be lost in
    // exactly the crash this function exists to survive.
    sync_dir(dir)
}

// --- recent projects ----------------------------------------------------------------------

/// How many projects `recent.json` remembers.
///
/// A cap rather than an unbounded log, because this list is drawn as a menu: past roughly a
/// screenful the entries stop being "the projects I was working on" and become a history file
/// nobody reads, and the oldest entry in a long list is overwhelmingly a directory that no
/// longer exists. Sixteen fits a menu on a short window without scrolling.
pub const MAX_RECENT: usize = 16;

/// Read `recent.json`, most recent first.
///
/// Does not fail, and for a stronger reason than [`load`] does not: a broken recents file is
/// worth *nothing*. It is a convenience list rebuilt by the next few opens, so a missing,
/// truncated or unparseable one answers with an empty list and is not even quarantined —
/// moving it aside would leave the user with a stray file to clean up in exchange for data
/// they cannot use.
///
/// `display_path` is recomputed here and the list is re-sorted, because neither can be trusted
/// from the file: the `$HOME` that wrote an entry is not necessarily the one reading it (the
/// same reason `workspace::refresh_display_paths` exists), and hand-editing or a half-finished
/// write from an older build could leave the order wrong. Sorting on read means every caller
/// gets the same order without agreeing to one.
pub fn load_recent(path: &Path) -> Vec<RecentProject> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        // Absent is the first launch, and unreadable is nothing this list can do anything
        // about. Both are "no recents", which is a correct answer rather than a degraded one.
        Err(_) => return Vec::new(),
    };
    let mut list: Vec<RecentProject> = match serde_json::from_slice(&bytes) {
        Ok(list) => list,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "recent projects file unusable");
            return Vec::new();
        }
    };

    let home = std::env::var_os("HOME").map(PathBuf::from);
    for entry in &mut list {
        entry.display_path = abbreviate(&entry.path, home.as_deref());
    }
    sort_recent(&mut list);
    list.truncate(MAX_RECENT);
    list
}

/// Write `list` to `path`, atomically and privately, exactly as the workspace is written.
///
/// Private (0600) even though a path list is not a secret: the set of directories a person
/// works in on a shared machine is inference nobody asked to publish, and matching
/// `workspace.json`'s mode means there is one rule here rather than two.
pub fn save_recent(path: &Path, list: &[RecentProject]) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(list)?)
}

/// Move `path` to the front of `list`, as of `opened_at`.
///
/// Pure, and takes the clock as an argument, so the ordering rules are testable without
/// touching the filesystem or waiting a millisecond for two entries to differ.
///
/// Matching is on the path alone, so reopening a folder under a different project *name*
/// updates the existing entry rather than adding a second one — the same identity
/// `workspace::open_project` uses when it activates an already-open path instead of opening it
/// twice. The name is refreshed from the new open, because a renamed directory should show its
/// new name.
pub fn remember_recent(list: &mut Vec<RecentProject>, path: &Path, name: &str, opened_at: u64) {
    list.retain(|entry| entry.path != path);
    list.insert(
        0,
        RecentProject {
            path: path.to_path_buf(),
            name: name.to_owned(),
            display_path: abbreviate(path, std::env::var_os("HOME").map(PathBuf::from).as_deref()),
            opened_at,
        },
    );
    list.truncate(MAX_RECENT);
}

/// Drop `path` from `list`. Answers whether anything was removed.
pub fn forget_recent(list: &mut Vec<RecentProject>, path: &Path) -> bool {
    let before = list.len();
    list.retain(|entry| entry.path != path);
    before != list.len()
}

// --- per-file view positions (M12) --------------------------------------------------------

/// How many files remember where the user was.
///
/// **Generous where [`MAX_RECENT`] is small, and the difference is the reason.** Sixteen is a
/// cap on a list that is *drawn as a menu*: past a screenful the entries stop being useful and
/// start being a history file nobody reads. Nothing ever draws this one. It is looked up by
/// path, one entry at a time, so the only cost of a large cap is bytes — 256 records of roughly
/// a hundred bytes is about 25 KB, which covers a week of files and is smaller than the
/// workspace it sits beside.
///
/// There is deliberately **no TTL**. An expiry means the file you come back to on Monday opens
/// at line 1, which is the complaint this exists to answer.
pub const MAX_POSITIONS: usize = 256;

/// Where the view positions live: `$XDG_STATE_HOME/cide/positions.json`.
///
/// Beside `recent.json` and `workspace.json`, on the same argument [`state_dir`] makes — the
/// app writes it, nobody hand-edits it, and nobody wants it in a dotfiles repository.
pub fn positions_path() -> PathBuf {
    state_dir().join("positions.json")
}

/// Read `positions.json`.
///
/// Does not fail, and for the same stronger reason [`load_recent`] does not: a broken positions
/// file is worth *nothing*. It is re-derived by the user simply looking at a file again, so a
/// missing, truncated or unparseable one answers with an empty list and is not even quarantined
/// — leaving a stray `positions.corrupt-1.json` behind would cost the user a cleanup in
/// exchange for data they cannot use.
///
/// Deleted files are **not** stat'd away on load. That would be one syscall per entry on the
/// launch path to reclaim a hundred bytes each, and a stale entry is already harmless: the
/// restore clamps against the document it actually finds.
pub fn load_positions(path: &Path) -> Vec<ViewPosition> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Vec<ViewPosition>>(&bytes) {
        Ok(mut list) => {
            // Trimmed on read as well as on write. A file written by a build with a larger cap,
            // or hand-edited, must not make every later save carry entries this build would
            // never have kept.
            sort_positions(&mut list);
            list.truncate(MAX_POSITIONS);
            list
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "view positions file unusable");
            Vec::new()
        }
    }
}

/// Write `list` to `path`, atomically and privately, exactly as the workspace is written.
///
/// Private (0600) for the reason [`save_recent`] is: the set of files a person has been reading
/// on a shared machine is inference nobody asked to publish, and one rule here beats two.
pub fn save_positions(path: &Path, list: &[ViewPosition]) -> Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(list)?)
}

/// Record where the user is in one file, evicting the least recently touched when full.
///
/// Pure, and takes the clock as an argument, so the eviction rule is testable without touching
/// the filesystem or waiting a millisecond for two entries to differ — the same shape
/// [`remember_recent`] has.
///
/// Matching is on the path alone: one entry per file, overwritten. That is the whole difference
/// between this and a navigation history, which is an *ordered* list with many entries per file
/// — see `ui/src/editor/navHistory.ts`, which deliberately does not share this store.
pub fn remember_position(list: &mut Vec<ViewPosition>, mut at: ViewPosition, touched_at: u64) {
    at.touched_at = touched_at;
    list.retain(|entry| entry.path != at.path);
    list.push(at);
    if list.len() > MAX_POSITIONS {
        // Sort then truncate rather than "drop the first": the list is not kept in order on
        // disk, and a build that wrote it in another order must not evict an entry the user
        // touched a minute ago.
        sort_positions(list);
        list.truncate(MAX_POSITIONS);
    }
}

/// The remembered place for `path`, or `None`.
pub fn position_for<'a>(list: &'a [ViewPosition], path: &Path) -> Option<&'a ViewPosition> {
    list.iter().find(|entry| entry.path == path)
}

/// Most recently touched first, ties broken by path so the order is total.
///
/// Ties are real: two panes showing two files can be noted within the same millisecond, and an
/// unstable order there would make the eviction pick a different victim on two identical runs.
fn sort_positions(list: &mut [ViewPosition]) {
    list.sort_by(|a, b| {
        b.touched_at
            .cmp(&a.touched_at)
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// Newest first, ties broken by path so the order is total.
///
/// Two entries can genuinely share a timestamp — opening a multi-root project records one
/// entry per call within the same millisecond — and an unstable order there would make the
/// menu reshuffle between two identical reads.
fn sort_recent(list: &mut [RecentProject]) {
    list.sort_by(|a, b| {
        b.opened_at
            .cmp(&a.opened_at)
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// Milliseconds since the Unix epoch, or 0 on a clock set before it.
///
/// Saturating rather than erroring: a wrong timestamp costs this list its order, and refusing
/// to remember a project because the machine's clock is odd is a worse trade.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// `~/work/cide` for a path under `$HOME`, the path itself otherwise.
///
/// A private twin of `workspace::abbreviate`, which is private to that module. Widening it to
/// `pub` was the alternative and lost on ownership: this is eleven lines with one edge case,
/// and the edge case — an unset or empty `$HOME`, where `strip_prefix("")` succeeds for every
/// relative path and would spell `work/cide` as `~/work/cide` — is pinned by a test below as
/// well as by one over there.
fn abbreviate(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home.filter(|home| !home.as_os_str().is_empty()) else {
        return path.display().to_string();
    };
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Create a file only its owner can read.
///
/// `workspace.json` stopped being merely a layout when `Settings::proxy` arrived:
/// `http://user:hunter2@proxy.corp:3128` is an ordinary value for it, stored as typed
/// because a proxy that needs credentials cannot be used otherwise until cide has a keyring.
/// `File::create` asks for 0666, which under the usual umask lands on 0644 — that password
/// readable by every other account on the machine. Redacting the log and the `Debug` impl,
/// which is what `cide_ipc::ProxySettings` does, guards the copies; this guards the original.
///
/// Applied to the temp file rather than to the target afterwards, because the mode travels
/// with the inode through the `rename`. Chmod-after-rename would leave a window in which the
/// finished file exists at 0644, which is exactly the window that matters. It also needs no
/// migration: a workspace already on disk at 0644 is replaced by this inode on its next save.
#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// Whatever the platform's default is. cide is a Linux app; this arm exists so the module
/// still compiles for anyone building it elsewhere, and claims nothing about permissions.
#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<File> {
    File::create(path)
}

/// The directory a file lives in. A bare file name has an empty parent, which means here.
fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Sibling temp file for [`save_atomic`], unique to one call of it.
///
/// The pid separates two cide processes; the counter separates two threads of one, which the
/// pid alone does not. Sharing a temp name is not a lost race but a corrupt file: the second
/// `File::create` truncates the first writer's bytes, and whichever thread renames first
/// publishes one workspace's prefix over another's tail.
fn temp_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let name = file_name_or(path, "workspace.json");
    let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{nonce}.tmp", std::process::id()))
}

/// `fsync` a directory, the Unix way of making a rename within it durable.
fn sync_dir(dir: &Path) -> Result<()> {
    Ok(File::open(dir)?.sync_all()?)
}

/// Move an unreadable file aside so it is recoverable and no longer in the way.
///
/// Best-effort: if the rename fails there is nothing useful left to do but say so, and the
/// caller still starts from defaults.
fn quarantine(path: &Path) {
    let Some(target) = free_quarantine_path(path) else {
        tracing::warn!(
            path = %path.display(),
            "no free quarantine name; leaving the unusable file in place"
        );
        return;
    };
    match fs::rename(path, &target) {
        Ok(()) => tracing::warn!(
            from = %path.display(),
            to = %target.display(),
            "moved the unusable workspace file aside"
        ),
        Err(error) => tracing::warn!(
            path = %path.display(),
            %error,
            "could not move the unusable workspace file aside"
        ),
    }
}

/// The first unused `workspace.corrupt-<n>.json` beside `path`.
///
/// Bounded because a directory that somehow holds every name should end the search rather
/// than spin; by then the user has a larger problem than one more corrupt file.
fn free_quarantine_path(path: &Path) -> Option<PathBuf> {
    (1..=999u32).map(|n| quarantine_path(path, n)).find(|c| {
        // A path we cannot even stat is treated as taken: overwriting it could destroy an
        // earlier quarantined copy.
        matches!(c.try_exists(), Ok(false))
    })
}

fn quarantine_path(path: &Path, n: u32) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".into());
    let name = match path.extension() {
        Some(ext) => format!("{stem}.corrupt-{n}.{}", ext.to_string_lossy()),
        None => format!("{stem}.corrupt-{n}"),
    };
    path.with_file_name(name)
}

fn file_name_or(path: &Path, fallback: &str) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Collapses a burst of mutations into one write.
///
/// It owns no thread and no timer; the caller polls it from whatever loop it already has.
/// A timer here would mean an async runtime in the domain crate, which `cide-headless`
/// links without one.
#[derive(Debug)]
pub struct Debouncer {
    delay: Duration,
    /// When the current burst started, or `None` when nothing is pending.
    ///
    /// Measured from the *first* change of the burst rather than the most recent, so a user
    /// who keeps typing still gets a write every `delay` instead of none until they stop.
    started: Mutex<Option<Instant>>,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            started: Mutex::new(None),
        }
    }

    /// Record that the workspace changed and a write is owed.
    pub fn note_change(&self) {
        self.started.lock().get_or_insert_with(Instant::now);
    }

    /// Whether a write is owed and its delay has elapsed. Does not clear.
    pub fn due(&self) -> bool {
        matches!(*self.started.lock(), Some(started) if started.elapsed() >= self.delay)
    }

    /// [`Self::due`], clearing the pending flag when it answers true.
    ///
    /// Clearing before the write rather than after is deliberate: a change arriving while
    /// the write is in flight starts a fresh burst and will be written again, whereas
    /// clearing afterwards would swallow it. The cost is that a caller whose write then
    /// fails owns the retry — call [`Self::note_change`] again, or the change waits on disk
    /// for the next unrelated mutation.
    pub fn take(&self) -> bool {
        let mut started = self.started.lock();
        match *started {
            Some(at) if at.elapsed() >= self.delay => {
                *started = None;
                true
            }
            _ => false,
        }
    }
}

impl Default for Debouncer {
    fn default() -> Self {
        Self::new(SAVE_DEBOUNCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cide_ipc::{
        Axis, DiffOrigin, DiffSpec, LayoutNode, Pane, PaneKind, PaneRole, PaneTree, Project,
        ProjectRoot, SettingsSection, Tab, TabKind, WindowRole,
    };
    use cide_ipc::{PaneId, ProjectId, SessionId, SplitId, TabId, WindowLabel};
    use indexmap::IndexMap;

    /// A temp directory that removes itself, so a failing test does not leak into the next
    /// run and a parallel test never shares a path with this one.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cide-persist-{tag}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .expect("read temp dir")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Two projects, several tabs, a split pane tree and a detached-pane window: enough
    /// shape that a round trip through JSON would notice a dropped field. Map *order* is not
    /// among them — `IndexMap`'s `PartialEq` ignores it — so ordering has its own test.
    fn fixture() -> Workspace {
        let mut workspace = Workspace::default();

        let project_a = project("cide", "~/work/cide", true);
        let project_b = project("ui", "~/work/ui", false);
        let (a_id, b_id) = (project_a.id, project_b.id);
        let a_tab = project_a.tabs[0].id;
        let detached = project_a.tabs[0].tree.focused;

        workspace.projects.insert(a_id, project_a);
        workspace.projects.insert(b_id, project_b);
        workspace.windows.insert(
            WindowLabel::shell(),
            WindowRole::Shell {
                projects: vec![a_id, b_id],
                active: Some(a_id),
            },
        );
        workspace.windows.insert(
            WindowLabel::detached_pane(),
            WindowRole::DetachedPane {
                project: a_id,
                tab: a_tab,
                pane: detached,
            },
        );
        workspace.rev = 7;
        workspace
    }

    fn project(name: &str, display_path: &str, split: bool) -> Project {
        let primary_session = SessionId::new();
        let console = Pane {
            id: PaneId::new(),
            kind: PaneKind::Claude,
            role: PaneRole::Primary,
            session: Some(primary_session),
            title: format!("{name} : claude"),
        };
        let mut tree = PaneTree {
            root: LayoutNode::Leaf { pane: console.id },
            focused: console.id,
            maximized: None,
            panes: IndexMap::from_iter([(console.id, console.clone())]),
        };
        if split {
            let shell = Pane {
                id: PaneId::new(),
                kind: PaneKind::Shell,
                role: PaneRole::Auxiliary,
                session: Some(SessionId::new()),
                title: format!("{name} : bash"),
            };
            tree.root = LayoutNode::Split {
                id: SplitId::new(),
                axis: Axis::Row,
                a: Box::new(LayoutNode::Leaf { pane: console.id }),
                b: Box::new(LayoutNode::Leaf { pane: shell.id }),
                ratio: 0.62,
            };
            tree.maximized = Some(shell.id);
            tree.panes.insert(shell.id, shell);
        }

        let home = Tab {
            id: TabId::new(),
            kind: TabKind::ClaudeHome,
            tree,
        };
        let settings_pane = Pane {
            id: PaneId::new(),
            kind: PaneKind::Editor,
            role: PaneRole::Auxiliary,
            session: None,
            title: "settings".into(),
        };
        let settings = Tab {
            id: TabId::new(),
            kind: TabKind::Settings {
                section: SettingsSection::Keymap,
            },
            tree: PaneTree {
                root: LayoutNode::Leaf {
                    pane: settings_pane.id,
                },
                focused: settings_pane.id,
                maximized: None,
                panes: IndexMap::from_iter([(settings_pane.id, settings_pane)]),
            },
        };
        let active_tab = home.id;

        Project {
            id: ProjectId::new(),
            name: name.into(),
            display_path: display_path.into(),
            dot: "var(--accent)".into(),
            roots: vec![ProjectRoot {
                path: PathBuf::from(display_path),
                label: name.into(),
            }],
            tabs: vec![home, settings],
            active_tab,
            tab_mru: vec![active_tab],
            detached: IndexMap::new(),
            dock_anchors: IndexMap::new(),
            primary_session,
        }
    }

    #[test]
    fn a_saved_workspace_loads_back_unchanged() {
        let dir = TempDir::new("round-trip");
        let path = dir.join("workspace.json");
        let workspace = fixture();

        save_atomic(&path, &workspace).expect("save");

        assert_eq!(load(&path), workspace);
    }

    /// A `workspace.json` written before `tabMru` existed still loads, and loads *usable*.
    ///
    /// This is the upgrade path for every file on every disk, and it is the expensive one to
    /// get wrong: `tabMru` is `#[serde(default)]`, so the document parses to an empty order,
    /// `workspace::validate` refuses that, and `WorkspaceState::load` answers a refusal by
    /// replacing the whole workspace with `Workspace::default()`. Miss the repair and the first
    /// launch after the upgrade greets the user with no projects and no tabs — with the file
    /// still on disk, intact, and never read again.
    ///
    /// Renaming the key stands in for the field not being there, the way the `dockAnchors` test
    /// below does: that is the only state that has ever been on anyone's disk.
    #[test]
    fn a_workspace_written_before_the_tab_focus_order_loads_and_validates() {
        let dir = TempDir::new("mru-migration");
        let path = dir.join("workspace.json");
        // `demo_workspace` rather than this module's `fixture`, which carries a maximized pane
        // that does not hold focus and so fails `validate` for a reason that has nothing to do
        // with this test. The demo is the one shared fixture the validator accepts.
        save_atomic(&path, &crate::workspace::demo_workspace()).expect("save");

        let raw = fs::read_to_string(&path).expect("read back");
        assert!(
            raw.contains(r#""tabMru""#),
            "the order is written, not skipped"
        );
        fs::write(&path, raw.replace(r#""tabMru""#, r#""wasNotAFieldYet""#)).expect("rewrite");

        let loaded = load(&path);
        assert!(
            !loaded.projects.is_empty(),
            "the file was read rather than quarantined"
        );
        for p in loaded.projects.values() {
            assert_eq!(
                p.tab_mru,
                vec![p.active_tab],
                "the order is repaired to the one thing the file actually knew"
            );
        }
        crate::workspace::validate(&loaded)
            .expect("and it passes the validator that decides whether the layout survives");
    }

    /// The shape `demo_workspace` claims, asserted on **both** sides of a round trip.
    ///
    /// A helper rather than assertions written once after the load: checked only afterwards,
    /// a fixture that drifted down to two projects would still pass, and the test would go on
    /// reporting a green round trip for something smaller than the criterion names.
    fn assert_demo_shape(ws: &Workspace, when: &str) {
        assert_eq!(ws.projects.len(), 3, "three projects {when}");
        assert_eq!(pane_count(ws), 12, "twelve panes {when}");
        let first = ws.projects.values().next().expect("a project");
        assert_eq!(
            first.roots.len(),
            2,
            "the first project is multi-root {when}"
        );
    }

    /// Panes living in tabs. Detached panes are counted separately where it matters, so that
    /// detaching one in a test does not silently keep this total at twelve.
    fn pane_count(ws: &Workspace) -> usize {
        ws.projects
            .values()
            .flat_map(|p| &p.tabs)
            .map(|t| t.tree.panes.len())
            .sum()
    }

    /// `load` clears file-tab dirty flags by design; the demo carries a dirty file tab. So the
    /// fixture is compared after that same transformation rather than by laundering whatever
    /// came back off disk, which would hide a dropped tab along with the flag.
    ///
    /// `load` also calls `refresh_display_paths`, which is deliberately *not* replayed here:
    /// it recomputes `display_path` from the primary root against the current `$HOME`, and the
    /// fixture's value came from that same function in this same process, so replaying it
    /// would be a no-op. The consequence is worth naming — `display_path` is the one field
    /// these round trips do not really check, because `load` would rebuild it even if serde
    /// dropped it. Anything that needs to pin that field has to compare the bytes on disk.
    fn as_loaded(ws: &Workspace) -> Workspace {
        let mut expected = ws.clone();
        crate::workspace::clear_dirty_flags(&mut expected);
        expected
    }

    /// The round trip the acceptance criterion actually names: three projects, twelve panes,
    /// multi-root.
    ///
    /// `fixture()` above is two projects, five panes and one root each, so before this test
    /// multi-root was never serialised at all. `demo_workspace` is the same fixture
    /// `cide-headless` renders and the validate tests exercise, which is why it is reused here
    /// instead of a fourth hand-built workspace that could drift away from all three.
    #[test]
    fn the_demo_workspace_round_trips_with_its_multi_root_shape_intact() {
        let dir = TempDir::new("demo-round-trip");
        let path = dir.join("workspace.json");
        let workspace = crate::workspace::demo_workspace();
        assert_demo_shape(&workspace, "before the save");

        save_atomic(&path, &workspace).expect("save");
        let loaded = load(&path);

        assert_demo_shape(&loaded, "after the load");
        assert_eq!(
            loaded.projects.values().next().expect("a project").roots,
            workspace.projects.values().next().expect("a project").roots,
            "both roots come back, in order, with their labels"
        );
        assert_eq!(loaded, as_loaded(&workspace));
    }

    /// A `workspace.json` as the build *before* the rows change wrote it: a nested 2x2 tree
    /// — `Row(Col(a, b), Col(c, d))`, columns of stacked tiles — a populated `dockAnchors`,
    /// a live detached pane and a second tab. Captured from `serde_json::to_string_pretty`
    /// on a real `Workspace`, then edited only to give it that shape.
    ///
    /// Checked in as text rather than rebuilt from the constructors, which is the whole
    /// point: a constructor moves with the code, so a test built from one cannot notice the
    /// day the format stops matching what is on somebody's disk.
    const LEGACY_WORKSPACE: &str = r#"{
  "schemaVersion": 1,
  "rev": 41,
  "settings": {
    "windowMode": "stacked",
    "theme": "light",
    "eachProjectKeepsClaudeTab": true,
    "reopenLastProject": true,
    "keepSessionsOnWindowClose": true,
    "confirmCloseWithLiveSession": false,
    "editor": {
      "fontSize": 13,
      "tabSize": 4,
      "insertSpaces": true,
      "showMinimap": true,
      "wordWrap": false,
      "trimTrailingWhitespaceOnSave": false
    },
    "terminal": { "fontSize": 12, "scrollback": 5000, "renderer": "auto" },
    "graphics": {
      "disableDmabufRenderer": null,
      "disableCompositingMode": null,
      "disableNvidiaExplicitSync": null
    },
    "claude": {
      "disableMouse": false,
      "altScreenFullRepaint": false,
      "disableAlternateScreen": false
    }
  },
  "projects": {
    "95a107d5-944e-497c-b75a-dca593009bf9": {
      "id": "95a107d5-944e-497c-b75a-dca593009bf9",
      "name": "cide",
      "displayPath": "~/work/cide",
      "dot": "var(--accent)",
      "roots": [{ "path": "~/work/cide", "repo": null, "label": "cide" }],
      "tabs": [
        {
          "id": "8e5ed863-412d-487f-a6c4-ad5b4ba0b5d1",
          "kind": { "kind": "claudeHome" },
          "tree": {
            "root": {
              "kind": "split",
              "id": "98ce54f1-72d5-4938-b5f3-1e6a6c488704",
              "axis": "row",
              "a": {
                "kind": "split",
                "id": "1c0d4e51-6f0a-4a2b-9b64-2a7ad2c1f001",
                "axis": "col",
                "a": { "kind": "leaf", "pane": "2f130069-a1a3-4788-aff1-eeecd3e9118c" },
                "b": { "kind": "leaf", "pane": "5f4274c3-fbc4-4891-8a54-7ce69e5439f8" },
                "ratio": 0.7
              },
              "b": {
                "kind": "split",
                "id": "1c0d4e51-6f0a-4a2b-9b64-2a7ad2c1f002",
                "axis": "col",
                "a": { "kind": "leaf", "pane": "3a9b1f22-0c71-4d1e-8a55-11aa22bb33cc" },
                "b": { "kind": "leaf", "pane": "44445555-6666-4777-8888-999900001111" },
                "ratio": 0.35
              },
              "ratio": 0.62
            },
            "focused": "3a9b1f22-0c71-4d1e-8a55-11aa22bb33cc",
            "maximized": null,
            "panes": {
              "2f130069-a1a3-4788-aff1-eeecd3e9118c": {
                "id": "2f130069-a1a3-4788-aff1-eeecd3e9118c",
                "kind": "claude",
                "role": "primary",
                "session": "b7bd5681-36fd-4436-a71b-c4a34dcd580e",
                "title": "cide : claude"
              },
              "5f4274c3-fbc4-4891-8a54-7ce69e5439f8": {
                "id": "5f4274c3-fbc4-4891-8a54-7ce69e5439f8",
                "kind": "shell",
                "role": "auxiliary",
                "session": "e0d851bd-6139-412d-a390-4456ea3b9bdf",
                "title": "cide : bash"
              },
              "3a9b1f22-0c71-4d1e-8a55-11aa22bb33cc": {
                "id": "3a9b1f22-0c71-4d1e-8a55-11aa22bb33cc",
                "kind": "claude",
                "role": "auxiliary",
                "session": "aaaabbbb-cccc-4ddd-8eee-ffff00001111",
                "title": "cide : claude — tests"
              },
              "44445555-6666-4777-8888-999900001111": {
                "id": "44445555-6666-4777-8888-999900001111",
                "kind": "diff",
                "role": "auxiliary",
                "session": null,
                "title": "cide : claude — diff"
              }
            }
          }
        },
        {
          "id": "e103d9b3-d11b-4cb7-84fa-2133f79765f0",
          "kind": { "kind": "settings", "section": "keymap" },
          "tree": {
            "root": { "kind": "leaf", "pane": "c7386d65-f163-42bc-9a2c-9bfa6e818815" },
            "focused": "c7386d65-f163-42bc-9a2c-9bfa6e818815",
            "maximized": null,
            "panes": {
              "c7386d65-f163-42bc-9a2c-9bfa6e818815": {
                "id": "c7386d65-f163-42bc-9a2c-9bfa6e818815",
                "kind": "editor",
                "role": "auxiliary",
                "session": null,
                "title": "settings"
              }
            }
          }
        }
      ],
      "activeTab": "8e5ed863-412d-487f-a6c4-ad5b4ba0b5d1",
      "detached": {
        "deadbeef-0000-4111-8222-333344445555": {
          "id": "deadbeef-0000-4111-8222-333344445555",
          "kind": "shell",
          "role": "auxiliary",
          "session": "12121212-3434-4545-8656-767878789090",
          "title": "cide : bash"
        }
      },
      "dockAnchors": {
        "deadbeef-0000-4111-8222-333344445555": {
          "sibling": { "kind": "split", "split": "1c0d4e51-6f0a-4a2b-9b64-2a7ad2c1f002" },
          "split": "1c0d4e51-6f0a-4a2b-9b64-2a7ad2c1f003",
          "axis": "row",
          "side": "after",
          "ratio": 0.73
        }
      },
      "primarySession": "b7bd5681-36fd-4436-a71b-c4a34dcd580e"
    }
  },
  "windows": {
    "shell:0e9a1b2c-3d4e-4f50-8a1b-2c3d4e5f6071": {
      "kind": "shell",
      "projects": ["95a107d5-944e-497c-b75a-dca593009bf9"],
      "active": "95a107d5-944e-497c-b75a-dca593009bf9"
    },
    "pane:0e9a1b2c-3d4e-4f50-8a1b-2c3d4e5f6072": {
      "kind": "detachedPane",
      "project": "95a107d5-944e-497c-b75a-dca593009bf9",
      "tab": "8e5ed863-412d-487f-a6c4-ad5b4ba0b5d1",
      "pane": "deadbeef-0000-4111-8222-333344445555"
    }
  }
}"#;

    /// Criterion 2 of the rows change, now carrying a second job: a real schema-1 document
    /// off somebody's disk still loads, and everything in it still means what it meant.
    ///
    /// It no longer takes the fast path — `CURRENT_SCHEMA` moved to 2 for the proxy scope — so
    /// this is also the proof that the migration ladder does not damage what it walks past.
    /// Every assertion below was written when nothing was migrating this document at all, and
    /// every one of them still has to hold after a round trip through `serde_json::Value`:
    /// the anchors, the ratios, the split ids, the removed `repo` field, the two tabs.
    ///
    /// No quarantine copy appears, and nothing is written beside it: `load` migrates in
    /// memory, and the file on disk is rewritten only by the next ordinary save.
    #[test]
    fn a_schema_1_workspace_off_a_real_disk_migrates_without_losing_anything() {
        let dir = TempDir::new("legacy");
        let path = dir.join("workspace.json");
        fs::write(&path, LEGACY_WORKSPACE).expect("write the captured document");

        let ws = load(&path);

        assert_eq!(Workspace::CURRENT_SCHEMA, 3);
        assert_eq!(
            ws.schema_version, 3,
            "the ladder ran every rung — 1 → 2 → 3 — and stopped at current"
        );
        assert_eq!(
            dir.entries(),
            vec!["workspace.json".to_string()],
            "nothing was quarantined and nothing was written beside it"
        );

        // The scope this user gets is the behaviour they already had: both pane kinds
        // proxied, cide's own `git` inheriting whatever cide itself was launched with.
        // Spelled out rather than compared against `ProxyScope::default()`, because the whole
        // reason the migration writes it is that the default is expected to move.
        assert_eq!(
            ws.settings.proxy.scope.claude,
            cide_ipc::ProxyTarget::Configured
        );
        assert_eq!(
            ws.settings.proxy.scope.shells,
            cide_ipc::ProxyTarget::Configured
        );
        assert_eq!(
            ws.settings.proxy.scope.git,
            cide_ipc::ProxyTarget::Untouched,
            "schema 1 never put a proxy into cide's own git, and an upgrade must not either"
        );

        /*
         * Autosave, on. (M15, schema 3)
         *
         * The `editor` block in this captured document lists six fields and no `autosave` — it
         * predates the field by two milestones, which is exactly what makes it the right thing
         * to assert against. An upgrading user gets autosave on, which is what a fresh install
         * gets, so the upgrade changes nothing they can see today.
         *
         * The literal above is deliberately left alone. Adding `autosave` to it would turn this
         * into a test of a document no user has, which is the one thing a captured fixture is
         * for not being.
         */
        assert!(
            !LEGACY_WORKSPACE.contains("autosave"),
            "the captured document must stay the one that predates the field"
        );
        assert!(
            ws.settings.editor.autosave,
            "a workspace written before autosave existed comes back with it on — the same \
             answer a fresh install gives, so nothing changes under an upgrading user"
        );

        let project = ws.projects.values().next().expect("the project loaded");
        assert_eq!(project.tabs.len(), 2);

        // Every root in this document carries a `"repo": null` that `ProjectRoot` no longer
        // has a field for — nothing in any build ever wrote anything else into it, and the
        // frontend derived a permanently-false `repoOpen` flag from it that hid the whole Git
        // group from the command palette (see `cide_ipc::workspace::ProjectRoot`). This is the
        // assertion behind the claim that dropping it needed no schema bump: serde ignores the
        // unknown key and the root loads with its path and label intact.
        assert!(
            LEGACY_WORKSPACE.contains(r#""repo": null"#),
            "the captured document is still the one written with the removed field"
        );
        assert_eq!(project.roots.len(), 1);
        assert_eq!(project.roots[0].label, "cide");
        assert_eq!(project.roots[0].path, PathBuf::from("~/work/cide"));
        let tree = &project.tabs[0].tree;

        // The tree is the shape it was on disk: two *columns* of stacked tiles, ratios and
        // split ids intact. The flattening renderer draws it correctly as it stands, which is
        // why no migration was needed to make it right.
        let LayoutNode::Split {
            id,
            axis,
            a,
            b,
            ratio,
        } = &tree.root
        else {
            panic!("the root is still a split");
        };
        assert_eq!(id.to_string(), "98ce54f1-72d5-4938-b5f3-1e6a6c488704");
        assert_eq!(*axis, Axis::Row);
        assert!((ratio - 0.62).abs() < 1e-6);
        for (side, want) in [(a, Axis::Col), (b, Axis::Col)] {
            let LayoutNode::Split { axis, .. } = side.as_ref() else {
                panic!("a column survived as a column");
            };
            assert_eq!(*axis, want);
        }

        // Every pane, every session binding, and both flags.
        assert_eq!(tree.panes.len(), 4);
        assert_eq!(crate::layout::leaves(&tree.root).len(), 4);
        assert_eq!(
            tree.focused.to_string(),
            "3a9b1f22-0c71-4d1e-8a55-11aa22bb33cc"
        );
        assert_eq!(tree.maximized, None);
        assert!(
            tree.panes.values().filter(|p| p.session.is_some()).count() == 3,
            "the three panes with a live conversation kept their binding"
        );

        // And the detached pane's anchor still names a split that is really there, so it
        // re-docks exactly rather than falling back to a guess.
        assert_eq!(project.detached.len(), 1);
        let (pane, anchor) = project.dock_anchors.iter().next().expect("one anchor");
        assert!(project.detached.contains_key(pane));
        assert!(
            crate::layout::can_restore(tree, anchor),
            "the anchor survived the upgrade"
        );

        crate::workspace::validate(&ws).expect("no invariant was tightened under it");
    }

    /// `Project::detached` is `#[serde(default)]`, so a field lost to a rename or a typo
    /// deserialises as an empty map instead of failing loudly — and a torn-out pane holds a
    /// live `SessionId`, so losing it silently orphans a running conversation.
    #[test]
    fn a_detached_pane_map_survives_a_round_trip() {
        let dir = TempDir::new("demo-detached");
        let path = dir.join("workspace.json");

        let mut workspace = crate::workspace::demo_workspace();
        let (project_id, tab_id, pane_id, session) = {
            let (id, p) = workspace.projects.first().expect("a demo project");
            let console = &p.tabs[0];
            // The last console pane that *carries a session*, not simply the last pane. The
            // console's last pane is the session-less diff, and detaching that one leaves the
            // session assertion below comparing `None` to `None` — green no matter what serde
            // did to the field, which is the one thing this test exists to catch. `take_pane`
            // only refuses a tab's sole leaf, and the console has four, so any of them is
            // detachable. Going through `detach_pane` rather than inserting into `detached` by
            // hand keeps the fixture one `validate` accepts.
            let (pane, kept) = console
                .tree
                .panes
                .iter()
                .rev()
                .find(|(_, pane)| pane.session.is_some())
                .expect("a console pane bound to a session");
            (*id, console.id, *pane, kept.session)
        };
        // Stated as an assertion rather than left to the `find` above: if the demo ever stops
        // giving its console panes sessions, this test must fail loudly instead of quietly
        // round-tripping a `None` and still reporting that the binding survives.
        assert!(
            session.is_some(),
            "the detached pane is bound to a live session, which is what makes losing it cost \
             something"
        );

        crate::workspace::detach_pane(&mut workspace, project_id, tab_id, pane_id).expect("detach");
        let detached = workspace.projects[&project_id].detached.clone();
        assert_eq!(detached.len(), 1, "the fixture really did detach a pane");
        assert_eq!(pane_count(&workspace), 11, "the pane left its tab");

        save_atomic(&path, &workspace).expect("save");
        let loaded = load(&path);

        assert_eq!(
            loaded.projects[&project_id].detached, detached,
            "the detached pane, its key, its kind and its session all come back"
        );
        // Spelled out separately from the map equality above so that a dropped `session`
        // names itself in the failure rather than showing up as a whole-`Pane` diff — and so
        // the claim in this test's doc comment is actually carried by an assertion.
        assert_eq!(
            loaded.projects[&project_id].detached[&pane_id].session, session,
            "the pane comes back still bound to its conversation"
        );
        assert_eq!(loaded, as_loaded(&workspace));

        // The assertion above is only worth writing because the failure it guards is silent.
        // Renaming the key stands in for any way the field could go missing — a schema change,
        // a serde rename — and shows what `#[serde(default)]` does with it: no error, no
        // warning, just a project that has forgotten a pane holding a live session.
        let raw = fs::read_to_string(&path).expect("read the file back");
        assert!(
            raw.contains(r#""detached""#),
            "the map is written, not skipped"
        );
        let mangled: Workspace =
            serde_json::from_str(&raw.replace(r#""detached""#, r#""detachedPanes""#))
                .expect("a workspace missing `detached` still deserialises");
        assert!(
            mangled.projects[&project_id].detached.is_empty(),
            "a missing `detached` defaults to empty rather than failing, which is the point"
        );
    }

    /// A workspace is saved with its panes still detached, so the position a detached pane
    /// re-docks into has to survive a quit — otherwise the one gesture that most obviously
    /// *should* be exact (detach, quit, relaunch, put it back) is the one that never is.
    ///
    /// The proof is the re-dock itself, performed on the workspace that came off disk and
    /// compared against the tree as it stood before the detach. Asserting only that the map
    /// round-trips would pass on an anchor whose `ratio` serialised as a string or whose
    /// `sibling` tag was renamed, because nothing would have tried to *use* it.
    ///
    /// It also asserts the direction that costs more if it breaks: a file written *before*
    /// `dockAnchors` existed still loads. That is every user's file, and the schema version
    /// was not bumped, so `#[serde(default)]` is the only thing between them and `load`
    /// quarantining their whole workspace.
    #[test]
    fn a_detached_panes_position_survives_a_quit() {
        let dir = TempDir::new("dock-anchor");
        let path = dir.join("workspace.json");

        let mut workspace = crate::workspace::demo_workspace();
        let (project_id, tab_id, pane_id) = {
            let (id, p) = workspace.projects.first().expect("a demo project");
            let console = &p.tabs[0];
            // The last pane of the console: it sits deepest in the tree, so a restore that
            // re-split at the root rather than at the recorded sibling would show up here.
            let pane = *console.tree.panes.keys().next_back().expect("panes");
            (*id, console.id, pane)
        };
        // Drag *the pane's own divider* off the 0.5 a fresh split writes, so a lost ratio
        // cannot pass by coincidence — and so the number under test is the one the re-dock
        // has to reinstate rather than one belonging to a split that never collapsed.
        let before = {
            let t = crate::workspace::tab_mut(&mut workspace, project_id, tab_id).expect("exists");
            let divider = crate::layout::anchor_of(&t.tree, pane_id)
                .expect("a console pane below the root")
                .split;
            crate::layout::set_ratio(&mut t.tree, divider, 0.73).expect("a live divider moves");
            t.tree.clone()
        };

        let label = crate::workspace::detach_pane(&mut workspace, project_id, tab_id, pane_id)
            .expect("detach");
        let anchors = workspace.projects[&project_id].dock_anchors.clone();
        assert_eq!(anchors.len(), 1, "detaching recorded where the pane was");

        save_atomic(&path, &workspace).expect("save");
        let mut loaded = load(&path);

        assert_eq!(
            loaded.projects[&project_id].dock_anchors, anchors,
            "the anchor comes back: sibling, divider id, axis, side and ratio"
        );
        // The bytes, not just the Rust round trip: `dockAnchors` and the nested `sibling` tag
        // are read by nothing in Rust that would notice a rename, and the frontend's
        // `DockSibling` is generated from this shape. (`rename_all_fields` on that enum is
        // house style rather than something this can catch: both variants carry a single
        // already-lowercase field, so removing it changes no byte.)
        let raw = fs::read_to_string(&path).expect("read the file back");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
        let stored = &value["projects"][project_id.to_string()]["dockAnchors"][pane_id.to_string()];
        assert!(
            stored.is_object(),
            "the anchor is written under `dockAnchors`"
        );
        assert!(
            stored["sibling"]["kind"] == "pane" || stored["sibling"]["kind"] == "split",
            "the sibling keeps its camelCase tag, got {:?}",
            stored["sibling"]
        );
        // Compared with a tolerance, not for equality: serde_json widens the `f32` to `f64`
        // before writing, so the file reads `0.7300000190734863` and an exact match would be
        // asserting the widening rather than the ratio.
        let ratio = stored["ratio"].as_f64().expect("the ratio is a number");
        assert!(
            (ratio - 0.73).abs() < 1e-6,
            "the divider position is written as a number, got {ratio}"
        );

        // And it is still usable: the tab comes back the tree it was, ratio included.
        crate::workspace::redock_pane(&mut loaded, &label).expect("redocks after the reload");
        let after = &crate::workspace::tab(&loaded, project_id, tab_id)
            .expect("exists")
            .tree;
        assert_eq!(
            after.root, before.root,
            "the position did not survive the quit"
        );
        crate::workspace::validate(&loaded).expect("valid");

        // Every `workspace.json` in existence was written before `dockAnchors` did, and this
        // change did not bump `Workspace::CURRENT_SCHEMA` — so those files are read straight
        // from the text by `read_workspace` with no migration step to fill the field in.
        // Without `#[serde(default)]` the whole document fails to deserialise, `load` sees a
        // `CoreError::Serde` and *quarantines* it: every project, tab and pane the user had
        // is renamed aside and they are handed an empty workspace. Renaming the key stands in
        // for the field simply not being there, which is the only state that has ever been on
        // anyone's disk. The sibling assertion on `detached` in
        // `a_detached_pane_map_survives_a_round_trip` exists for the same reason.
        assert!(
            raw.contains(r#""dockAnchors""#),
            "the map is written, not skipped"
        );
        let older: Workspace =
            serde_json::from_str(&raw.replace(r#""dockAnchors""#, r#""wasNotAFieldYet""#))
                .expect("a workspace predating `dockAnchors` still deserialises");
        assert!(
            older.projects[&project_id].dock_anchors.is_empty(),
            "a missing `dockAnchors` defaults to empty rather than failing the whole file"
        );
        // And the pane is still re-dockable from such a file — sparse anchors are the
        // documented state, not a corrupt one.
        let mut older = older;
        crate::workspace::redock_pane(&mut older, &label).expect("redocks without an anchor");
        crate::workspace::validate(&older).expect("valid");
    }

    /// `DiffOrigin` has struct variants, and a `serde(tag)` enum with struct variants needs
    /// `rename_all_fields` on top of `rename_all` — the outer one does not reach inside a
    /// variant. Nothing round-tripped this type, and the failure is invisible from Rust: a
    /// `request_id` key deserialises back into Rust perfectly well while every other reader of
    /// the wire format sees the wrong name. Hence the assertion on the bytes, not just on
    /// equality.
    #[test]
    fn a_claude_mcp_diff_tab_round_trips_with_camel_case_fields() {
        let dir = TempDir::new("diff-origin");
        let path = dir.join("workspace.json");

        let mut workspace = fixture();
        let pane = Pane {
            id: PaneId::new(),
            kind: PaneKind::Diff,
            role: PaneRole::Auxiliary,
            session: None,
            title: "main.rs — diff".into(),
        };
        let project = workspace.projects.values_mut().next().expect("a project");
        project.tabs.push(Tab {
            id: TabId::new(),
            kind: TabKind::Diff {
                spec: DiffSpec {
                    title: "main.rs".into(),
                    old_path: PathBuf::from("/home/dev/work/cide/src/main.rs"),
                    new_path: PathBuf::from("/home/dev/work/cide/src/main.rs.new"),
                    origin: DiffOrigin::ClaudeMcp {
                        request_id: "req-42".into(),
                    },
                },
                preview: false,
            },
            tree: PaneTree {
                root: LayoutNode::Leaf { pane: pane.id },
                focused: pane.id,
                maximized: None,
                panes: IndexMap::from_iter([(pane.id, pane)]),
            },
        });

        save_atomic(&path, &workspace).expect("save");

        let raw = fs::read_to_string(&path).expect("read the file back");
        assert!(
            raw.contains(r#""requestId": "req-42""#),
            "the blocked request's id is spelled camelCase on the wire; got:\n{raw}"
        );
        assert!(
            !raw.contains("request_id"),
            "a snake_case key means `rename_all_fields` is missing"
        );
        assert_eq!(load(&path), workspace);
    }

    /// The dirty flag describes a buffer, and buffers do not survive a quit. Restoring one
    /// set would leave a tab that `close_tab` refuses over edits that exist nowhere — a
    /// destructive confirmation with nothing behind it, and no gesture that clears it.
    #[test]
    fn a_file_tab_saved_dirty_loads_back_clean() {
        let dir = TempDir::new("dirty-reset");
        let path = dir.join("workspace.json");

        let mut workspace = fixture();
        let project = workspace.projects.values_mut().next().expect("a project");
        let pane = Pane {
            id: PaneId::new(),
            kind: PaneKind::Editor,
            role: PaneRole::Auxiliary,
            session: None,
            title: "main.rs".into(),
        };
        project.tabs.push(Tab {
            id: TabId::new(),
            kind: TabKind::File {
                path: PathBuf::from("/home/dev/work/cide/src/main.rs"),
                dirty: true,
            },
            tree: PaneTree {
                root: LayoutNode::Leaf { pane: pane.id },
                focused: pane.id,
                maximized: None,
                panes: IndexMap::from_iter([(pane.id, pane)]),
            },
        });

        save_atomic(&path, &workspace).expect("save");

        let loaded = load(&path);
        let restored = loaded
            .projects
            .values()
            .flat_map(|p| &p.tabs)
            .find_map(|t| match &t.kind {
                TabKind::File { dirty, .. } => Some(*dirty),
                _ => None,
            })
            .expect("the file tab came back");
        assert!(!restored, "a restored file tab starts clean");
        // Everything else still round-trips: this clears one flag, not the tab.
        assert_eq!(
            loaded
                .projects
                .values()
                .flat_map(|p| &p.tabs)
                .filter(|t| matches!(t.kind, TabKind::File { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn saving_creates_missing_parent_directories() {
        let dir = TempDir::new("mkdir");
        let path = dir.join("nested").join("deeper").join("workspace.json");

        save_atomic(&path, &Workspace::default()).expect("save");

        assert_eq!(load(&path), Workspace::default());
    }

    /// The file holds a proxy password now, so its mode is part of its contract.
    ///
    /// `Settings::proxy` stores `http://user:hunter2@proxy.corp:3128` verbatim — there is no
    /// keyring yet and a proxy that authenticates cannot be used otherwise. At the 0644 a
    /// plain `File::create` produces, every other account on the machine can read it.
    ///
    /// The second save is the point of the test as much as the first: an existing workspace
    /// written by an older build is at 0644, and the rename has to replace it rather than
    /// write through it, or the tightening never reaches anybody who already has one.
    #[cfg(unix)]
    #[test]
    fn a_saved_workspace_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("private");
        let path = dir.join("workspace.json");

        // Stand in for the pre-existing, world-readable file of an older build.
        fs::write(&path, b"{}").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen");

        save_atomic(&path, &fixture()).expect("save");

        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "workspace.json holds a proxy password");
    }

    #[test]
    fn a_successful_save_leaves_no_temp_file_behind() {
        let dir = TempDir::new("no-temp");
        let path = dir.join("workspace.json");

        save_atomic(&path, &fixture()).expect("save");
        save_atomic(&path, &fixture()).expect("overwrite");

        assert_eq!(dir.entries(), vec!["workspace.json".to_owned()]);
    }

    #[test]
    fn a_round_trip_preserves_the_header_tab_order() {
        let dir = TempDir::new("order");
        let path = dir.join("workspace.json");

        let mut workspace = Workspace::default();
        // Ids that descend, so that any sort by key visibly reverses the user's order.
        for (n, name) in ["zeta", "mu", "alpha"].iter().enumerate() {
            let mut project = project(name, "~/work", false);
            project.id = ProjectId::from(uuid::Uuid::from_u128(3 - n as u128));
            workspace.projects.insert(project.id, project);
        }
        for (n, label) in ["shell:c", "shell:b", "shell:a"].iter().enumerate() {
            let project = workspace.projects[n].id;
            workspace.windows.insert(
                WindowLabel((*label).to_owned()),
                WindowRole::Shell {
                    projects: vec![project],
                    active: Some(project),
                },
            );
        }

        save_atomic(&path, &workspace).expect("save");
        let loaded = load(&path);

        assert_eq!(
            loaded
                .projects
                .values()
                .map(|p| &p.name)
                .collect::<Vec<_>>(),
            vec!["zeta", "mu", "alpha"],
            "insertion order is the header tab order"
        );
        assert_eq!(
            loaded
                .windows
                .keys()
                .map(|l| l.as_str())
                .collect::<Vec<_>>(),
            vec!["shell:c", "shell:b", "shell:a"]
        );
    }

    #[test]
    fn concurrent_saves_each_publish_a_whole_file() {
        let dir = TempDir::new("concurrent");
        let path = dir.join("workspace.json");

        let mut large = fixture();
        large.projects[0].name = "x".repeat(200_000);
        let small = Workspace::default();

        save_atomic(&path, &small).expect("seed");

        std::thread::scope(|scope| {
            let readers = scope.spawn(|| {
                for _ in 0..400 {
                    let raw = fs::read_to_string(&path).expect("read");
                    serde_json::from_str::<Workspace>(&raw).expect("published file is whole");
                }
            });
            let a = scope.spawn(|| {
                for _ in 0..200 {
                    save_atomic(&path, &large).expect("save large");
                }
            });
            for _ in 0..200 {
                save_atomic(&path, &small).expect("save small");
            }
            a.join().expect("large saver");
            readers.join().expect("reader");
        });
    }

    #[test]
    fn a_corrupt_file_yields_defaults_and_is_moved_aside() {
        let dir = TempDir::new("corrupt");
        let path = dir.join("workspace.json");
        fs::write(&path, br#"{"schemaVersion":1,"rev":"#).expect("write");

        assert_eq!(load(&path), Workspace::default());

        assert!(!path.exists(), "the bad file must not stay in the way");
        assert_eq!(
            fs::read_to_string(dir.join("workspace.corrupt-1.json")).expect("quarantined"),
            r#"{"schemaVersion":1,"rev":"#,
            "the user's bytes must survive verbatim"
        );
    }

    #[test]
    fn an_empty_file_yields_defaults_and_is_moved_aside() {
        let dir = TempDir::new("empty");
        let path = dir.join("workspace.json");
        fs::write(&path, b"").expect("write");

        assert_eq!(load(&path), Workspace::default());

        assert!(!path.exists());
        assert!(dir.join("workspace.corrupt-1.json").exists());
    }

    #[test]
    fn a_well_formed_file_of_the_wrong_shape_yields_defaults() {
        let dir = TempDir::new("wrong-shape");
        let path = dir.join("workspace.json");
        // Valid JSON, current schema, but `rev` is not a number.
        fs::write(&path, br#"{"schemaVersion":1,"rev":"soon"}"#).expect("write");

        assert_eq!(load(&path), Workspace::default());
        assert!(dir.join("workspace.corrupt-1.json").exists());
    }

    #[test]
    fn repeated_corruption_picks_the_next_free_quarantine_name() {
        let dir = TempDir::new("repeat");
        let path = dir.join("workspace.json");

        for _ in 0..3 {
            fs::write(&path, b"not json").expect("write");
            assert_eq!(load(&path), Workspace::default());
        }

        assert_eq!(
            dir.entries(),
            vec![
                "workspace.corrupt-1.json".to_owned(),
                "workspace.corrupt-2.json".to_owned(),
                "workspace.corrupt-3.json".to_owned(),
            ],
            "no earlier quarantined copy may be overwritten"
        );
    }

    #[test]
    fn a_missing_file_yields_defaults_without_creating_anything() {
        let dir = TempDir::new("missing");
        let path = dir.join("workspace.json");

        assert_eq!(load(&path), Workspace::default());

        assert!(dir.entries().is_empty(), "load must not write");
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_half_understood() {
        let newer = serde_json::json!({
            "schemaVersion": Workspace::CURRENT_SCHEMA + 1,
            "rev": 3,
            "settings": {},
            "projects": {},
            "windows": {},
            "somethingThisBuildWouldDrop": true,
        });

        assert!(migrate(newer.clone()).is_err());

        let dir = TempDir::new("newer");
        let path = dir.join("workspace.json");
        fs::write(&path, newer.to_string()).expect("write");

        assert_eq!(load(&path), Workspace::default());
        assert!(
            dir.join("workspace.corrupt-1.json").exists(),
            "the newer file must stay recoverable by the build that wrote it"
        );
    }

    #[test]
    fn a_document_without_a_schema_version_is_refused_as_pre_1() {
        let pre_1 = serde_json::json!({ "rev": 0, "projects": {}, "windows": {} });

        assert!(migrate(pre_1).is_err());
    }

    #[test]
    fn a_current_schema_document_migrates_to_itself() {
        let workspace = fixture();
        let value = serde_json::to_value(&workspace).expect("serialise");

        assert_eq!(migrate(value).expect("migrate"), workspace);
    }

    /// **The header tab order survives a migration, and it took a Cargo feature to make that
    /// true.**
    ///
    /// `projects` is an `IndexMap` whose insertion order *is* the order of the tabs across the
    /// top of the window. A migration necessarily routes the document through a
    /// `serde_json::Value`, and a stock `serde_json` stores object keys in a `BTreeMap` — so
    /// without the `preserve_order` feature, the first user to upgrade into schema 2 would
    /// have found their tabs silently re-sorted by uuid. Nothing would have failed; a
    /// migration test that only checked the *set* of projects would have passed.
    ///
    /// The two ids below are chosen so that insertion order and sorted order disagree: `f…`
    /// is inserted first and sorts last. A build without `preserve_order` answers `["a…",
    /// "f…"]` here.
    #[test]
    fn a_migration_does_not_re_sort_the_header_tabs() {
        // Not `fixture()`: this needs two projects whose uuids sort against the order they
        // were opened in, which is the only shape that can catch the bug.
        let first = "fedcba98-0000-4000-8000-000000000001";
        let second = "abcdef01-0000-4000-8000-000000000002";

        // Built from the real constructor and then re-keyed, rather than written out as a
        // JSON literal: a literal is a second copy of `Project`'s wire shape, and the next
        // field added to it would break this test for a reason that has nothing to do with
        // ordering.
        let at = |id: &str, name: &str| {
            let mut value = serde_json::to_value(project(name, "~/x", false)).expect("serialise");
            value["id"] = Value::from(id);
            value
        };
        let mut projects = serde_json::Map::new();
        projects.insert(first.into(), at(first, "opened first"));
        projects.insert(second.into(), at(second, "opened second"));

        let document = serde_json::json!({
            "schemaVersion": 1,
            "rev": 7,
            "settings": {},
            "projects": Value::Object(projects),
            "windows": {},
        });

        let ws = migrate(document).expect("a schema 1 document migrates");

        let order: Vec<&str> = ws.projects.values().map(|p| p.name.as_str()).collect();
        assert_eq!(
            order,
            vec!["opened first", "opened second"],
            "the tab strip came back sorted by uuid — serde_json's `preserve_order` feature \
             is what stops that, and this document is shaped so the two orders disagree"
        );
    }

    /// The migration writes the scope down rather than leaning on the serde default, so that
    /// a later change to `ProxyScope::default()` cannot re-scope a live configuration.
    ///
    /// Asserted on the *document*, not on the loaded struct: a struct-level assertion passes
    /// identically whether the value was written or defaulted, which is exactly the two cases
    /// this exists to tell apart.
    #[test]
    fn the_migration_records_the_scope_in_the_document_rather_than_defaulting_it() {
        let v1 = serde_json::json!({
            "schemaVersion": 1,
            "rev": 0,
            "settings": { "proxy": { "mode": "manual", "http": "http://proxy.corp:3128" } },
            "projects": {},
            "windows": {},
        });

        let migrated = v1_to_v2(v1);

        assert_eq!(migrated["schemaVersion"], serde_json::json!(2));
        assert_eq!(
            migrated["settings"]["proxy"]["scope"],
            serde_json::json!({ "claude": "configured", "shells": "configured", "git": "untouched" }),
            "the value has to be in the document; a default is an inference, and inferences \
             change when the constant does"
        );
        // And the user's own configuration is not disturbed on the way past.
        assert_eq!(
            migrated["settings"]["proxy"]["http"],
            serde_json::json!("http://proxy.corp:3128")
        );
    }

    /// The autosave migration writes the value down rather than leaning on the serde default.
    ///
    /// Asserted on the **document**, not on the loaded struct, and that distinction is the
    /// entire test: a struct-level assertion passes identically whether the value was written
    /// or defaulted, which is precisely the two cases this exists to tell apart. The same shape
    /// as `the_migration_records_the_scope_in_the_document_rather_than_defaulting_it` one screen
    /// up, because it is the same argument about a setting with a larger blast radius —
    /// autosave writes the user's files.
    #[test]
    fn the_migration_records_autosave_in_the_document_rather_than_defaulting_it() {
        let v2 = serde_json::json!({
            "schemaVersion": 2,
            "rev": 7,
            "settings": { "editor": { "fontSize": 13.0, "wordWrap": true } },
            "projects": {},
            "windows": {},
        });

        let migrated = v2_to_v3(v2);

        assert_eq!(migrated["schemaVersion"], serde_json::json!(3));
        assert_eq!(
            migrated["settings"]["editor"]["autosave"],
            serde_json::json!(true),
            "the value has to be in the document. A default is an inference from a constant in \
             this build, and the day somebody makes autosave opt-in that inference silently \
             turns it off for every user who had come to rely on it"
        );
        // And the settings the user actually chose are not disturbed on the way past.
        assert_eq!(
            migrated["settings"]["editor"]["wordWrap"],
            serde_json::json!(true)
        );
        assert_eq!(
            migrated["settings"]["editor"]["fontSize"],
            serde_json::json!(13.0)
        );
    }

    /// A schema-2 document with no `settings` block at all — the majority, since every field of
    /// `Settings` defaults and most users never open it — still gets the value recorded.
    #[test]
    fn a_schema_2_document_with_no_editor_block_still_gets_autosave_recorded() {
        let v2 = serde_json::json!({
            "schemaVersion": 2, "rev": 0, "projects": {}, "windows": {},
        });
        let migrated = v2_to_v3(v2);
        assert_eq!(
            migrated["settings"]["editor"]["autosave"],
            serde_json::json!(true)
        );
    }

    /// A value already in the document is left exactly as the user left it.
    ///
    /// It cannot occur in a file this build wrote — schema 2 predates the field — but a
    /// hand-edited workspace or a downgrade-then-upgrade can carry one, and a migration that
    /// overwrites a value somebody typed is the failure this whole ladder exists to avoid.
    #[test]
    fn a_migration_does_not_overwrite_an_autosave_the_user_already_chose() {
        let v2 = serde_json::json!({
            "schemaVersion": 2,
            "rev": 0,
            "settings": { "editor": { "autosave": false } },
            "projects": {},
            "windows": {},
        });
        let migrated = v2_to_v3(v2);
        assert_eq!(
            migrated["settings"]["editor"]["autosave"],
            serde_json::json!(false),
            "somebody turned it off on purpose; an upgrade must not turn it back on"
        );
    }

    /// A document that never had a `settings` block still gets one, because the alternative
    /// is a migration that records nothing for the users who never opened Settings — which is
    /// most of them.
    #[test]
    fn a_document_with_no_settings_block_still_gets_a_recorded_scope() {
        let v1 = serde_json::json!({
            "schemaVersion": 1, "rev": 0, "projects": {}, "windows": {},
        });

        let migrated = v1_to_v2(v1);
        assert_eq!(
            migrated["settings"]["proxy"]["scope"]["git"],
            serde_json::json!("untouched")
        );
        // And it still loads, which is the point of writing into a document at all.
        let ws: Workspace = serde_json::from_value(migrated).expect("the result deserialises");
        assert_eq!(ws.settings.proxy.scope, cide_ipc::ProxyScope::default());
    }

    /// A hand-broken `settings` is left alone and allowed to fail with serde's own message.
    ///
    /// Overwriting it would turn "your settings block is corrupt" into "your proxy
    /// configuration silently became the default", which is the wrong answer to give somebody
    /// who has been editing the file by hand.
    #[test]
    fn a_settings_block_that_is_not_an_object_is_not_rewritten() {
        let v1 = serde_json::json!({
            "schemaVersion": 1, "rev": 0,
            "settings": "this is not a settings block",
            "projects": {}, "windows": {},
        });

        let migrated = v1_to_v2(v1);
        assert_eq!(
            migrated["settings"],
            serde_json::json!("this is not a settings block")
        );
        assert!(
            migrate(migrated).is_err(),
            "and it is refused rather than half-understood"
        );
    }

    #[test]
    fn an_absolute_xdg_override_is_honoured_and_a_relative_one_ignored() {
        assert_eq!(
            xdg_dir(Some("/srv/state".into()), ".local/state"),
            PathBuf::from("/srv/state/cide")
        );
        // A relative override would resolve against the working directory, so the default
        // wins instead.
        assert!(
            xdg_dir(Some("relative/state".into()), ".local/state").ends_with(".local/state/cide")
        );
        assert!(xdg_dir(None, ".local/state").ends_with(".local/state/cide"));
    }

    #[test]
    fn the_well_known_paths_sit_under_their_base_directories() {
        assert_eq!(workspace_path(), state_dir().join("workspace.json"));
        assert_eq!(keymap_path(), config_dir().join("keymap.json"));
        // Beside the workspace, not inside it, and not in config. See `recent_path`.
        assert_eq!(recent_path(), state_dir().join("recent.json"));
        assert_ne!(recent_path(), workspace_path());
    }

    // --- recent projects ------------------------------------------------------------------

    fn recents(entries: &[(&str, u64)]) -> Vec<RecentProject> {
        entries
            .iter()
            .map(|(path, opened_at)| RecentProject {
                path: PathBuf::from(path),
                name: file_name_or(Path::new(path), "project"),
                display_path: path.to_string(),
                opened_at: *opened_at,
            })
            .collect()
    }

    #[test]
    fn remembering_a_project_puts_it_first() {
        let mut list = recents(&[("/a", 10), ("/b", 20)]);
        remember_recent(&mut list, Path::new("/c"), "c", 30);

        assert_eq!(
            list.iter().map(|e| e.path.as_path()).collect::<Vec<_>>(),
            [Path::new("/c"), Path::new("/a"), Path::new("/b")],
        );
    }

    #[test]
    fn reopening_a_project_moves_it_rather_than_duplicating_it() {
        // The failure this guards is a menu that lists the same folder four times because the
        // user opened it four times — the identity is the path, not the open.
        let mut list = recents(&[("/a", 10), ("/b", 20)]);
        remember_recent(&mut list, Path::new("/a"), "renamed", 30);

        assert_eq!(list.len(), 2, "the second open added an entry");
        assert_eq!(list[0].path, PathBuf::from("/a"));
        assert_eq!(list[0].opened_at, 30);
        assert_eq!(list[0].name, "renamed", "the name did not follow the open");
    }

    #[test]
    fn the_list_is_capped_and_drops_the_oldest() {
        let mut list = Vec::new();
        for n in 0..(MAX_RECENT + 5) {
            remember_recent(&mut list, &PathBuf::from(format!("/p{n}")), "p", n as u64);
        }

        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0].path, PathBuf::from(format!("/p{}", MAX_RECENT + 4)));
        assert!(
            !list.iter().any(|e| e.path == Path::new("/p0")),
            "the cap kept the oldest and dropped a recent one",
        );
    }

    // --- per-file view positions ----------------------------------------------------------

    fn at(path: &str, line: u32) -> ViewPosition {
        ViewPosition {
            path: PathBuf::from(path),
            top_line: line.saturating_sub(5).max(1),
            line,
            column: 1,
            touched_at: 0,
        }
    }

    #[test]
    fn one_entry_per_file_however_often_it_is_noted() {
        let mut list = Vec::new();
        remember_position(&mut list, at("/a.rs", 10), 100);
        remember_position(&mut list, at("/b.rs", 20), 110);
        remember_position(&mut list, at("/a.rs", 900), 120);

        assert_eq!(
            list.len(),
            2,
            "a file is remembered once, not once per scroll"
        );
        let a = position_for(&list, Path::new("/a.rs")).expect("a is remembered");
        assert_eq!(a.line, 900, "and the newest note wins");
        assert_eq!(
            a.touched_at, 120,
            "the clock is the store's, whatever the caller sent — it is the eviction key"
        );
    }

    #[test]
    fn the_position_store_evicts_the_least_recently_touched() {
        /*
         * **The list is built the way a *load* produces one — newest first — and that is the
         * whole point of the test.**
         *
         * Filling it with `remember_position` alone proves nothing about the eviction rule: that
         * function retains-then-pushes, so a list it built by itself already has recency and
         * insertion order agreeing, and "drop element 0" would pass. A mutation test caught
         * exactly that. `load_positions` sorts newest-first before handing the list over, so the
         * live list on every launch after the first has element 0 as the *most* recently touched
         * — and a cap that dropped the front would evict the file the user was reading when they
         * quit, every time.
         */
        let mut list: Vec<ViewPosition> = (0..MAX_POSITIONS)
            .map(|n| ViewPosition {
                // Newest first: n = 0 is the most recently touched.
                touched_at: (MAX_POSITIONS - n) as u64,
                ..at(&format!("/f{n}.rs"), 1)
            })
            .collect();

        remember_position(&mut list, at("/new.rs", 1), 10_001);

        assert_eq!(list.len(), MAX_POSITIONS);
        assert!(
            position_for(&list, Path::new("/f0.rs")).is_some(),
            "the front of a freshly loaded list is the NEWEST entry; evicting it would throw \
             away the file the user was reading when they quit",
        );
        assert!(
            position_for(&list, Path::new(&format!("/f{}.rs", MAX_POSITIONS - 1))).is_none(),
            "the genuinely least recently touched entry is the one that goes",
        );

        // And re-touching an entry rescues it, which is the other half of an LRU.
        let oldest = format!("/f{}.rs", MAX_POSITIONS - 2);
        remember_position(&mut list, at(&oldest, 42), 20_000);
        remember_position(&mut list, at("/newer.rs", 1), 20_001);
        assert_eq!(list.len(), MAX_POSITIONS);
        assert!(
            position_for(&list, Path::new(&oldest)).is_some(),
            "the file the user just came back to must not be the one evicted",
        );
    }

    #[test]
    fn two_files_noted_in_the_same_millisecond_evict_the_same_way_whatever_order_they_arrived() {
        /*
         * Ties are real: two panes showing two files are noted within one millisecond routinely,
         * and `now_ms` has millisecond resolution. `sort_by` is *stable*, so without the
         * tie-break the surviving entry is whichever happened to be earlier in the list — which
         * differs between a fresh session and one restored from disk, for the same set of files.
         * The user-visible symptom is a position that survives on one machine and is evicted on
         * another, which is unreportable.
         */
        let victim = |mut list: Vec<ViewPosition>| {
            remember_position(&mut list, at("/new.rs", 1), 9);
            let mut kept: Vec<String> = list
                .iter()
                .map(|e| e.path.to_string_lossy().into_owned())
                .collect();
            kept.sort();
            kept
        };

        let tied = |names: &[&str]| -> Vec<ViewPosition> {
            let mut list: Vec<ViewPosition> = (0..MAX_POSITIONS - names.len())
                .map(|n| ViewPosition {
                    touched_at: 1_000 + n as u64,
                    ..at(&format!("/bulk{n}.rs"), 1)
                })
                .collect();
            // All on the same tick, which is what makes the order below the only difference.
            list.extend(names.iter().map(|name| ViewPosition {
                touched_at: 7,
                ..at(name, 1)
            }));
            list
        };

        assert_eq!(
            victim(tied(&["/a.rs", "/b.rs"])),
            victim(tied(&["/b.rs", "/a.rs"])),
            "the same set of files, two arrival orders, one eviction — the tie-break by path is \
             what makes the order total",
        );
    }

    #[test]
    fn positions_round_trip_and_a_broken_file_costs_nothing() {
        let dir = TempDir::new("positions");
        let path = dir.join("positions.json");

        let mut list = Vec::new();
        remember_position(&mut list, at("/a.rs", 120), 10);
        save_positions(&path, &list).expect("save");

        let loaded = load_positions(&path);
        let entry = position_for(&loaded, Path::new("/a.rs")).expect("a is remembered");
        assert_eq!((entry.line, entry.top_line), (120, 115));

        // A broken positions file is worth *nothing* — it is re-derived by looking at a file
        // again — so it reads as empty and is deliberately not quarantined. `load` moves a
        // broken workspace aside; that asymmetry is the point.
        fs::write(&path, b"{ not a list").expect("write");
        assert!(load_positions(&path).is_empty());
        assert_eq!(
            dir.entries(),
            ["positions.json"],
            "nothing was moved aside: a stray file is a cleanup the user did not ask for",
        );

        // And a file that was never written is the first launch, not a failure.
        assert!(load_positions(&dir.join("nothing-here.json")).is_empty());
    }

    #[test]
    fn a_file_written_with_a_larger_cap_is_trimmed_on_read() {
        let dir = TempDir::new("positions-oversize");
        let path = dir.join("positions.json");
        let over: Vec<ViewPosition> = (0..(MAX_POSITIONS + 20))
            .map(|n| ViewPosition {
                touched_at: n as u64,
                ..at(&format!("/f{n}.rs"), 1)
            })
            .collect();
        save_positions(&path, &over).expect("save");

        let loaded = load_positions(&path);
        assert_eq!(loaded.len(), MAX_POSITIONS);
        assert!(
            position_for(&loaded, Path::new(&format!("/f{}.rs", MAX_POSITIONS + 19))).is_some(),
            "the trim keeps the newest, so a hand-edited file cannot make every later save \
             carry entries this build would never have kept",
        );
    }

    #[test]
    fn forgetting_reports_whether_it_removed_anything() {
        let mut list = recents(&[("/a", 10)]);

        assert!(forget_recent(&mut list, Path::new("/a")));
        assert!(list.is_empty());
        // The UI's "remove from list" needs to be able to tell a removal from a miss, or a
        // stale menu silently reports success for an entry that was already gone.
        assert!(!forget_recent(&mut list, Path::new("/a")));
    }

    #[test]
    fn recents_round_trip_newest_first_whatever_order_the_file_was_in() {
        let dir = TempDir::new("recents-round-trip");
        let path = dir.join("recent.json");

        // Deliberately written oldest-first: `load_recent` sorts, so no writer has to.
        save_recent(&path, &recents(&[("/a", 10), ("/b", 30), ("/c", 20)])).expect("save");
        let loaded = load_recent(&path);

        assert_eq!(
            loaded.iter().map(|e| e.path.as_path()).collect::<Vec<_>>(),
            [Path::new("/b"), Path::new("/c"), Path::new("/a")],
        );
    }

    #[test]
    fn an_unusable_recents_file_reads_as_no_recents_and_is_left_alone() {
        let dir = TempDir::new("recents-corrupt");
        let path = dir.join("recent.json");
        fs::write(&path, b"{ not a list").expect("write");

        assert!(load_recent(&path).is_empty());
        // Unlike `workspace.json`, nothing is quarantined: the list is rebuilt by the next
        // few opens, and a `recent.corrupt-1.json` would be litter the user has to clean up.
        assert!(path.exists(), "the unusable file was moved aside");
        assert!(load_recent(&dir.join("absent.json")).is_empty());
    }

    /// Which directories a person works in is inference nobody asked to publish, and the
    /// shared `write_atomic` is what makes this true of both files rather than one.
    #[cfg(unix)]
    #[test]
    fn a_recents_file_is_written_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("recents-mode");
        let path = dir.join("recent.json");
        save_recent(&path, &recents(&[("/a", 1)])).expect("save");

        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_display_path_only_abbreviates_a_real_home() {
        let home = Path::new("/home/dev");

        assert_eq!(
            abbreviate(Path::new("/home/dev/work/cide"), Some(home)),
            "~/work/cide"
        );
        assert_eq!(abbreviate(home, Some(home)), "~");
        assert_eq!(abbreviate(Path::new("/opt/src"), Some(home)), "/opt/src");
        // The edge case the function exists to get right: `strip_prefix("")` succeeds for
        // every relative path, so an empty `$HOME` must not abbreviate at all.
        assert_eq!(
            abbreviate(Path::new("work/cide"), Some(Path::new(""))),
            "work/cide"
        );
        assert_eq!(abbreviate(Path::new("work/cide"), None), "work/cide");
    }

    #[test]
    fn the_debouncer_does_not_fire_before_its_delay() {
        let debouncer = Debouncer::new(Duration::from_secs(30));

        assert!(!debouncer.due(), "nothing pending means nothing to write");
        debouncer.note_change();

        assert!(!debouncer.due());
        assert!(!debouncer.take());
    }

    #[test]
    fn the_debouncer_fires_once_its_delay_has_elapsed() {
        let debouncer = Debouncer::new(Duration::from_millis(20));
        debouncer.note_change();
        std::thread::sleep(Duration::from_millis(40));

        assert!(debouncer.due());
        assert!(debouncer.take());
    }

    #[test]
    fn taking_a_due_write_clears_it_until_the_next_change() {
        let debouncer = Debouncer::new(Duration::from_millis(20));
        debouncer.note_change();
        std::thread::sleep(Duration::from_millis(40));

        assert!(debouncer.take());
        assert!(!debouncer.due(), "one change owes exactly one write");
        assert!(!debouncer.take());

        debouncer.note_change();
        std::thread::sleep(Duration::from_millis(40));
        assert!(debouncer.take());
    }

    #[test]
    fn the_deadline_runs_from_the_first_change_of_a_burst() {
        let debouncer = Debouncer::new(Duration::from_millis(30));
        debouncer.note_change();

        // A steady stream of edits must not starve the write forever.
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(10));
            debouncer.note_change();
        }

        assert!(debouncer.take());
    }
}
