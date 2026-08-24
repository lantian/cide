//! Project and tab commands.
//!
//! Thin by policy: each handler unwraps its arguments, calls one `cide-core` operation
//! through [`WorkspaceState::update`] — which validates and marks the file dirty — and
//! returns the new revision. The rules about what may be closed or reordered live in the
//! domain, so a second frontend, or a future headless mode, gets them for free.

use std::path::{Path, PathBuf};

use cide_core::CoreError;
use cide_core::{persist, workspace};
use cide_ipc::{
    Pane, PaneId, PaneKind, PaneRole, ProjectId, RecentEntry, RecentProject, TabId, TabKind,
};
use parking_lot::Mutex;
use tauri::{Manager, State};

use crate::workspace_state::WorkspaceState;

/// Returned by every mutation so the caller can order or discard a later snapshot.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutated {
    pub rev: u64,
}

/// Open a project over one or more roots, or activate the one already over them.
///
/// **Synchronous, and that is load-bearing** — see [`open_project_here`], which is the whole of
/// what this does. Anything that wants an open from an `async` command has to go through
/// [`open_project_on_main_thread`]; there is a test below that says so.
#[tauri::command(rename_all = "camelCase")]
pub fn project_open(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    paths: Vec<String>,
    name: Option<String>,
) -> Result<ProjectId, CoreError> {
    let roots: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    open_project_here(&app, &state, roots, name)
}

