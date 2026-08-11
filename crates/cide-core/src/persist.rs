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

use cide_ipc::Workspace;
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
/// A `Value` does not preserve object key order, so a document that passes through here
/// loses the header tab order. That is why [`load`] only routes documents that actually need
/// migrating through this function; the first real migration step will want serde_json's
/// `preserve_order` feature turned on.
pub fn migrate(value: Value) -> Result<Workspace> {
    const CURRENT: u64 = Workspace::CURRENT_SCHEMA as u64;

    let Some(version) = value.get("schemaVersion").and_then(Value::as_u64) else {
        return Err(CoreError::Serde(
            "workspace has no schemaVersion; pre-1 files are not supported".into(),
        ));
    };

    // A ladder: each supported older schema gets an arm that rewrites the document one step
    // forward and re-enters here, e.g. `1 => migrate(v1_to_v2(value))`. Adding version 2 is
    // then an arm, not a restructuring.
    match version {
        CURRENT => Ok(serde_json::from_value(value)?),
        v if v > CURRENT => Err(CoreError::Serde(format!(
            "workspace schema {v} is newer than this build's {CURRENT}; refusing to downgrade it"
        ))),
        v => Err(CoreError::Serde(format!(
            "workspace schema {v} is no longer supported"
        ))),
    }
}

/// Write `ws` to `path` so that a crash leaves either the old file or the new one, never a
/// half-written one.
///
/// Creates parent directories as needed.
pub fn save_atomic(path: &Path, ws: &Workspace) -> Result<()> {
    let dir = parent_dir(path);
    fs::create_dir_all(dir)?;

    let json = serde_json::to_vec_pretty(ws)?;
    // The temp file is a sibling of the target: `rename` is only atomic within a single
    // filesystem, and the system temp directory is routinely a different one.
    let tmp = temp_path(path);

    let write = (|| -> io::Result<()> {
        let mut file = create_private(&tmp)?;
        file.write_all(&json)?;
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
                repo: None,
                label: name.into(),
            }],
            tabs: vec![home, settings],
            active_tab,
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

    /// Criterion 2 of the rows change: nothing on anybody's disk had to move.
    ///
    /// `CURRENT_SCHEMA` is still 1, so this document takes the fast path in `read_workspace`
    /// and is never routed through `serde_json::Value` — which would re-sort the header tab
    /// order. No migration arm runs, no file is rewritten, no quarantine copy appears, and
    /// every anchor comes back naming the same split it named before.
    #[test]
    fn a_workspace_written_before_this_change_loads_at_schema_1_untouched() {
        let dir = TempDir::new("legacy");
        let path = dir.join("workspace.json");
        fs::write(&path, LEGACY_WORKSPACE).expect("write the captured document");

        let ws = load(&path);

        assert_eq!(Workspace::CURRENT_SCHEMA, 1, "no schema bump was needed");
        assert_eq!(ws.schema_version, 1);
        assert_eq!(
            dir.entries(),
            vec!["workspace.json".to_string()],
            "nothing was quarantined and nothing was written beside it"
        );

        let project = ws.projects.values().next().expect("the project loaded");
        assert_eq!(project.tabs.len(), 2);
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
