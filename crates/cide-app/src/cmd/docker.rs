//! Reading and acting on the machine's Docker daemon. (M41)
//!
//! Handlers stay thin, `cmd/mod.rs`'s policy: unwrap arguments, call `docker_state`, wrap the
//! result. Everything that decides *anything* — which daemon, what a failure means, what the
//! daemon's own words were — lives in `cide-docker` or `docker_state`.
//!
//! # Every one of these is `async` over `blocking`
//!
//! `cmd/spec.rs`'s rule, and it bites harder here: Tauri 2 polls a synchronous command on the
//! GTK main loop, and a Docker call is a round trip to a daemon that may be a virtual machine —
//! `remove` on a large container takes real seconds. A synchronous handler would freeze every
//! window for the duration. `#[tauri::command(async)]` is *not* the fix and neither is an `async
//! fn` whose body never awaits; both only move the wait onto a runtime worker where it still
//! blocks.

use std::sync::Arc;

use cide_core::CoreError;
use cide_ipc::docker::{ContainerAction, DockerBoard};
use tauri::{AppHandle, State};

use crate::docker_state::DockerState;

type Result<T> = std::result::Result<T, CoreError>;

/// Run a blocking job on the pool and report a lost worker as an I/O failure.
///
/// Mirrors `cmd::spec::blocking`. A join failure means the task panicked or the runtime is going
/// away; neither is something the frontend can act on differently from the call itself failing.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(CoreError::Io(format!(
            "the docker worker did not finish: {error}"
        ))),
    }
}

/// The whole board.
///
/// Never `Err` for a Docker reason — see [`DockerBoard`]. The `Result` is here because the
/// worker itself can be lost, which is not a fact about Docker.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_board(
    app: AppHandle,
    state: State<'_, Arc<DockerState>>,
) -> Result<DockerBoard> {
    let state = Arc::clone(&state);
    // `board_watching` rather than `board`: the daemon's event stream is subscribed to here, on
    // the first read that succeeds, which is the moment somebody is actually looking at Docker.
    // Without it the panel is only ever as fresh as the last thing the user did in cide — a
    // container started in a terminal would not appear until Refresh.
    blocking(move || Ok(state.board_watching(&app))).await
}

/// Do one thing to one container, and answer with the whole board.
///
/// # Why the board comes back rather than `{ok: true}`
///
/// `cmd/tasks.rs`'s rule: a call that returned success would be followed at once by a second
/// asking what happened, and the frame in between shows a list that is visibly wrong — the row
/// the user just stopped still drawn as running. The board is also emitted, for the *other*
/// windows; this return value is what makes the acting window's own update immediate.
///
/// A daemon refusal is an `Err`, deliberately, and it is one of the few here that is: the button
/// did not do what it said, and the daemon's own sentence ("You cannot remove a running
/// container") is better than anything cide could compose. `cmd/tasks.rs`'s note on `runCommand`
/// applies — a mutation whose failure is swallowed ships broken three times.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_container_action(
    app: AppHandle,
    state: State<'_, Arc<DockerState>>,
    container: String,
    action: ContainerAction,
) -> Result<DockerBoard> {
    let state = Arc::clone(&state);
    let board = blocking(move || {
        state
            .act(&container, action)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(state.board())
    })
    .await?;
    crate::emit::docker_changed(&app, &board);
    Ok(board)
}