/// Everything an open does, on the caller's thread — **and the caller must be the main one**.
///
/// # The crash this shape exists to prevent
///
/// This was the body of [`project_open`] and `project_open_recent` called that command function
/// directly. `project_open` is synchronous, so Tauri runs it on the main thread — the GTK one, on
/// Linux — and the body below was written against that fact. `project_open_recent` is `async`, so
/// Tauri runs it as a task on the shared async runtime, on a tokio worker. The same code, two
/// threading contracts, and only one of them true.
///
/// The one that bit is [`crate::ide::IdeServers::ensure`]: it owns a *second* tokio runtime (the
/// `cide-ide` one) and drives the port bind with `rt.block_on`. `Runtime::block_on` panics —
/// "Cannot start a runtime from within a runtime" — when the calling thread is already inside a
/// runtime, which is exactly what a tokio worker is. In a release build `panic = "abort"`, so that
/// panic is not a failed command, it is the process going away: opening a project from the recents
/// dropdown killed the app. (In a debug build it is quieter and worse to diagnose — the task
/// unwinds, the workspace has already been mutated and broadcast, so the project *appears*, and
/// the promise the webview is awaiting simply never settles.)
///
/// `DiagnosticsRegistry::ensure` is the second reason and would have been a slower failure:
/// it forks language servers, and `cide_core::child_env::arm` requires the forking thread to
/// outlive the child. It routes through `on_spawn_thread`, so the fork itself is safe wherever
/// this runs — but that only holds while every spawn site keeps doing so, and a command worker is
/// the thread the whole `on_spawn_thread` mechanism exists because of.
///
/// So there is one open path, it runs where a synchronous command runs, and the two ways in are
/// [`project_open`] (already there) and [`open_project_on_main_thread`] (which hops).
fn open_project_here(
    app: &tauri::AppHandle,
    state: &WorkspaceState,
    roots: Vec<PathBuf>,
    name: Option<String>,
) -> Result<ProjectId, CoreError> {
    let id = state.update(|ws| workspace::open_project(ws, roots, name))?;

    // Started here rather than lazily at first spawn, because the lockfile has to exist
    // before any `claude` in this project looks for one. `open_project` also *activates* an
    // already-open path instead of opening it twice, so `ensure` is by design idempotent.
    if let Some(servers) = app.try_state::<crate::ide::IdeServers>() {
        let roots = state.with(|ws| {
            workspace::project(ws, id)
                .map(|p| p.roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        });
        servers.ensure(app, id, roots);
    }

    // The language servers, on the same trigger and idempotent for the same reason. Started here
    // rather than lazily on the first editor open, because rust-analyzer is 30–120 seconds from
    // launch to its first useful answer on a real workspace — waiting until a file is opened
    // would put that whole delay in front of the user at the moment they most want an answer.
    if let Some(diagnostics) = app.try_state::<crate::lsp::DiagnosticsRegistry>() {
        let roots = state.with(|ws| {
            workspace::project(ws, id)
                .map(|p| p.roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        });
        diagnostics.ensure(app, id, roots);
        // The IDE server started above cannot have found this project's store — it did not exist
        // yet — so the link is completed from this side. See `ide::link_diagnostics`.
        crate::ide::link_diagnostics(app, id);
    }

    // M18: and the project's task tracker, on the same trigger and idempotent for the same
    // reason. Opened here rather than when the Tasks panel is first shown, because the store is
    // what the flusher ticks: a subagent can start writing tasks over the MCP socket long before
    // anybody opens the panel, and a tracker nothing is ticking is a debounce that never fires.
    if let Some(stores) = app.try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>() {
        // Read in its own borrow rather than reusing the `roots` above: this one wants the
        // *first* root, because one project has one tracker. See `TasksStores::ensure`.
        let root = state.with(|ws| {
            workspace::project(ws, id)
                .ok()
                .and_then(|p| p.roots.first().map(|r| r.path.clone()))
        });
        if let Some(root) = root {
            stores.ensure(id, &root);
        }
    }

    // Recorded here rather than in `cide_core::workspace::open_project`, because the domain
    // function is also how a *restore* re-opens everything in `workspace.json` at launch, and
    // a restore must not reorder the list — every launch would otherwise rewrite the recents
    // in project order and the word "recent" would stop meaning anything. This is the command,
    // which is only reached by a user asking for a project.
    //
    // Reads the name back out of the domain rather than using the `name` argument: that
    // argument is `None` for every real open, and the name a user recognises is the one the
    // header shows.
    let named = state.with(|ws| {
        workspace::project(ws, id)
            .ok()
            .and_then(|p| p.roots.first().map(|r| (r.path.clone(), p.name.clone())))
    });
    if let Some((root, name)) = named {
        // Off this thread, and not awaited. This function runs on the main thread — the GTK
        // one, on Linux; see the note above about what enforces that — and `remember` is two
        // `fsync`s and a `rename` behind a lock that `project_recent` can be holding while it
        // stats an unmounted share. Doing it inline would freeze every window in the process for
        // as long as that takes, which is the one thing this list is not worth. It is
        // best-effort by design (see `remember`), so there is no answer to wait for and nothing
        // to report.
        drop(tauri::async_runtime::spawn_blocking(move || {
            remember(&root, &name)
        }));
    }

    // **In `PerProject` mode an open names a window nobody had built.**
    //
    // `workspace::open_project` ends in `rebuild_windows`, which mints a `shell:<uuid>` role for
    // the new project — and until this line nothing turned that role into an OS window. The
    // project existed, held a console tab with a live `claude`, and had no window anywhere: the
    // only reason it was ever visible was that the header rendered every project in the process
    // rather than the ones its own window names, so it appeared as a tab in some *other*
    // window — over content that window does not draw, and inert, because `activate_project`
    // only moves `active` on shells whose `projects` contains the id. Both halves are fixed
    // together; see `ui/src/windows/windowTabs.ts`.
    //
    // A no-op in `Stacked`: the one shell already exists, `reconcile` skips it, and nothing is
    // doomed because the open added a role rather than dropping one. `project_close` and
    // `close_tab` below call it for the mirror-image reason.
    //
    // Reported rather than logged, exactly as `window_set_mode` does: at this point the domain
    // has already committed the open, so a caller that sees this error still has the project —
    // what it does not have is the window, which is worth saying out loud.
    crate::cmd::window::reconcile(app, state)?;
    Ok(id)
}

// --- recent projects ------------------------------------------------------------------------

/// Serialises every read-modify-write of `recent.json`.
///
/// A process-wide lock rather than managed Tauri state: this is one small file with no
/// in-memory mirror, so there is no state to register and nothing for a second window to get a
/// stale copy of. Without it two windows opening projects at the same moment would each load
/// the old list, prepend their own entry and publish it — and the second write, being atomic,
/// would cleanly and completely erase the first.
static RECENT_LOCK: Mutex<()> = Mutex::new(());

/// Add a project to the recents file. Best-effort by design.
///
/// A failure here must never fail the open: the user asked for a project, not for a menu entry,
/// and refusing to open a directory because a convenience list could not be written would be an
/// absurd trade. It is logged, because a recents list that silently never grows is otherwise
/// indistinguishable from one nobody wired up.
fn remember(root: &Path, name: &str) {
    let _guard = RECENT_LOCK.lock();
    let path = persist::recent_path();
    let mut list = persist::load_recent(&path);
    persist::remember_recent(&mut list, root, name, persist::now_ms());
    if let Err(error) = persist::save_recent(&path, &list) {
        tracing::warn!(path = %path.display(), %error, "could not record the recent project");
    }
}

/// Pair each remembered project with whether its directory is still there.
///
/// One `is_dir` per entry, capped at [`persist::MAX_RECENT`] — sixteen stats, on the gesture
/// that opens a menu. `is_dir` rather than `exists`: a file where a project used to be is not
/// something `open_project` can do anything with, and reporting it as openable would move the
/// failure to a place with no way to explain itself.
///
/// **Call this with [`RECENT_LOCK`] released.** The lock exists to serialise read-modify-write
/// of one small file, which is microseconds; a `stat` on an unmounted share is an NFS timeout.
/// Holding the lock across these would turn a menu nobody is watching into a stall on every
/// other caller of the file, `project_open` included.
fn with_existence(list: Vec<RecentProject>) -> Vec<RecentEntry> {
    list.into_iter()
        .map(|project| RecentEntry {
            exists: project.path.is_dir(),
            project,
        })
        .collect()
}

/// The recent-projects list, most recent first.
///
/// Async over `spawn_blocking` because it reads a file and stats up to sixteen directories,
/// and one of those directories can be an unmounted network share — a `stat` that takes the
/// NFS timeout on the IPC thread would freeze every window, not just the menu that asked.
#[tauri::command(rename_all = "camelCase")]
pub async fn project_recent() -> Result<Vec<RecentEntry>, CoreError> {
    tauri::async_runtime::spawn_blocking(|| {
        // The lock covers the read and is dropped before the stats — see `with_existence`.
        let list = {
            let _guard = RECENT_LOCK.lock();
            persist::load_recent(&persist::recent_path())
        };
        with_existence(list)
    })
    .await
    .map_err(|e| CoreError::Io(format!("recent projects: {e}")))
}

/// Drop one entry, or every entry, and answer with what is left.
///
/// `path` of `None` clears the list. One command rather than two because they are one
/// read-modify-write apart and the frontend renders both answers the same way; splitting them
/// would mean two chances to forget the lock above.
///
/// Answers with the new list rather than a count, so the menu that asked can repaint from the
/// truth instead of patching its own copy — the same argument the workspace mirror makes.
#[tauri::command(rename_all = "camelCase")]
pub async fn project_forget_recent(path: Option<String>) -> Result<Vec<RecentEntry>, CoreError> {
    tauri::async_runtime::spawn_blocking(move || {
        // Read-modify-write under the lock; the stats below it are not — see `with_existence`.
        let list = {
            let _guard = RECENT_LOCK.lock();
            let file = persist::recent_path();
            let mut list = persist::load_recent(&file);
            let changed = match &path {
                Some(path) => persist::forget_recent(&mut list, Path::new(path)),
                None => {
                    let had = !list.is_empty();
                    list.clear();
                    had
                }
            };
            // Nothing to write when nothing moved: the common case is a menu acting on an entry
            // that a second window already removed, and rewriting an identical file would churn
            // the inode for no change anyone can observe.
            if changed && let Err(error) = persist::save_recent(&file, &list) {
                tracing::warn!(path = %file.display(), %error, "could not update the recent projects");
            }
            list
        };
        with_existence(list)
    })
    .await
    .map_err(|e| CoreError::Io(format!("recent projects: {e}")))
}

/// Reopen a remembered project, refusing an entry whose directory has gone.
///
/// The refusal is the whole reason this is not just `project_open`. `open_project` will happily
/// create a project over a path that does not exist — it never touches the filesystem — so a
/// stale recents entry would open a project with an empty tree, an explorer showing nothing and
/// a `claude` spawned into a directory that is not there. Every one of those symptoms is
/// several steps away from the cause. Checked here rather than in `project_open` because that
/// command is also how a test and a restore open a project, and neither should have to have a
/// directory on disk.
///
/// It is `async` **for the stat and for nothing else**. That is what forces the hop back through
/// [`open_project_on_main_thread`] at the end: this task runs on a tokio worker, and an open run
/// there aborted the process. [`open_project_here`] has the mechanism.
#[tauri::command(rename_all = "camelCase")]
pub async fn project_open_recent(
    app: tauri::AppHandle,
    path: String,
) -> Result<ProjectId, CoreError> {
    let root = PathBuf::from(&path);
    let exists = tauri::async_runtime::spawn_blocking({
        let root = root.clone();
        move || root.is_dir()
    })
    .await
    .map_err(|e| CoreError::Io(format!("{path}: {e}")))?;

    if !exists {
        return Err(CoreError::Io(format!(
            "{} is no longer a directory — it may have been moved, deleted or unmounted",
            root.display()
        )));
    }

    // Through the one open path rather than the domain function, so the IDE server, the language
    // servers and the recents entry are handled exactly once — and **on the main thread**, which
    // this task is not on. See [`open_project_here`] for the crash that came of assuming it was.
    open_project_on_main_thread(app, vec![root], None).await
}

/// Run an open on the thread a synchronous command would have run it on, and wait for the answer.
///
/// The bridge every `async` command has to cross to reach [`open_project_here`]. It exists
/// because the two halves of "open a project from the recents list" have opposite requirements:
/// the `is_dir` above must **not** run on the main thread (an unmounted share turns it into an
/// NFS timeout that freezes every window, which is the same reason [`project_recent`] stats off
/// it), and the open must **only** run there.
///
/// `run_on_main_thread` returns as soon as the closure is queued, so the answer comes back over a
/// channel. `spawn_blocking` for the `recv` rather than blocking this task: the closure runs when
/// the GTK main loop next turns, and holding an async worker on a blocking `recv` in the meantime
/// is how a runtime with N workers ends up with N-1.
///
/// A `RecvError` means the closure was dropped without answering — the main loop is gone, i.e. the
/// app is on its way out. Reported rather than swallowed, because "the project did not open" with
/// no message is precisely the failure this file keeps finding.
async fn open_project_on_main_thread(
    app: tauri::AppHandle,
    roots: Vec<PathBuf>,
    name: Option<String>,
) -> Result<ProjectId, CoreError> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<ProjectId, CoreError>>();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let state = handle.state::<WorkspaceState>();
        // The receiver is gone only if this task was cancelled, in which case nobody is waiting
        // for the id — but the project is open either way, which is what the user asked for.
        let _ = tx.send(open_project_here(&handle, &state, roots, name));
    })
    .map_err(|e| CoreError::Io(format!("could not open the project: {e}")))?;

    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| CoreError::Io(format!("could not open the project: {e}")))?
        .map_err(|_| CoreError::Io("the application is shutting down".into()))?
}

/// Show a project's primary root in the desktop's file manager.
///
/// From Rust, and for the same reason `app_open_log_dir` is: the `opener` plugin's JS command
/// is capability-gated per window and a detached window deliberately has none, so doing it here
/// makes the gesture work from every window without widening what a webview may open to
/// "any path it can name".
#[tauri::command(rename_all = "camelCase")]
pub async fn project_reveal(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<String, CoreError> {
    let root = state.with(|ws| {
        workspace::project(ws, project).map(|p| p.roots.first().map(|r| r.path.clone()))
    })?;
    let root = root.ok_or(CoreError::NoRoots)?;

    // Both halves off the runtime's worker pool, for the same reason `project_recent` is: the
    // `stat` can be an NFS timeout on an unmounted root, and `open_path` forks a file manager,
    // which on a cold desktop is not instant either. The whole answer is one blocking task.
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_opener::OpenerExt;

        // Asking a file manager to open a path that is not there is an error dialog with no
        // explanation in it — the same trap `app_open_log_dir` sidesteps by creating the
        // directory. Here it must not be created: a project root that has gone is news, not a
        // hole to fill.
        if !root.is_dir() {
            return Err(CoreError::Io(format!("{} is not there", root.display())));
        }
        app.opener()
            .open_path(root.to_string_lossy(), None::<&str>)
            .map_err(|e| CoreError::Io(format!("could not open {}: {e}", root.display())))?;
        Ok(root.display().to_string())
    })
    .await
    .map_err(|e| CoreError::Io(format!("reveal: {e}")))?
}

