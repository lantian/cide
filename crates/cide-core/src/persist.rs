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
        let mut file = File::create(&tmp)?;
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
        Axis, LayoutNode, Pane, PaneKind, PaneRole, PaneTree, Project, ProjectRoot,
        SettingsSection, Tab, TabKind, WindowRole,
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