/// How much disk one volume is using. (M57)
///
/// # Why this is not a field on the volume's detail
///
/// Because it costs a filesystem walk. `GET /volumes` carries no size at all — the only endpoint
/// that does is `/system/df`, measured here at **8.4 seconds cold** and about 1.2 warm. Putting it
/// on `docker_detail` would make clicking a volume row take that long, and putting it on the board
/// would make every daemon event do it.
///
/// So it is asked separately, once a volume's detail is open, and the pane draws a placeholder
/// until it lands. `None` is the daemon declining to measure, which is **not** zero: a volume
/// reported as `0 B` when nobody counted is a claim that it is empty.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_volume_size(
    state: State<'_, Arc<DockerState>>,
    name: String,
) -> Result<Option<i64>> {
    let state = Arc::clone(&state);
    blocking(move || {
        state
            .volume_size(&name)
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await
}

/// Remove an image, a volume or a network, and answer with the whole board. (M56)
///
/// # A refusal is the point of this command, not a failure of it
///
/// Nothing here forces. Docker refuses each of these while anything is using it, and its sentence
/// names *what* — `image is being used by stopped container a1b2c3`, `volume is in use - […]`,
/// `has active endpoints` — which is more useful than the removal would have been. So the `Err`
/// arm is not an edge case to be tidied away: it is the ordinary answer whenever the user picked
/// something in use, and `dockerStore` shows it verbatim. See `cide_docker::api::remove`.
///
/// The board comes back for `docker_container_action`'s reason, and is emitted for the same one.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_remove(
    app: AppHandle,
    state: State<'_, Arc<DockerState>>,
    target: cide_ipc::docker::Removable,
) -> Result<DockerBoard> {
    let state = Arc::clone(&state);
    let board = blocking(move || {
        state
            .remove(&target)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(state.board())
    })
    .await?;
    crate::emit::docker_changed(&app, &board);
    Ok(board)
}

/// Point at a different daemon, and answer with that daemon's board.
///
/// `endpoint` is a `DOCKER_HOST` URL, taken from a [`cide_ipc::docker::ContextRow`] the panel
/// already has. `None` restores the ladder — which is not the same as naming the endpoint the
/// ladder currently picks: a machine whose `docker context use` changes under a running cide
/// should follow it, and only an explicit choice should pin.
///
/// # The endpoint is parsed here and never trusted as a string
///
/// It arrives from the webview, and `cide_docker::connect::Endpoint::parse` is the only thing
/// that decides what a `DOCKER_HOST` spelling means — including refusing the schemes cide does
/// not speak. A command that passed the string through would let `ssh://` reach a code path
/// that had never been told it cannot be reached.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_use_endpoint(
    app: AppHandle,
    state: State<'_, Arc<DockerState>>,
    endpoint: Option<String>,
) -> Result<DockerBoard> {
    let endpoint = match endpoint.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        Some(value) => Some(
            cide_docker::connect::Endpoint::parse(value)
                .map_err(|refusal| CoreError::Io(refusal.sentence()))?,
        ),
        None => None,
    };

    let state = Arc::clone(&state);
    let board = blocking(move || {
        state.choose(endpoint);
        Ok(state.board())
    })
    .await?;
    crate::emit::docker_changed(&app, &board);
    Ok(board)
}