// --- the folder picker ----------------------------------------------------------------------

/// Ask the user for one or more project folders. Empty means cancelled.
///
/// **This exists because of a stacking bug, and the bug is not ours.** The report was that the
/// picker opens *behind* the cide window on KDE Wayland, where a dialog with no parent is
/// stacked wherever the compositor likes. Following it back:
///
/// * The frontend called `@tauri-apps/plugin-dialog`'s `open()`, which lands on the plugin's
///   `open` command. That command does `set_parent(&window)` — but inside
///   `#[cfg(any(windows, target_os = "macos"))]`. On Linux the parent is *never* set, and no
///   option the JS API accepts can set it.
/// * Reaching for the plugin's Rust API instead does not help either. It is `rfd` underneath,
///   and `tauri-plugin-dialog`'s default feature is `gtk3`, so `rfd` builds its GTK3 backend —
///   in which the word "parent" does not appear at all. `set_parent` compiles, stores a handle
///   and is then dropped on the floor. (`rfd`'s *portal* backend does honour it, via ashpd's
///   `WindowIdentifier`; switching to it means `default-features = false, features =
///   ["xdg-portal"]` on the plugin, which drags in ashpd/zbus and makes the picker depend on a
///   portal service being installed. That was the alternative, and it lost on both counts.)
///
/// So the dialog is built here, against the window's real `GtkApplicationWindow`.
/// `FileChooserNative` is the same class GTK applications use: with a portal installed it *is*
/// the desktop's own picker (KDE's, on the reporter's machine), and without one it falls back
/// to GTK's — and it sets a transient parent in both cases, which is the relation Wayland
/// needs to keep a dialog above the window that owns it.
///
/// **Unverified on screen, and it cannot be verified here**: confirming a stacking order needs
/// a running GUI, and launching one is forbidden in this environment. What is checked is that
/// it compiles, that the parent is a real handle rather than `None` when the window has one,
/// and the mechanism above, which was read out of the two dependencies' sources rather than
/// assumed.
#[tauri::command(rename_all = "camelCase")]
pub async fn project_pick(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Vec<String>, CoreError> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    show_picker(
        &app,
        window,
        PickerSpec {
            title: "Open project",
            accept: "Open",
            folders: true,
            // A project may span several roots, and `project_open` already takes a list.
            multiple: true,
            filter: None,
        },
        tx,
    )?;

    // `spawn_blocking` rather than blocking the command's own task: the answer arrives only
    // when the user has finished browsing, which is unbounded, and the async runtime's worker
    // pool is shared with every other command in flight.
    //
    // A `recv` error means the sender was dropped without answering — the dialog was destroyed
    // with the window, say. Cancelled and never-answered both mean "no project", which is why
    // the empty vector covers them rather than an error the caller would have to invent a
    // message for.
    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or_default())
        .await
        .map_err(|e| CoreError::Io(format!("folder picker: {e}")))?;

    Ok(picked
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// What a picker asks for. One type across both `cfg` arms, so a second caller cannot pick up
/// the Linux behaviour and quietly lose the other.
///
/// `filter` is `(label, globs)` — glob form, because that is what GTK's `FileFilter` takes and
/// because the plugin's bare-extension form can be derived from it and not the other way round.
pub(crate) struct PickerSpec {
    pub title: &'static str,
    /// The confirm button's label. GTK only; the plugin has no equivalent.
    pub accept: &'static str,
    pub folders: bool,
    pub multiple: bool,
    pub filter: Option<(&'static str, &'static [&'static str])>,
}

/// Build and show the picker on the GTK main thread, answering through `tx`.
///
/// Returns as soon as the dialog is *queued*, not when it is answered: `FileChooserNative::run`
/// spins a nested main loop, which would freeze every webview in the process — including the
/// one that asked — for as long as the user browses. The response signal keeps the app alive
/// behind the dialog.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
pub(crate) fn show_picker(
    app: &tauri::AppHandle,
    window: tauri::WebviewWindow,
    spec: PickerSpec,
    tx: std::sync::mpsc::Sender<Vec<PathBuf>>,
) -> Result<(), CoreError> {
    app.run_on_main_thread(move || {
        use gtk::prelude::*;

        // `None` is survivable and is not the same as a failure: the dialog still opens, just
        // unparented — which is the state being fixed, so it is worth a line in the log rather
        // than a silent regression if `gtk_window` ever starts failing.
        let parent = match window.gtk_window() {
            Ok(parent) => Some(parent),
            Err(error) => {
                tracing::warn!(%error, "no GTK window to parent the picker to");
                None
            }
        };

        let dialog = gtk::FileChooserNative::new(
            Some(spec.title),
            parent.as_ref(),
            if spec.folders {
                gtk::FileChooserAction::SelectFolder
            } else {
                gtk::FileChooserAction::Open
            },
            Some(spec.accept),
            Some("Cancel"),
        );
        // Modal as well as parented. The parent decides *stacking*; modality is what stops the
        // user reaching the window underneath and opening a second picker on top of this one.
        dialog.set_modal(true);
        dialog.set_select_multiple(spec.multiple);
        if let Some((name, patterns)) = spec.filter {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(name));
            for pattern in patterns {
                filter.add_pattern(pattern);
            }
            dialog.add_filter(filter);
            // A second, unrestricted filter rather than none: a theme saved as `.jsonc`, or
            // with no extension at all, is a file the importer handles perfectly well, and a
            // picker that cannot select it is a dead end with no error message.
            let any = gtk::FileFilter::new();
            any.set_name(Some("All files"));
            any.add_pattern("*");
            dialog.add_filter(any);
        }

        /*
         * One reference, held by the handler and released by it.
         *
         * `show()` returns immediately, so nothing on this stack can keep the dialog alive
         * until the user answers — dropping it here would destroy the window that was just
         * put on screen. Parking a clone in the handler works because the handler is owned by
         * the dialog; taking it out again inside the handler is what stops that cycle becoming
         * a leak of one dialog per pick. Dropping our reference from inside the handler is
         * safe: GObject holds its own reference for the duration of a signal emission, so the
         * instance cannot be finalised under the handler's feet.
         */
        let held = std::rc::Rc::new(std::cell::RefCell::new(Some(dialog.clone())));
        dialog.connect_response(move |dialog, response| {
            let picked = if response == gtk::ResponseType::Accept {
                dialog.filenames()
            } else {
                Vec::new()
            };
            dialog.hide();
            // The receiver is gone if the command's task was cancelled. Nothing to do about
            // it, and nothing lost: the user's answer had nowhere to go anyway.
            let _ = tx.send(picked);
            held.borrow_mut().take();
        });
        dialog.show();
    })
    .map_err(|e| CoreError::Io(format!("{}: {e}", spec.title)))
}

/// Windows and macOS, where the plugin parents the dialog itself and `rfd`'s backend honours
/// it. cide is a Linux app; this arm exists so the crate still compiles elsewhere, and it is
/// the shape the whole command would have if the Linux gap above were ever closed upstream.
#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
pub(crate) fn show_picker(
    _app: &tauri::AppHandle,
    window: tauri::WebviewWindow,
    spec: PickerSpec,
    tx: std::sync::mpsc::Sender<Vec<PathBuf>>,
) -> Result<(), CoreError> {
    use tauri_plugin_dialog::DialogExt;

    let answer = move |paths: Option<Vec<tauri_plugin_dialog::FilePath>>| {
        let picked = paths
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .collect();
        let _ = tx.send(picked);
    };
    let mut builder = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(spec.title);
    if let Some((name, patterns)) = spec.filter {
        // The plugin wants bare extensions where GTK wants globs, which is why `PickerSpec`
        // carries the glob form: one of the two has to convert, and stripping `*.` is total
        // where synthesising a glob from an extension is not.
        let bare: Vec<&str> = patterns
            .iter()
            .map(|p| p.trim_start_matches("*."))
            .collect();
        builder = builder.add_filter(name, &bare);
    }
    if spec.folders {
        builder.pick_folders(answer);
    } else {
        builder.pick_files(answer);
    }
    Ok(())
}

/// Close a project, its tabs and its windows.
///
/// `force` is the caller's statement that the user has been shown, and accepted, whatever
/// unsaved edits the project holds. Without it a dirty file tab anywhere in the project is
/// [`CoreError::UnsavedChanges`] — see `cide_core::workspace::close_project`.
#[tauri::command(rename_all = "camelCase")]
pub fn project_close(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    force: bool,
) -> Result<Mutated, CoreError> {
    // Read before the mutation: once the project is gone the diff tabs are gone with it, and
    // with them the only record of which agent turns are still blocked waiting on them.
    let blocked = app
        .try_state::<crate::ide::IdeServers>()
        .map(|_| crate::ide::pending_request_ids(&state, project))
        .unwrap_or_default();

    let out = state.update(|ws| {
        workspace::close_project(ws, project, force)?;
        Ok(Mutated { rev: ws.rev })
    })?;

    if let Some(servers) = app.try_state::<crate::ide::IdeServers>() {
        // Reject first, stop second. `stop` would cancel these anyway, but doing it here
        // records the accurate reason — the project closed — rather than reporting a
        // shutdown to an agent whose editor is still running.
        crate::ide::cancel_for_tabs(
            &servers,
            project,
            blocked,
            cide_ide_mcp::CancelReason::ProjectClosed,
        );
        servers.stop(project);
    }

    // And its language servers. Dropping the entry runs each one's shutdown ladder, which is
    // where a `gopls` writes the cache that keeps the *next* open fast — so a project closed and
    // reopened does not re-index from nothing.
    if let Some(diagnostics) = app.try_state::<crate::lsp::DiagnosticsRegistry>() {
        diagnostics.close(project);
    }

    // And its task tracker, which is written on the way out rather than merely dropped: a task
    // created a moment before the project was closed is inside the 500 ms debounce, and a store
    // that goes away without a final write takes it with it. `close` does both — see
    // `TasksStores::close` for why the merged board is logged rather than broadcast.
    if let Some(stores) = app.try_state::<std::sync::Arc<crate::tasks_state::TasksStores>>() {
        stores.close(project);
    }

    // And its Ctrl+Shift+T records. They name a `ProjectId` nothing can resolve any more, and
    // reopening the same directory mints a *new* id — so left here they would be unreachable
    // rather than merely stale, sitting in a 16-deep stack and pushing live records out of it.
    app.state::<crate::closed_tabs::ClosedTabs>()
        .forget_project(project);

    // Closing a project drops the window roles that showed parts of it. Those windows are
    // still on screen until something takes them down, and a detached one would sit there
    // blank with an unreachable child behind it.
    crate::cmd::window::reconcile(&app, &state)?;
    Ok(out)
}

/// Bring a project to the front of the window that holds it.
///
/// The header draws a tab per open project and its `onActivate` had nowhere to go: the
/// domain has had `activate_project` since projects were multi-window, and no command ever
/// exposed it. So with two projects open, clicking the other one did nothing — the same
/// dead-control failure the sidebar's search button had.
///
/// Idempotent, and cheap: activating the project that is already active still bumps `rev`
/// through `update`, which is what makes a second window follow along.
///
/// It also **ensures the project's IDE server**, which is the other half of the cap that
/// `ide::servable_projects` applies at launch. That cap binds at most 32 ports, ordered so a
/// project some window is showing always makes the cut — but a workspace larger than the cap
/// still has projects with no server, and switching to one is exactly how a user reaches it.
/// Without this, such a project's panes spawn with no `CLAUDE_CODE_SSE_PORT`: no `openDiff`,
/// no selection, no `@`-mentions, and no error anywhere, because a `claude` with no IDE looks
/// identical to a `claude` whose IDE never started. `ensure` is idempotent, so for the
/// overwhelmingly common under-the-cap case this is a `DashMap` hit and nothing else.
#[tauri::command(rename_all = "camelCase")]
pub fn project_activate(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Mutated, CoreError> {
    let out = state.update(|ws| {
        // Resolved first so an unknown id is an error rather than a silent no-op — the
        // frontend passes an id it read from the tree, so a miss means they have diverged.
        workspace::project(ws, project)?;
        workspace::activate_project(ws, project);
        Ok(Mutated { rev: ws.rev })
    })?;

    // After the mutation, and reading the roots in a separate borrow: `ensure` blocks on a
    // runtime to bind a port, and holding the workspace lock across that would stall every
    // other command behind it — the same reason `setup` snapshots before `PendingIdeServers::install`.
    if let Some(servers) = app.try_state::<crate::ide::IdeServers>() {
        let roots = state.with(|ws| {
            workspace::project(ws, project)
                .map(|p| p.roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        });
        servers.ensure(&app, project, roots);
    }
    Ok(out)
}

#[tauri::command(rename_all = "camelCase")]
pub fn project_reorder(
    state: State<'_, WorkspaceState>,
    from: usize,
    to: usize,
) -> Result<Mutated, CoreError> {
    state.update(|ws| {
        workspace::reorder_project(ws, from, to)?;
        Ok(Mutated { rev: ws.rev })
    })
}

/// Open a closable full-screen Claude tab.
///
/// The second half of the product's headline feature: the pinned console is the project's
/// one conversation, and this is where a user starts others. Splitting it creates *new*
/// sessions rather than shells — the only behavioural difference between the two Claude
/// tabs, and it lives in `cmd::pane::default_intent`.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_new_claude(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    title: Option<String>,
) -> Result<TabId, CoreError> {
    state.update(|ws| {
        let name = workspace::project(ws, project)?.name.clone();
        let title = title.unwrap_or_else(|| "Claude".to_string());
        workspace::open_tab(
            ws,
            project,
            TabKind::ClaudeFull { title },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Claude,
                // Every pane in a ClaudeFull tab is closable; closing the last closes the
                // tab. Only the pinned console has a pane that cannot go.
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                title: format!("{name} : claude"),
            },
        )
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn tab_activate(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
) -> Result<Mutated, CoreError> {
    state.update(|ws| {
        workspace::activate_tab(ws, project, tab)?;
        Ok(Mutated { rev: ws.rev })
    })
}

/// Close a tab.
///
/// Two refusals come back from the domain, and both are enforcement rather than courtesy:
///
/// * [`CoreError::TabPinned`] for the project console. The frontend also declines to draw a
///   close button on it, but that is the courtesy — this is why no UI bug can lose it.
/// * [`CoreError::UnsavedChanges`] for a file tab with unsaved edits, unless `force`. The
///   frontend answers that by showing `CloseConfirm` naming the file, and calls again with
///   `force: true` if the user chooses to discard. A `×` on a tab is one click away from a
///   lost afternoon, and the only thing standing between the two is this refusal — the
///   dialog is not, because a dialog that fails to render fails open.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_close(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    force: bool,
) -> Result<Mutated, CoreError> {
    // Closing a diff tab **rejects** it, and this is a deliberate divergence worth naming.
    //
    // The protocol's `TAB_CLOSED` means accepted-as-proposed — verified against the real CLI,
    // which writes the model's version on receiving it — and that is what VS Code sends when
    // its diff tab is closed. cide does not, because the two gestures are not the same thing
    // here: the diff pane carries explicit Reject / Accept as proposed / Accept controls, so
    // a user who means to accept has a button that says so. A `×` on a tab reads as dismiss,
    // and resolving a dismissal as acceptance would write a file change on a gesture nobody
    // makes with that intent. Rejection is recoverable; a write is not.
    let dismissed = crate::ide::request_id_for_tab(&state, project, tab);

    // Read *before* the close, because after it there is nothing left to read. `with` rather
    // than a second `update`, so the record describes the tab as it stood one instant before it
    // went — the tree included, with its pane ids and session bindings.
    let record = state.with(|ws| closing_record(ws, project, tab));

    let out = state.update(|ws| {
        workspace::close_tab(ws, project, tab, force)?;
        Ok(Mutated { rev: ws.rev })
    })?;

    // Pushed only once the close has actually happened. `close_tab` refuses a pinned console and
    // an unsaved buffer, and a record pushed before the refusal would let Ctrl+Shift+T open a
    // second copy of a tab that never went anywhere.
    if let Some(record) = record {
        app.state::<crate::closed_tabs::ClosedTabs>().push(record);
    }

    if let Some(request_id) = dismissed
        && let Some(servers) = app.try_state::<crate::ide::IdeServers>()
    {
        crate::ide::resolve_or_cancel(&servers, project, &request_id, None);
    }

    // Same reason as `project_close`: a tab can own detached windows, and their roles have
    // just been pruned.
    crate::cmd::window::reconcile(&app, &state)?;
    Ok(out)
}

/// Move a tab within its strip — the drop half of a tab drag.
///
/// # The domain rule has existed since M4 and nothing could reach it
///
/// `cide_core::workspace::reorder_tab` is complete, invariant-preserving and covered by three
/// tests, and until this command there was **no `#[tauri::command]` that called it**: the whole
/// feature was implemented and reachable from nothing, which is this project's signature defect
/// and the eighteenth recorded instance. (`project_reorder` beside it is the nineteenth and is
/// still in that state — registered, exposed at `ui/src/ipc/client.ts:165`, and called by no
/// line of TypeScript. It is named in `README.md` rather than fixed here.) The lesson this
/// batch takes from it: a domain function with no command is not "half done", it is *absent*,
/// and its tests passing says nothing about whether a user can do the thing.
///
/// # Ids on the wire, indices in the lock
///
/// `reorder_tab` takes indices, and this command deliberately does not. The drop index is
/// computed in the webview at pointer-move time, off a `Tab[]` that arrived in some earlier
/// snapshot; between that move and the `pointerup` that commits it, an agent's `openDiff`, a
/// ctrl+click or another window's close can insert or remove a tab. An index-based wire call
/// would then move a tab the user was not dragging — silently, and into a position they did not
/// aim at. Ids name the thing itself, so the worst a stale drag can do is fail to find it.
///
/// Both ids are resolved **inside** `state.update`'s closure, where the workspace lock is held,
/// so the resolution and the mutation cannot be separated by another command. Resolving them in
/// the frontend and sending numbers is the same bug one layer up.
///
/// `before` is the tab the dragged one lands *in front of*; `None` means the end of the strip.
/// A boundary rather than a destination index, because "insert before this tab" is stable under
/// the removal that precedes the insert, whereas an index means one thing before the tab is
/// lifted out and another after it — the off-by-one that every hand-rolled reorder ships once.
///
/// Two refusals come back from the domain, and the frontend is built so a user cannot reach
/// either: [`CoreError::TabPinned`] for moving the console or dropping anything ahead of it
/// (`tabDrag.ts` refuses the grab and clamps the caret to boundary 1), and
/// [`CoreError::IndexOutOfRange`] for an id that is no longer in the strip. They are enforcement
/// regardless — the same division of labour `tab_close` documents.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_reorder(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    before: Option<TabId>,
) -> Result<Mutated, CoreError> {
    state.update(|ws| {
        let (from, to) = reorder_target(&workspace::project(ws, project)?.tabs, tab, before)?;
        workspace::reorder_tab(ws, project, from, to)?;
        Ok(Mutated { rev: ws.rev })
    })
}

/// Resolve (dragged tab, drop boundary) to [`workspace::reorder_tab`]'s (from, to) indices.
///
/// A free function over a slice rather than four lines inside the closure above, because it is
/// **arithmetic with an off-by-one in it** and a rule that can only be reached through a
/// `State<WorkspaceState>` is a rule that gets tested at the level of "does the app start". This
/// project has paid for that six times over; the tests below drive it directly.
///
/// `before` is a *boundary* — the tab the dragged one lands in front of, `None` for the end of
/// the strip — and the conversion to a destination index is the whole subtlety. `reorder_tab`
/// removes `from` and then inserts at `to`, so `to` addresses the list **with the dragged tab
/// already lifted out**: every position after `from` has shifted down by one. A boundary to the
/// right of `from` therefore loses one; a boundary at or to the left of it does not.
///
/// Get it wrong in the obvious way — pass the boundary through unchanged — and dragging a tab
/// one place to the right puts it back exactly where it started. The gesture then reads as
/// "reordering does not work" for the single most common drag there is, while every leftward drag
/// behaves perfectly, which is the sort of half-working that survives a manual test.
fn reorder_target(
    tabs: &[cide_ipc::Tab],
    tab: TabId,
    before: Option<TabId>,
) -> Result<(usize, usize), CoreError> {
    let from = tabs
        .iter()
        .position(|t| t.id == tab)
        .ok_or(CoreError::NoSuchTab(tab))?;
    let boundary = match before {
        Some(id) => tabs
            .iter()
            .position(|t| t.id == id)
            .ok_or(CoreError::NoSuchTab(id))?,
        None => tabs.len(),
    };
    Ok((
        from,
        if boundary > from {
            boundary - 1
        } else {
            boundary
        },
    ))
}

/// What Ctrl+Shift+T would need to bring `tab` back, read off the workspace before it closes.
///
/// A free function over `&Workspace` rather than three lines inside [`tab_close`], for the
/// reason `cmd::file::open_git_diff` gives about itself: a policy reachable only through a
/// `State<WorkspaceState>` is a policy that gets tested at the level of "does the app start",
/// and the `dirty` rule below is exactly the kind that is individually obvious and wrong in
/// combination.
///
/// `dirty` is normalised to `false` here rather than in `closed_tabs`, because this is the
/// moment the fact becomes true: the buffer does not survive the close — a dirty tab only got
/// this far because the user chose to discard — so a reopened tab is showing what is on disk.
/// `persist::load` clears the same flag on restore for the same reason, and a record that came
/// back dirty would refuse its own next close over edits nobody made.
///
/// `None` for a tab or project that is not there, which the caller reads as "nothing to
/// remember" rather than as an error: `close_tab` is about to fail on the same lookup and its
/// refusal is the one the user should see.
fn closing_record(
    ws: &cide_ipc::Workspace,
    project: ProjectId,
    tab: TabId,
) -> Option<crate::closed_tabs::ClosedTab> {
    let p = workspace::project(ws, project).ok()?;
    let index = p.tabs.iter().position(|t| t.id == tab)?;
    let t = &p.tabs[index];
    let kind = match &t.kind {
        TabKind::File { path, .. } => TabKind::File {
            path: path.clone(),
            dirty: false,
        },
        other => other.clone(),
    };
    Some(crate::closed_tabs::ClosedTab {
        project,
        kind,
        index,
        tree: t.tree.clone(),
    })
}

/// Put back the last tab this project closed. Ctrl+Shift+T.
///
/// Answers `Ok(None)` when the stack has nothing for this project, which the frontend reports
/// through `unmet` — the shape `keys/dispatch.ts` already uses for a precondition that failed.
/// An `Err` would land on the failure toast, and "there is nothing to reopen" is not a failure.
///
/// # Every press does one visible thing, or says why it did none
///
/// A record can be stale in two ways by the time it is popped, and the rule for each is chosen
/// so that the key is never a key that silently does nothing.
///
/// * **The file was deleted since.** `tab_open_file` does not stat, so the tab would open and
///   the editor would land in its `load: 'failed'` state — honest, but not what "reopen" means.
///   This stats first, the same test `terminal_open_path` performs, and the record is **dropped
///   and the next one tried**. Leaving it instead would make Ctrl+Shift+T dead for ever after
///   one deleted file, which is exactly the failure mode this project keeps finding.
/// * **It is already open**, because the picker, a ctrl+click or the tree got there first. Then
///   it depends on *which* tab it is. Not the active one: it is **activated, and that is the
///   press** — the user asked to be shown that tab and now they are looking at it, which is a
///   real outcome and not a consolation prize. Already the active one: there is nothing at all
///   to show, so the record is dropped like a deleted file's and the loop goes on.
///
/// [`same_tab`] is deliberately the same question each opener asks — a path for a file, a
/// singleton for Settings, a repo-and-path pair for a git diff — because a reopen that answered
/// it its own way is how two tabs over one file come back.
///
/// A `while let` over [`crate::closed_tabs::ClosedTabs::pop`] rather than a peek-then-commit:
/// the stack is a `Mutex<Vec<_>>`, popping under one lock per iteration is simpler, and a record
/// this call has decided against is one the *next* call must not see again.
#[tauri::command(rename_all = "camelCase")]
pub fn tab_reopen_closed(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Option<TabId>, CoreError> {
    let stack = app.state::<crate::closed_tabs::ClosedTabs>();

    while let Some(record) = stack.pop(project) {
        match state.with(|ws| reopen_plan(ws, &record)) {
            Reopen::Skip => continue,
            Reopen::Show(id) => {
                state.update(|ws| workspace::activate_tab(ws, project, id))?;
                return Ok(Some(id));
            }
            Reopen::Reinsert => {
                let id = state.update(|ws| {
                    workspace::reinsert_tab(ws, project, record.index, record.kind, record.tree)
                })?;
                return Ok(Some(id));
            }
        }
    }

    Ok(None)
}

/// What [`tab_reopen_closed`] should do with one record. See that function for the reasoning.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Reopen {
    /// Put the tab back where it was.
    Reinsert,
    /// It is open already and is not the tab on screen; show it, and that is the press.
    Show(TabId),
    /// Nothing this record could do would be visible. Drop it and try the next one.
    Skip,
}

/// The three-way decision, as a function of the workspace and the record alone.
///
/// A free function rather than three `if`s inside the command, because this is the rule with the
/// cases in it and a rule that can only be reached through a `State<WorkspaceState>` is a rule
/// that gets tested at the level of "does the app start". Every branch below is one the user
/// reaches by ordinary means — deleting a file in another editor, opening it again from Ctrl+P,
/// pressing the key twice — and the wrong answer to any of them is a key that appears broken.
///
/// The `is_file` stat is inside rather than lifted out, so the whole decision is one call: split
/// across the caller and here, "deleted" and "already open" would be two rules that have to be
/// read together to know what a press does.
pub(crate) fn reopen_plan(
    ws: &cide_ipc::Workspace,
    record: &crate::closed_tabs::ClosedTab,
) -> Reopen {
    // A file that is no longer there. `tab_open_file` does not stat, so without this the tab
    // opens and the editor lands in `load: 'failed'` — honest, and not what "reopen" means.
    if let TabKind::File { path, .. } = &record.kind
        && !path.is_file()
    {
        return Reopen::Skip;
    }

    let Ok(p) = workspace::project(ws, record.project) else {
        // The project closed between the push and the pop. `forget_project` prunes these, so
        // this is the race rather than the ordinary case — and skipping is right either way.
        return Reopen::Skip;
    };
    match p.tabs.iter().find(|t| same_tab(&t.kind, &record.kind)) {
        // Already open and already on screen: there is nothing a press could show.
        Some(open) if open.id == p.active_tab => Reopen::Skip,
        Some(open) => Reopen::Show(open.id),
        None => Reopen::Reinsert,
    }
}

/// Whether `open` is the tab a reopen of `wanted` would produce, so it can be activated instead.
///
/// Mirrors the dedupe each opener already performs rather than inventing a fourth rule:
/// `cmd::file::open_file_tab` matches a `File` on its path, `cmd::settings::tab_open_settings`
/// treats Settings as a per-project singleton, and `cmd::file::open_git_diff` matches a `Git`
/// diff on its repo and path (never on `side`, which the pane is free to switch while open).
///
/// A `ClaudeFull` tab is **never** the same as another: two of them differ only by title, they
/// hold different conversations, and the record carries the session bindings that say which.
fn same_tab(open: &TabKind, wanted: &TabKind) -> bool {
    match (open, wanted) {
        (TabKind::File { path: a, .. }, TabKind::File { path: b, .. }) => a == b,
        (TabKind::Settings { .. }, TabKind::Settings { .. }) => true,
        // An extension page is a singleton *per extension*, not per project: two of them are two
        // different READMEs, and a user comparing SQL against YAML wants both open. The `name` is
        // deliberately not part of the identity — it is a caption, and an extension that renamed
        // itself between the tab opening and a refresh must not become a second tab.
        (
            TabKind::Extension {
                marketplace: ma,
                extension: ea,
                ..
            },
            TabKind::Extension {
                marketplace: mb,
                extension: eb,
                ..
            },
        ) => ma == mb && ea == eb,
        (TabKind::Diff { spec: a, .. }, TabKind::Diff { spec: b, .. }) => {
            match (&a.origin, &b.origin) {
                (
                    cide_ipc::DiffOrigin::Git {
                        repo: ra, path: pa, ..
                    },
                    cide_ipc::DiffOrigin::Git {
                        repo: rb, path: pb, ..
                    },
                ) => ra == rb && pa == pb,
                // A revision diff is keyed on **all four** fields, unlike the working-tree one
                // above, which deliberately leaves the side out. A working diff tab *switches
                // sides in place*, so the side is a mode of one tab; a revision pair **is** the
                // tab's identity, and two comparisons of one file are two different documents.
                // Without this arm the `_` below answered `false` for every pair of revision
                // tabs, so Ctrl+Shift+T on a closed one reopened a duplicate beside the original.
                (
                    cide_ipc::DiffOrigin::GitRevision {
                        repo: ra,
                        path: pa,
                        new: na,
                        old: oa,
                    },
                    cide_ipc::DiffOrigin::GitRevision {
                        repo: rb,
                        path: pb,
                        new: nb,
                        old: ob,
                    },
                ) => ra == rb && pa == pb && na == nb && oa == ob,
                // A `ClaudeMcp` diff answers a *blocked agent turn* through a `request_id` that
                // is dead the moment the turn ends, so reopening one could only ever produce a
                // tab waiting on a future nobody will resolve. Never the same tab as anything,
                // including itself.
                _ => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{DiffOrigin, DiffSpec, RepoId, SettingsSection, Workspace, git::DiffSide};

    /// This file's own source, for the two structural tests below.
    ///
    /// A structural test and not a behavioural one because the thing being pinned is *which
    /// thread a command runs on*, and Tauri decides that from the `async` keyword at
    /// compile time. There is no value to assert on at runtime: by the time a test could
    /// observe the wrong thread, the process it was observing has aborted.
    const SOURCE: &str = include_str!("project.rs");

    /// Every `async fn` in [`SOURCE`], as (name, body).
    ///
    /// Bodies are cut at the first line that is exactly `}` — rustfmt puts a closing brace at
    /// column 0 only at the end of a top-level item, and `cargo fmt --all --check` is the first
    /// step of this project's gate, so that boundary is enforced rather than hoped for.
    fn async_fns(src: &str) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        for (at, _) in src.match_indices("async fn ") {
            // `pub async fn` matches at the same place through its own `async fn`; only take
            // the ones that begin a line (possibly after `pub `), so a mention inside a comment
            // — of which this module has several — is not read as a definition.
            let line_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
            if !matches!(&src[line_start..at], "" | "pub ") {
                continue;
            }
            let rest = &src[at + "async fn ".len()..];
            let name = &rest[..rest.find('(').unwrap_or(0)];
            let body = match rest.find("\n}\n") {
                Some(end) => &rest[..end],
                None => rest,
            };
            out.push((name, body));
        }
        out
    }

    /// Every way into the open path, as the substring a call to it leaves in the source.
    ///
    /// **`project_open(` is in this list because it is the one the regression actually used.**
    /// `project_open_recent` did not call the private helper — that helper did not exist; it
    /// called the *command function*, which is `pub fn` and therefore callable from anywhere in
    /// the crate. A rule naming only [`open_project_here`] would pass while the exact original
    /// bug was written back in one line above it.
    const OPEN_CALLS: [&str; 2] = ["open_project_here(", "project_open("];

    /// **No `async` command may run an open on its own task.** The crash this batch fixed.
    ///
    /// `project_open_recent` called the `project_open` command function directly. That function
    /// is synchronous, so Tauri runs it on the main thread and its body is written for the main
    /// thread; `project_open_recent` is `async`, so Tauri runs it on a tokio worker — where
    /// `IdeServers::ensure`'s `rt.block_on` panics with "Cannot start a runtime from within a
    /// runtime" and, under the release profile's `panic = "abort"`, takes the whole app with it.
    ///
    /// So: an `async fn` here may reach the open path only after `run_on_main_thread`.
    /// The count is asserted as well as the rule, because a rule that stops matching anything
    /// passes for ever while the mechanism it guards is quietly deleted.
    ///
    /// [`OPEN_CALLS`] holds literals that also appear in this test's own text, which would be a
    /// trap if [`SOURCE`] were searched directly — it is this file, tests included. It is not
    /// one here: the needles are only ever looked for inside the *body of an `async fn`*, and
    /// every function in this module is synchronous.
    #[test]
    fn no_async_command_opens_a_project_on_its_own_task() {
        let mut hops = 0;
        for (name, body) in async_fns(SOURCE) {
            for call in OPEN_CALLS {
                let Some(at) = body.find(call) else {
                    continue;
                };
                assert!(
                    body[..at].contains("run_on_main_thread("),
                    "`{name}` is async, so Tauri runs it on a tokio worker — it must hop to the \
                     main thread before calling `{call})`, or `IdeServers::ensure` aborts the \
                     process"
                );
                hops += 1;
            }
        }
        assert_eq!(
            hops, 1,
            "exactly one async function should bridge to the open path \
             (`open_project_on_main_thread`); finding none means the bridge was removed and this \
             test now guards nothing"
        );
    }

    /// And the other half: the ordinary open stays synchronous.
    ///
    /// Making `project_open` `async` would move *every* open — the picker's, the palette's, a
    /// drag onto the window — onto a worker, which is the same abort with a wider blast radius
    /// and no recents entry involved.
    ///
    /// The negative half asks [`async_fns`] rather than `SOURCE.contains`, and that is not
    /// style: [`SOURCE`] is this file *including this test*, so a needle written as a literal
    /// here would find itself and the assertion would hold no matter what the code did. The
    /// escaped `\"camelCase\"` in the positive half is what keeps that one honest — the literal
    /// as it appears in the file is not the string it searches for.
    #[test]
    fn project_open_is_a_synchronous_command() {
        assert!(
            SOURCE.contains("#[tauri::command(rename_all = \"camelCase\")]\npub fn project_open("),
            "project_open must stay a synchronous #[tauri::command]"
        );
        assert!(
            !async_fns(SOURCE)
                .iter()
                .any(|(name, _)| *name == "project_open"),
            "project_open must not become async — see `open_project_here`"
        );
    }

    /// The panic itself, so the reasoning above is a fact rather than folklore.
    ///
    /// `IdeServers` owns a second tokio runtime and binds its port with `rt.block_on`. This is
    /// that call, made from a task on another runtime, which is exactly the position a `#[tauri::
    /// command] async fn` puts it in. If tokio ever stops panicking here the fix above is merely
    /// unnecessary rather than wrong — but the comment claiming a crash would have gone stale,
    /// and this is what notices.
    ///
    /// The inner runtime is held through an `Arc` whose last reference stays *outside* the task:
    /// dropping a `Runtime` from inside a runtime panics too, and a second panic during the first
    /// one's unwind is an abort, which would take the test binary rather than fail a test.
    #[test]
    fn a_second_runtime_cannot_be_driven_from_a_task_on_the_first() {
        let workers = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime");
        let ide = std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("a second runtime"),
        );

        let joined = workers.block_on({
            let ide = std::sync::Arc::clone(&ide);
            async move { tokio::spawn(async move { ide.block_on(async {}) }).await }
        });

        let error = joined.expect_err("block_on from inside a runtime does not return");
        assert!(
            error.is_panic(),
            "a command worker driving the IDE runtime panics; with panic=abort that is the crash"
        );
    }

    /// A workspace with one project and one file tab, and the ids for both.
    fn with_file(path: &str) -> (Workspace, ProjectId, TabId) {
        let mut ws = Workspace::default();
        let project = workspace::open_project(&mut ws, vec![PathBuf::from("/w")], None)
            .expect("a project opens");
        let tab = workspace::open_tab(
            &mut ws,
            project,
            TabKind::File {
                path: PathBuf::from(path),
                dirty: true,
            },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                title: "f".into(),
            },
        )
        .expect("a tab opens");
        (ws, project, tab)
    }

    /// The record carries the tab's position and its tree, and drops `dirty`.
    ///
    /// The dirty half is the one worth pinning. A tab only closes dirty because the user chose
    /// to discard, so the buffer is gone and the file on disk is what a reopen shows — a record
    /// that remembered `dirty: true` would come back claiming unsaved edits that exist nowhere,
    /// and the *next* close of that tab would raise a discard dialog about them.
    #[test]
    fn a_closing_record_remembers_the_position_and_the_tree_but_not_the_dirty_flag() {
        let (ws, project, tab) = with_file("/w/src/lib.rs");
        let panes: Vec<PaneId> = workspace::tab(&ws, project, tab)
            .expect("exists")
            .tree
            .panes
            .keys()
            .copied()
            .collect();

        let record = closing_record(&ws, project, tab).expect("a record");
        assert_eq!(record.project, project);
        assert_eq!(record.index, 1, "immediately right of the pinned console");
        assert_eq!(
            record.kind,
            TabKind::File {
                path: PathBuf::from("/w/src/lib.rs"),
                dirty: false,
            }
        );
        assert_eq!(
            record.tree.panes.keys().copied().collect::<Vec<_>>(),
            panes,
            "the pane ids come back with it, which is what lets a reopened tab re-adopt its \
             own parked terminal"
        );
    }

    /// Open `n` extra file tabs beside the console, and hand back every tab id in strip order.
    ///
    /// The ids are **read back off the strip** rather than collected as the tabs are made.
    /// `workspace::open_tab` inserts each new tab at index 1 — the newest file tab is the
    /// leftmost one — so creation order is the reverse of strip order, and every test below is
    /// about positions in the strip. Collecting them in creation order would silently shift
    /// every index in the reorder arithmetic these tests exist to pin.
    fn strip(n: usize) -> (Workspace, ProjectId, Vec<TabId>) {
        let (mut ws, project, _) = with_file("/w/f0.rs");
        for i in 1..n {
            workspace::open_tab(
                &mut ws,
                project,
                TabKind::File {
                    path: PathBuf::from(format!("/w/f{i}.rs")),
                    dirty: false,
                },
                Pane {
                    id: PaneId::new(),
                    kind: PaneKind::Editor,
                    role: PaneRole::Auxiliary,
                    session: None,
                    conversation: None,
                    title: "f".into(),
                },
            )
            .expect("a tab opens");
        }
        let ids = workspace::project(&ws, project)
            .expect("a project")
            .tabs
            .iter()
            .map(|t| t.id)
            .collect();
        (ws, project, ids)
    }

    /// The off-by-one, driven in both directions.
    ///
    /// The rightward rows are the ones that catch the bug: pass the boundary through unchanged
    /// and `(1, 2)` becomes `(1, 2)`, which `reorder_tab` turns into remove-then-insert at the
    /// same place — the tab does not move. Every leftward drag still works, so a manual test of
    /// "does dragging work" passes while half the gesture is dead.
    #[test]
    fn a_drop_boundary_becomes_the_index_the_tab_lands_on_after_it_is_lifted_out() {
        // The console plus four file tabs: ids 0..=4, and 4 is the last index.
        let (ws, project, ids) = strip(4);
        let tabs = &workspace::project(&ws, project).expect("a project").tabs;
        assert_eq!(tabs.len(), 5, "console + four files");
        let at = |tab: usize, before: Option<usize>| {
            reorder_target(tabs, ids[tab], before.map(|b| ids[b]))
                .expect("both ids are in the strip")
        };

        // Rightward: the boundary is past the lifted tab, so it loses one.
        assert_eq!(
            at(1, Some(3)),
            (1, 2),
            "tab 1 dropped before tab 3 lands at 2"
        );
        assert_eq!(at(1, None), (1, 4), "and dropped past the end, at the end");
        // The smallest rightward move there is, and the one the naive version turns into a no-op.
        assert_eq!(
            at(1, Some(2)),
            (1, 1),
            "dropped just before its own neighbour: unchanged"
        );

        // Leftward: the boundary is at or before the lifted tab, so it passes through.
        assert_eq!(
            at(3, Some(1)),
            (3, 1),
            "tab 3 dropped before tab 1 lands at 1"
        );
        assert_eq!(at(3, Some(3)), (3, 3), "dropped on itself is a no-op");
    }

    /// The two refusals, and the point is that they are the **domain's** rather than a second copy.
    ///
    /// `reorder_target` is pure arithmetic and deliberately does not know what a pinned tab is:
    /// it happily resolves a drop before the console to `to = 0`, and `reorder_tab` is what
    /// refuses it. One authority, so the frontend's clamp cannot drift into being the only guard.
    #[test]
    fn the_console_cannot_be_moved_and_nothing_can_be_moved_in_front_of_it() {
        let (mut ws, project, ids) = strip(3);
        // Resolved first, and the borrow released, so the domain call below can take `&mut ws`.
        let resolve = |tab: TabId, before: Option<TabId>| {
            reorder_target(
                &workspace::project(&ws, project).expect("a project").tabs,
                tab,
                before,
            )
            .expect("resolves")
        };
        let in_front = resolve(ids[2], Some(ids[0]));
        let the_console = resolve(ids[0], Some(ids[2]));

        assert_eq!(in_front, (2, 0), "the arithmetic does not editorialise");
        assert_eq!(
            workspace::reorder_tab(&mut ws, project, in_front.0, in_front.1),
            Err(CoreError::TabPinned),
            "and the domain refuses the drop in front of the console"
        );
        assert_eq!(
            workspace::reorder_tab(&mut ws, project, the_console.0, the_console.1),
            Err(CoreError::TabPinned),
            "and refuses moving the console itself"
        );
    }

    /// A tab that left the strip between the pointer-move and the drop.
    ///
    /// This is why the wire carries ids and not indices: the drop boundary is computed in the
    /// webview against a snapshot, and an agent's `openDiff` or another window's close can move
    /// the strip under it. With indices the stale call would silently reorder *some other tab*;
    /// with ids the worst case is this refusal.
    #[test]
    fn an_id_that_is_no_longer_in_the_strip_refuses_rather_than_moving_a_neighbour() {
        let (ws, project, ids) = strip(3);
        let tabs = &workspace::project(&ws, project).expect("a project").tabs;
        let gone = TabId::new();

        assert_eq!(
            reorder_target(tabs, gone, Some(ids[1])),
            Err(CoreError::NoSuchTab(gone))
        );
        assert_eq!(
            reorder_target(tabs, ids[1], Some(gone)),
            Err(CoreError::NoSuchTab(gone))
        );
    }

    #[test]
    fn there_is_nothing_to_remember_about_a_tab_that_is_not_there() {
        let (ws, project, _) = with_file("/w/a.rs");
        assert!(closing_record(&ws, project, TabId::new()).is_none());
        assert!(closing_record(&ws, ProjectId::new(), TabId::new()).is_none());
    }

    /// A temp file that goes away with the test. Nothing in this crate has a fixture for one and
    /// the deleted-file branch genuinely needs a real `stat`, so this is deliberately the same
    /// shape `lifecycle::tests::temp_dir` uses.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("cide-reopen-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("a scratch dir");
            let path = dir.join(name);
            std::fs::write(&path, b"x").expect("a scratch file");
            Self(path)
        }
        fn delete(&self) {
            std::fs::remove_file(&self.0).expect("removes");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().expect("a parent"));
        }
    }

    /// A workspace with one project holding only its pinned console.
    fn bare() -> (cide_ipc::Workspace, ProjectId) {
        let mut ws = cide_ipc::Workspace::default();
        let project = workspace::open_project(&mut ws, vec![PathBuf::from("/w")], None)
            .expect("a project opens");
        (ws, project)
    }

    /// The whole three-way rule, one branch at a time, on a real file.
    ///
    /// Each branch is a state an ordinary session reaches: deleting the file in another editor,
    /// opening it again from Ctrl+P and walking away from it, and pressing the key while looking
    /// at the tab it would reopen. The answers differ, and getting any of them wrong shows up as
    /// "Ctrl+Shift+T does nothing".
    #[test]
    fn the_reopen_rule_shows_a_hidden_tab_reinserts_a_gone_one_and_skips_what_it_cannot_show() {
        let scratch = Scratch::new("lib.rs");
        let (mut ws, project) = bare();
        let record = crate::closed_tabs::ClosedTab {
            project,
            kind: TabKind::File {
                path: scratch.0.clone(),
                dirty: false,
            },
            index: 1,
            tree: cide_core::layout::new_tree(Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                title: "lib.rs".into(),
            }),
        };

        // Not open: put it back.
        assert_eq!(reopen_plan(&ws, &record), Reopen::Reinsert);

        // Open and active: nothing a press could show, so drop the record and keep looking.
        let open = workspace::open_tab(
            &mut ws,
            project,
            TabKind::File {
                path: scratch.0.clone(),
                dirty: false,
            },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                title: "lib.rs".into(),
            },
        )
        .expect("opens");
        assert_eq!(reopen_plan(&ws, &record), Reopen::Skip);

        // Open but not active: showing it *is* the gesture. This is the branch that would be
        // wrong as a `Skip` — the user asked for that tab and would get an unrelated one.
        let console = workspace::console_tab(&ws, project).expect("exists");
        workspace::activate_tab(&mut ws, project, console).expect("activates");
        assert_eq!(reopen_plan(&ws, &record), Reopen::Show(open));

        // Deleted since: skipped whatever else is true, so the stat has to come first.
        workspace::close_tab(&mut ws, project, open, false).expect("closes");
        scratch.delete();
        assert_eq!(reopen_plan(&ws, &record), Reopen::Skip);
    }

    /// A record whose project closed under it. `forget_project` prunes these, so this is the
    /// race and not the ordinary path — but a `project(...).expect()` here would take the whole
    /// window down for it.
    #[test]
    fn a_record_for_a_project_that_is_gone_is_skipped_rather_than_fatal() {
        let scratch = Scratch::new("a.rs");
        let (ws, _) = bare();
        let orphan = crate::closed_tabs::ClosedTab {
            project: ProjectId::new(),
            kind: TabKind::File {
                path: scratch.0.clone(),
                dirty: false,
            },
            index: 1,
            tree: cide_core::layout::new_tree(Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                title: "a.rs".into(),
            }),
        };
        assert_eq!(reopen_plan(&ws, &orphan), Reopen::Skip);
    }

    fn git_diff(repo: RepoId, path: &str, side: DiffSide) -> TabKind {
        TabKind::Diff {
            spec: DiffSpec {
                title: "d".into(),
                old_path: PathBuf::from(path),
                new_path: PathBuf::from(path),
                origin: DiffOrigin::Git {
                    repo,
                    path: path.into(),
                    side,
                },
            },
            preview: false,
        }
    }

    /// `same_tab` has to agree with each opener's own dedupe, or a reopen produces the second
    /// tab over one file that those checks exist to prevent.
    #[test]
    fn an_already_open_tab_is_recognised_the_way_its_opener_recognises_it() {
        let a = TabKind::File {
            path: PathBuf::from("/w/a.rs"),
            dirty: false,
        };
        let a_dirty = TabKind::File {
            path: PathBuf::from("/w/a.rs"),
            dirty: true,
        };
        let b = TabKind::File {
            path: PathBuf::from("/w/b.rs"),
            dirty: false,
        };
        assert!(
            same_tab(&a, &a_dirty),
            "the path is the identity, not the flag"
        );
        assert!(!same_tab(&a, &b));

        // Settings is a per-project singleton, whatever section either one is showing.
        assert!(same_tab(
            &TabKind::Settings {
                section: SettingsSection::Keymap
            },
            &TabKind::Settings {
                section: SettingsSection::default()
            },
        ));

        // A git diff matches on repo and path and *not* on side: the pane switches sides while
        // it is open, so a record made on `Unstaged` names the tab now showing `Staged`.
        let repo = RepoId::new();
        assert!(same_tab(
            &git_diff(repo, "src/lib.rs", DiffSide::Staged),
            &git_diff(repo, "src/lib.rs", DiffSide::Unstaged),
        ));
        assert!(!same_tab(
            &git_diff(repo, "src/lib.rs", DiffSide::Staged),
            &git_diff(RepoId::new(), "src/lib.rs", DiffSide::Staged),
        ));

        // Two Claude tabs are never the same tab. They differ only by title and hold different
        // conversations, so matching them would silently drop one of the two records.
        let one = TabKind::ClaudeFull {
            title: "one".into(),
        };
        assert!(!same_tab(&one, &one.clone()));

        // And a diff Claude is blocked on never matches anything — it is never a record in the
        // first place (`closed_tabs::push` refuses it), and nothing here should make it one.
        let mcp = TabKind::Diff {
            spec: DiffSpec {
                title: "d".into(),
                old_path: PathBuf::from("a"),
                new_path: PathBuf::from("a"),
                origin: DiffOrigin::ClaudeMcp {
                    request_id: "r".into(),
                },
            },
            preview: false,
        };
        assert!(!same_tab(&mcp, &mcp.clone()));
    }
}