/// Open an exec or a log follow on a container, and hand back a live session id. (M42)
///
/// # Why this answers a `SessionId` and looks nothing like a Docker command
///
/// Because what it produces *is* an ordinary session. `cide_docker::session` builds it through
/// `PtySession::connect`, so from here on it is indistinguishable from a shell pane: the frontend
/// attaches with `session_attach`, writes with `session_write`, resizes with `session_resize`, and
/// detaching into a window or parking it across a project switch works because none of that code
/// knows or cares where the bytes come from. That is the whole return on M42's `Transport` seam.
///
/// The pane is bound to the id afterwards by `pane_bind_session`, exactly as `session_spawn`'s is
/// — a session exists before it belongs to a pane.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_session_open(
    app: AppHandle,
    state: State<'_, Arc<DockerState>>,
    registry: State<'_, crate::state::SessionRegistry>,
    container: String,
    stream: cide_ipc::docker::DockerStream,
    // The **wire** geometry, converted here — `cide_pty::Geometry` is not a DTO and deriving
    // `Deserialize` on it would put a wire concern in the PTY crate. `session_spawn`'s own
    // `pty_geometry` does the conversion and is reused rather than restated, because a second
    // copy is a second chance to swap `cols` and `rows`.
    geometry: cide_ipc::Geometry,
) -> Result<cide_ipc::SessionId> {
    let state = Arc::clone(&state);
    let id = cide_ipc::SessionId::new();
    let session = blocking(move || {
        state
            .open_stream(&container, stream, super::session::pty_geometry(geometry))
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await?;

    // Before the registry insert, so no window can learn about this session before something is
    // watching for its death — `session_spawn`'s ordering, and its comment applies unchanged.
    //
    // **`watch_jobs` is deliberately not called.** It announces `Busy`/`AwaitingInput` from
    // `tcgetpgrp` against a pid this machine owns, and neither of these transports has one; the
    // seam's own `foreground_pgid` already answers `None`, so calling it would install a watcher
    // that can never fire. Not calling it says so.
    crate::lifecycle::watch_for_exit(app.clone(), id, &session);
    registry.insert(id, session);
    Ok(id)
}

/// The raw `inspect` document for a container, image, volume or network. (M43)
///
/// Answers text, which the frontend opens in a read-only editor tab — `TabKind::Docker`. Not a
/// modelled shape: see `cide_docker::Docker::inspect` for why the whole truth is the feature.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_inspect(
    state: State<'_, Arc<DockerState>>,
    target: cide_ipc::docker::InspectTarget,
) -> Result<String> {
    let state = Arc::clone(&state);
    blocking(move || {
        state
            .inspect(&target)
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await
}

/// Bring a Compose stack up, down, or restart it. (M43)
///
/// # Why this answers the board and the action's output is dropped
///
/// The board, for `docker_container_action`'s reason: a call that returned success would be
/// followed at once by a second asking what happened. The *output* is dropped because Compose
/// writes progress rather than a result — `Container shop-db-1  Started`, one line per service —
/// and the result the user actually wants is the container list, which is what comes back.
///
/// A **failure** is an `Err` carrying Compose's own last line, which is the sentence that says
/// what went wrong ("port is already allocated"), and it must not be swallowed.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_compose_action(
    app: AppHandle,
    state: State<'_, Arc<DockerState>>,
    project: String,
    action: cide_ipc::docker::ComposeAction,
    working_dir: Option<String>,
    files: Vec<String>,
) -> Result<DockerBoard> {
    let state = Arc::clone(&state);
    let board = blocking(move || {
        let dir = working_dir.map(std::path::PathBuf::from);
        state
            .compose(dir.as_deref(), &project, action, &files)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(state.board())
    })
    .await?;
    crate::emit::docker_changed(&app, &board);
    Ok(board)
}

/// Say that a Docker panel is, or is no longer, on screen. (M52)
///
/// # Why the frontend has to tell Rust this
///
/// Because the subscription that keeps a board live is expensive and invisible. `GET /events` is
/// an open socket and a thread, and every event it delivers becomes a **full board read** —
/// containers, images, volumes and networks. `DockerState::ensure_watching` made that lazy, so it
/// starts only once somebody opens the panel, and for a year that was the whole rule: it never
/// stopped. A session that showed the Docker tab once kept reading the daemon for the rest of its
/// life, for a panel nobody was looking at.
///
/// Rust cannot work this out for itself. Whether a panel is mounted is a fact about the React
/// tree, in a window it has no handle on, and the panel lives in the bottom tool window's active
/// tab — which changes with a click Rust never hears about. So the panel says so, on mount and on
/// unmount, and `DockerState` counts because every window has one.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_watch(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<DockerState>>,
    // What is true for this window *now*, and when it said so. **A level and a stamp, never an
    // increment** — see `DockerState::panels` for the two orderings that broke a counter, both of
    // which StrictMode produces on every mount in development.
    seq: u64,
    watching: bool,
) -> Result<()> {
    let state = Arc::clone(&state);
    let label = window.label().to_string();
    blocking(move || {
        state.watching(&app, &label, seq, watching);
        Ok(())
    })
    .await
}

/// Resolve a compose file and a verb into something a terminal pane can spawn. (M48)
///
/// # Why this hands back a plan instead of running it
///
/// `cide_ipc::docker::ComposeRun` carries the argument at length. In one line: the pane that runs
/// it is an **ordinary shell pane**, so `session_spawn` — with its job watcher, its proxy
/// environment, its `$EDITOR`, and everything that makes a pane survive a detach and a project
/// switch — is the right spawn site, and a second one here would be a copy of all of it.
///
/// A refusal is an `Err` carrying `ComposeAvailability`'s own sentence, which names what to
/// install. That matters more on this road than on the panel's: a user who right-clicked a
/// compose file has said plainly what they expect to happen, and silence is not an answer.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_compose_plan(
    file: String,
    action: cide_ipc::docker::ComposeAction,
    // The services to narrow the run to, or empty for the whole file. The editor's gutter draws
    // one marker per service and passes that one; every other surface passes none.
    services: Vec<String>,
) -> Result<cide_ipc::docker::ComposeRun> {
    // No `DockerState` and no daemon handle, for `DockerState::compose`'s reason: a Compose
    // invocation is a CLI lookup, and requiring a connection would refuse a command that would
    // have worked while cide happened to be between daemons.
    blocking(move || {
        cide_docker::compose::plan(std::path::Path::new(&file), action, &services)
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await
}

/// Open a read-only inspect tab, or activate the one already showing this thing. (M43)
///
/// # Why an existing tab is reused
///
/// `tab_open_spec`'s rule, and the failure it prevents is the same: a panel row clicked twice
/// mints two tabs that look identical in the strip, and the user closes one at random. Matching
/// on the *target* rather than the title is what makes that work — two containers can share a
/// name, and the same container's tab must be found whatever its name was when it was opened.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_open_inspect(
    state: State<'_, crate::workspace_state::WorkspaceState>,
    project: cide_ipc::ProjectId,
    target: cide_ipc::docker::InspectTarget,
    name: String,
) -> Result<cide_ipc::TabId> {
    let wanted = target;
    state.update(|ws| {
        let open = cide_core::workspace::project(ws, project)?;
        let existing = open
            .tabs
            .iter()
            .find(|tab| matches!(&tab.kind, cide_ipc::TabKind::Docker { target, .. } if *target == wanted))
            .map(|tab| tab.id);
        if let Some(id) = existing {
            cide_core::workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }
        cide_core::workspace::open_tab(
            ws,
            project,
            cide_ipc::TabKind::Docker {
                title: wanted.title(&name),
                target: wanted.clone(),
            },
            cide_ipc::Pane {
                id: cide_ipc::PaneId::new(),
                kind: cide_ipc::PaneKind::Editor,
                role: cide_ipc::PaneRole::Auxiliary,
                // No process — the document renders over the tree, exactly as an OpenSpec page
                // and a Settings page do.
                session: None,
                conversation: None,
                conversation_since: None,
                continues: None,
                title: "docker".into(),
                docker: None,
            },
        )
    })
}

/// List one directory inside a container. (M44)
///
/// Never `Err` for a container reason: a distroless image with no shell answers `Unusable` with a
/// sentence, because a tree that said "empty" would be a lie about a filesystem that is full.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_files_list(
    state: State<'_, Arc<DockerState>>,
    container: String,
    path: String,
) -> Result<cide_ipc::docker::ContainerListing> {
    let state = Arc::clone(&state);
    blocking(move || Ok(state.list_dir(&container, &path))).await
}

/// Read one file out of a container, as text. (M44)
///
/// An `Err` here **is** the answer for the ordinary refusals — a directory, a symlink, a binary,
/// something over the cap — and each carries its own sentence naming which. The pane shows it in
/// place of the buffer; an empty buffer would read as an empty file.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_files_read(
    state: State<'_, Arc<DockerState>>,
    container: String,
    path: String,
) -> Result<String> {
    let state = Arc::clone(&state);
    blocking(move || {
        state
            .read_file(&container, &path)
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await
}

/// Open a container's file browser, or activate the tab already showing it. (M44)
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_open_files(
    state: State<'_, crate::workspace_state::WorkspaceState>,
    project: cide_ipc::ProjectId,
    container: String,
    name: String,
) -> Result<cide_ipc::TabId> {
    let wanted = container;
    state.update(|ws| {
        let open = cide_core::workspace::project(ws, project)?;
        let existing = open
            .tabs
            .iter()
            .find(|tab| {
                matches!(&tab.kind, cide_ipc::TabKind::DockerFiles { container, .. } if *container == wanted)
            })
            .map(|tab| tab.id);
        if let Some(id) = existing {
            cide_core::workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }
        cide_core::workspace::open_tab(
            ws,
            project,
            cide_ipc::TabKind::DockerFiles {
                container: wanted.clone(),
                name: name.clone(),
            },
            cide_ipc::Pane {
                id: cide_ipc::PaneId::new(),
                kind: cide_ipc::PaneKind::Editor,
                role: cide_ipc::PaneRole::Auxiliary,
                session: None,
                conversation: None,
                conversation_since: None,
                continues: None,
                title: "docker files".into(),
                docker: None,
            },
        )
    })
}

/// What the detail pane shows for one container, image, volume or network. (M47)
///
/// Never `Err` for a Docker reason: a thing removed between the row being drawn and the row being
/// clicked answers `Missing` with a sentence, because that is an ordinary outcome on this surface
/// and a pane that threw would take the panel down through its boundary.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_detail(
    state: State<'_, Arc<DockerState>>,
    target: cide_ipc::docker::InspectTarget,
) -> Result<cide_ipc::docker::DockerDetail> {
    let state = Arc::clone(&state);
    blocking(move || Ok(state.detail(&target))).await
}

/// Apply an edit to a container by replacing it. (M47)
///
/// # What this actually does, and why the name says so
///
/// Docker cannot change a running container's ports. The container is removed and a new one is
/// created from the same image, under the same name, with the same configuration except for what
/// the edit names — see `cide_docker::detail::recreate`. Every tool that appears to edit a
/// container does this; naming the command `recreate` is what stops the *next* reader assuming
/// there is an update endpoint it forgot about.
///
/// The **id changes**, so anything holding the old one — an exec pane, a log follow — is attached
/// to a container that no longer exists. The answer carries the whole board so the panel
/// repaints, and the new id so a caller can follow it.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_recreate(
    app: AppHandle,
    state: State<'_, Arc<DockerState>>,
    container: String,
    edit: cide_ipc::docker::ContainerEdit,
) -> Result<DockerBoard> {
    let state = Arc::clone(&state);
    let board = blocking(move || {
        state
            .recreate(&container, &edit)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(state.board())
    })
    .await?;
    crate::emit::docker_changed(&app, &board);
    Ok(board)
}

/// Copy a file or directory out of a container onto this machine. (M47)
///
/// # Why the dialog is opened from Rust
///
/// Because the webview's is not available. Tauri routes `window.showSaveFilePicker` and friends
/// through its dialog plugin, which is capability-gated and cide does not grant — the same wall
/// `globalThis.confirm` hit, and it fails as a *rejected command* whose text reads like a stale
/// binary rather than as a missing permission. `cmd::project::project_pick` already opens a
/// native picker from Rust for that reason; this follows it.
///
/// # A directory comes out as a tar
///
/// There is no recursive read in the Engine API that is not one, and unpacking it here would mean
/// deciding *where* — which is the user's answer, given through this very dialog. `docker cp` of a
/// directory produces the same tar, so the shape is one a Docker user already expects.
///
/// Answers the path written, or `None` when the dialog was cancelled. Cancelled is not an error:
/// it is the ordinary way of deciding not to.
#[tauri::command(rename_all = "camelCase")]
pub async fn docker_files_download(
    window: tauri::WebviewWindow,
    state: State<'_, Arc<DockerState>>,
    container: String,
    path: String,
    directory: bool,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let base = path.rsplit('/').next().unwrap_or("download").to_string();
    let suggested = if directory {
        format!("{base}.tar")
    } else {
        base
    };

    let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(if directory {
            "Save directory as a tar archive"
        } else {
            "Save file"
        })
        .set_file_name(&suggested)
        .save_file(move |picked| {
            let _ = tx.send(picked.and_then(|p| p.into_path().ok()));
        });

    // `spawn_blocking` rather than blocking the command's own task — `project_pick`'s reason: the
    // answer arrives only when the user has finished browsing, which is unbounded, and the async
    // runtime's workers are shared with every other command in flight.
    let destination = tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or(None))
        .await
        .map_err(|e| CoreError::Io(format!("save dialog: {e}")))?;
    let Some(destination) = destination else {
        return Ok(None);
    };

    let state = Arc::clone(&state);
    let written = destination.clone();
    blocking(move || {
        // Read **after** the dialog, so a large file does not stall the gesture before the user
        // has even said where it goes. The cost is that a refusal — a symlink, something over the
        // cap — arrives after they have chosen, which is the better half of the trade: the
        // sentence names the path, and nothing has been written.
        let bytes = if directory {
            state.read_archive(&container, &path)
        } else {
            state.read_bytes(&container, &path)
        }
        .map_err(|error| CoreError::Io(error.to_string()))?;

        std::fs::write(&written, bytes).map_err(|error| {
            CoreError::Io(format!("could not write {}: {error}", written.display()))
        })?;
        Ok(())
    })
    .await?;

    Ok(Some(destination.display().to_string()))
}
