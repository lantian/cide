//! Session commands: spawn a PTY, attach a webview sink to it, write, resize.

use std::path::PathBuf;
use std::sync::Arc;

use cide_ipc::{Geometry, PaneId, SessionId};
use cide_pty::{Geometry as PtyGeometry, PtySession, Sink, SpawnSpec};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{Manager, State};

use crate::state::{AttachmentKey, SessionRegistry};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("no such session")]
    NoSuchSession,
    #[error("{0}")]
    Pty(String),
}

impl serde::Serialize for SessionError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Tagged, so the frontend branches on a variant rather than matching on prose.
        let (kind, message) = match self {
            Self::NoSuchSession => ("noSuchSession", self.to_string()),
            Self::Pty(_) => ("pty", self.to_string()),
        };
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("SessionError", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &message)?;
        st.end()
    }
}

/// Environment every PTY child gets.
///
/// `TERM=xterm-256color` rather than plain `xterm` is not cosmetic: with `xterm` the
/// Claude Code TUI falls back to 8 colours and ASCII box-drawing, and the alternate screen
/// does not engage. Scrubbing `TMUX` matters for the same reason — its presence triggers
/// an unconditional 256-colour clamp that visibly desaturates the accent colour.
///
/// Deliberately absent: `ANTHROPIC_API_KEY`. It outranks subscription OAuth in the
/// credential precedence order, so injecting one would silently bill a Console org for a
/// user on Claude Max. The child inherits its auth by inheriting the environment; cide
/// never reads `~/.claude/.credentials.json`.
fn base_env(spec: SpawnSpec) -> SpawnSpec {
    spec.env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "cide")
        .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
        // xterm.js reports one wheel event per notch, unamplified, which makes the TUI
        // scroll a single line at a time and feel broken.
        .env("CLAUDE_CODE_SCROLL_SPEED", "3")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        // A terminal that inherits stale COLUMNS/LINES lies to the child about its size
        // until the first SIGWINCH.
        .env_remove("COLUMNS")
        .env_remove("LINES")
        .env_remove("CI")
}

/// Whether this program is the Claude Code CLI, and so has hooks worth registering.
///
/// Matched on the file name, which is enough because the frontend spawns the bare string
/// `claude` and lets `PATH` resolve it.
///
/// **The limitation is worth stating, because breaking it is silent.** `claude` on this
/// machine resolves to `~/.local/share/claude/versions/2.1.226`, whose file name is a version
/// number and matches nothing here. That is harmless today — the resolution happens in the
/// OS, after this decision — but the moment anything passes an absolute path as the program
/// (a configurable CLI location in Settings, say), this returns false, no `--settings` is
/// attached, and every session runs with no hooks: no token figures, no fast buffer reload,
/// and a close confirm that cannot tell busy from idle. Nothing fails; the features simply
/// are not there. A change to what is passed as `program` needs a change here too.
fn program_is_claude(program: &str) -> bool {
    std::path::Path::new(program)
        .file_name()
        .map(|n| n == "claude")
        .unwrap_or(false)
}

/// Whether this spawn should branch rather than continue.
///
/// **`Option<bool>` is a wire requirement, not taste.** Tauri looks each command parameter up
/// by key and hands the value to serde: a key the frontend did not send reaches an `Option`
/// as `None` (`CommandItem::deserialize_option` visits none) but reaches a plain `bool`
/// through `deserialize_json`, which returns `Err("command session_spawn missing required
/// key fork")`. `TerminalPane`'s `specFor` builds its spec without a `fork` property at all,
/// only the `forkPrimary` split branch ever assigns one, and `JSON.stringify` drops an absent
/// key — so with `fork: bool` **every ordinary pane spawn was rejected before it reached
/// this function**, which is why a restored pane neither resumed nor spawned.
///
/// The alternative that lost was giving `session.spawn` a `fork = false` default in
/// `ui/src/ipc/client.ts`, the way `project.close` and `git.status` do for their flags. It
/// fixes the same call sites, but only until the next caller forgets, and this side is the
/// one that has to be right for callers it has not met.
fn wants_fork(fork: Option<bool>) -> bool {
    fork.unwrap_or(false)
}

/// The conversation arguments for a Claude child.
///
/// Order matters and a wrong one fails silently, so this is a function with tests rather
/// than a run of `.arg()` calls inline in a Tauri command.
///
/// `--fork-session` composing with `--session-id` was an open question in the plan, with a
/// fallback designed around it possibly not working. It was checked against 2.1.226: the
/// combination is accepted, the id we pass **is** honoured, the fork inherits the parent's
/// history, and the parent's transcript survives untouched beside the fork's. Both remain
/// independently resumable. So the id stays ours and no hook-learned correction is needed.
///
/// `fork` without `resume` is meaningless — there is nothing to branch from — and is treated
/// as a plain new session rather than passed through to be rejected by the CLI.
fn claude_args(id: SessionId, resume: Option<SessionId>, fork: bool) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(parent) = resume {
        args.push("--resume".into());
        args.push(parent.to_string());
        if fork {
            args.push("--fork-session".into());
        }
    }
    args.push("--session-id".into());
    args.push(id.to_string());
    args
}

/// The inline `--settings` JSON, or `None` when `cide-hook` cannot be located.
///
/// Looked up beside our own executable, which is where every packaging format this project
/// ships puts the two binaries together.
fn hook_settings() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let hook = exe.parent()?.join("cide-hook");
    if !hook.exists() {
        return None;
    }
    let settings =
        cide_claude::inline_settings(&hook.to_string_lossy(), &cide_claude::StatusLine::Ours);
    serde_json::to_string(&settings).ok()
}

/// Convert a wire geometry into the PTY crate's own, which clamps and derives pixel dims.
fn pty_geometry(g: Geometry) -> PtyGeometry {
    PtyGeometry::new(g.cols, g.rows, g.cell_width, g.cell_height)
}

/// Run a blocking job on the pool and report a lost worker as a PTY error.
///
/// Mirrors `cmd::file::blocking`. A join failure means the task panicked or the runtime is
/// going away; neither is something the frontend can act on differently from the operation
/// itself failing, so it does not get an error variant of its own.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, SessionError> + Send + 'static,
) -> Result<T, SessionError> {
    match tauri::async_runtime::spawn_blocking(job).await {
        Ok(result) => result,
        Err(error) => Err(SessionError::Pty(format!("session worker failed: {error}"))),
    }
}

/// Spawn a child for a pane.
///
/// `resume` continues an existing conversation; `resume` with `fork` branches from it, so
/// the new session shares history up to this point and then diverges while the parent is
/// left untouched. Both are ignored for anything that is not the Claude CLI.
///
/// `async`, and the fork itself on the blocking pool, because this is the one session
/// command that starts a process: `openpty` plus `fork`/`exec` plus four thread spawns, all
/// of which ran on the webview's main thread. A window whose user split four panes at once
/// paid for all four before it could paint anything.
#[tauri::command(rename_all = "camelCase")]
// A Tauri command's parameters are its wire shape: the frontend passes a flat object and the
// macro destructures it. Grouping these into a struct to satisfy the lint would add a type
// that exists only to be immediately taken apart, and would change the JSON the frontend
// sends. `pane_split` carries the same allow for the same reason.
#[allow(clippy::too_many_arguments)]
pub async fn session_spawn(
    app: tauri::AppHandle,
    registry: State<'_, SessionRegistry>,
    program: String,
    args: Vec<String>,
    cwd: String,
    geometry: Geometry,
    project: Option<cide_ipc::ProjectId>,
    resume: Option<SessionId>,
    // Optional on the wire, and it has to be: see [`wants_fork`].
    fork: Option<bool>,
) -> Result<SessionId, SessionError> {
    let mut spec = SpawnSpec::new(program, PathBuf::from(cwd)).geometry(pty_geometry(geometry));
    for a in args {
        spec = spec.arg(a);
    }
    let mut spec = base_env(spec);
    // Only Claude children get the settings payload. A shell has no hooks to register, and
    // handing it a `--settings` argument would simply be a bad argv.
    let is_claude = program_is_claude(&spec.program);

    // Minted before the spawn, not after, because for a Claude pane this id *is* the value
    // passed to `--session-id`. That equality is what makes everything downstream work: a
    // hook reports the CLI's `session_id`, and unless the CLI was told to use ours, every
    // frame it sends names a uuid this process has never heard of and is dropped. It is also
    // what lets a restored pane resume with `--resume <id>` and no extra bookkeeping.
    let id = SessionId::new();
    if is_claude {
        for a in claude_args(id, resume, wants_fork(fork)) {
            spec = spec.arg(a);
        }
    }

    // `CLAUDE_CODE_SSE_PORT` is load-bearing, not a hint. It makes a port match alone mark our
    // lockfile valid — skipping the cwd-containment, pid-liveness and PID-ancestry checks —
    // and it is what gates the CLI's port-filtered selection, so without it a `claude` here
    // falls back to disambiguating among every lockfile in a shared directory and may bind to
    // another editor entirely. See `cide-ide-mcp::lockfile`.
    //
    // Set for every pane, not only Claude ones: a user who types `claude` into a cide shell
    // should reach this project's server too.
    if let Some(project) = project
        && let Some(servers) = app.try_state::<crate::ide::IdeServers>()
        && let Some(port) = servers.port(project)
    {
        spec = spec.env("CLAUDE_CODE_SSE_PORT", port.to_string());
    }

    // Hooks are what make the status bar's token figures, the fast buffer reload and the
    // busy-vs-idle close confirm possible. They are registered inline via `--settings`
    // rather than by editing `~/.claude/settings.json`, which is the user's file and would
    // otherwise carry cide's hooks into every `claude` they ever run.
    if let Some(server) = app.try_state::<crate::hooks::HookServer>() {
        spec = spec.env(
            "CIDE_HOOK_SOCK",
            server.socket().to_string_lossy().to_string(),
        );

        if is_claude {
            match hook_settings() {
                Some(json) => spec = spec.arg("--settings").arg(json),
                // Without an absolute path to `cide-hook` the child cannot run it: its cwd is
                // the project root and its PATH is the user's. Skipping the flag leaves a
                // working session with no hooks, which is the right way to fail here.
                None => tracing::warn!("cannot locate cide-hook; this session reports no state"),
            }
        }
    }

    let session =
        blocking(move || PtySession::spawn(spec).map_err(|e| SessionError::Pty(e.to_string())))
            .await?;

    // The pid→pane binding is not done here: a session exists before it belongs to a pane,
    // and `pane_bind_session` is the one place that knows both. Binding early would have to
    // invent a pane id and then correct it.

    // Before the registry insert, so no window can learn about this session before something
    // is watching for its death. The watcher is a callback on the reaper now rather than a
    // thread of its own, and registering it after the child has already gone is safe — it
    // fires immediately instead of never — but registering it first keeps the ordering
    // obvious rather than relying on that.
    crate::lifecycle::watch_for_exit(app.clone(), id, &session);

    registry.insert(id, session);
    Ok(id)
}

/// Answer a diff that Claude Code is blocked on.
///
/// The three outcomes are the protocol's, not ours: accepted-with-edits carries the buffer
/// the user actually has on screen (which is why the diff pane reads its editor at click
/// time rather than trusting the proposal it was handed), accepted-as-proposed writes the
/// model's version, and rejected leaves the file alone.
#[tauri::command(rename_all = "camelCase")]
pub fn claude_diff_result(
    app: tauri::AppHandle,
    project: cide_ipc::ProjectId,
    request_id: String,
    outcome: cide_ipc::DiffAnswer,
) -> Result<(), SessionError> {
    let Some(servers) = app.try_state::<crate::ide::IdeServers>() else {
        return Err(SessionError::Pty("the IDE subsystem is not running".into()));
    };
    crate::ide::resolve_or_cancel(&servers, project, &request_id, Some(to_outcome(outcome)));
    Ok(())
}

/// The two documents a diff pane shows.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffContent {
    pub original: String,
    pub proposed: String,
}

/// Fetch the documents for a pending diff.
///
/// Deliberately *not* stored in [`cide_ipc::DiffSpec`]. That struct lives in `Workspace`,
/// which is serialised to `workspace.json` on a debounce — putting `new_file_contents` in it
/// would write the full text of every proposed edit into the user's saved layout, grow the
/// file without bound, and persist file contents long after the diff was answered. The
/// broker already holds the proposal for exactly as long as it is relevant, so the frontend
/// asks for it when it renders the tab and never afterwards.
///
/// `async` because it reads a file off disk. On the main thread that is a stall the length
/// of one `read(2)` on whatever filesystem the project happens to live on — a network mount
/// makes it a visible freeze at the moment a diff tab opens.
#[tauri::command(rename_all = "camelCase")]
pub async fn claude_diff_content(
    app: tauri::AppHandle,
    project: cide_ipc::ProjectId,
    request_id: String,
) -> Result<DiffContent, SessionError> {
    let servers = app
        .try_state::<crate::ide::IdeServers>()
        .ok_or_else(|| SessionError::Pty("the IDE subsystem is not running".into()))?;
    let broker = servers
        .broker(project)
        .ok_or_else(|| SessionError::Pty("no IDE server for that project".into()))?;

    let request = broker
        .pending()
        .into_iter()
        .find(|r| r.id == request_id)
        .ok_or_else(|| SessionError::Pty("that diff is no longer pending".into()))?;

    // A file the model is creating has no previous version; an empty left side is the
    // honest rendering of that, and is what makes the diff show as all-additions.
    let old_path = request.params.old_file_path.clone();
    let original =
        blocking(move || Ok(std::fs::read_to_string(&old_path).unwrap_or_default())).await?;

    Ok(DiffContent {
        original,
        proposed: request.params.new_file_contents,
    })
}

/// Bridge the wire form to the protocol's own enum.
///
/// Written here rather than in `cide-ipc` because that crate must not depend on the MCP
/// implementation — it is the contract, and the contract cannot know how the contract is served.
fn to_outcome(a: cide_ipc::DiffAnswer) -> cide_ide_mcp::DiffOutcome {
    match a {
        cide_ipc::DiffAnswer::AcceptedEdited { contents } => {
            cide_ide_mcp::DiffOutcome::Saved { contents }
        }
        cide_ipc::DiffAnswer::AcceptedAsIs => cide_ide_mcp::DiffOutcome::TabClosed,
        cide_ipc::DiffAnswer::Rejected => cide_ide_mcp::DiffOutcome::Rejected,
    }
}

/// Attach a webview sink to a session.
///
/// The caller receives the current screen first (see [`session_scrollback`]) so a pane
/// that opens onto an already-running session paints immediately instead of waiting for
/// the child's next output.
///
/// `pane` names *which* pane is attaching, and leaving it out is what broke mirroring: see
/// [`AttachmentKey`]. It is optional so a caller that does not name a pane still attaches,
/// on the old window-wide slot.
///
/// Synchronous, and the resize deliberately not on the blocking pool: see [`session_resize`].
#[tauri::command(rename_all = "camelCase")]
pub fn session_attach(
    registry: State<'_, SessionRegistry>,
    window: tauri::Window,
    session: SessionId,
    pane: Option<PaneId>,
    sink: Channel<InvokeResponseBody>,
    geometry: Geometry,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.resize(pty_geometry(geometry))
        .map_err(|e| SessionError::Pty(e.to_string()))?;

    let sink: Arc<dyn Sink> =
        Arc::new(move |bytes: &[u8]| sink.send(InvokeResponseBody::Raw(bytes.to_vec())).is_ok());
    let id = s.attach(sink);

    registry.record_attachment(
        AttachmentKey {
            session,
            window: window.label().to_string(),
            pane,
        },
        id,
    );
    Ok(())
}

/// Report that this pane has finished processing `bytes` of the session's output.
///
/// The pane, not the window: credit is per sink, and two mirrors of one session render at
/// their own speeds. Crediting by window would let a pane that is keeping up pay off the
/// debt of one that has stalled, which is the flow control failing open.
///
/// Called from `term.write`'s completion callback, which is the only moment xterm has
/// actually parsed the bytes rather than merely received them. Until this arrives the bytes
/// count as outstanding, and a sink that accumulates enough of them stops being sent raw
/// output — see [`cide_pty::CreditPolicy`].
///
/// Silently ignores a session or attachment that has gone away. An ack racing a pane close
/// is ordinary rather than exceptional, and there is nothing useful to tell the caller.
///
/// Stays synchronous, unlike its neighbours: this is the per-frame hot path and its whole
/// body is a map lookup and an atomic add. Handing each one to the async runtime would cost
/// a task spawn per frame to save nothing.
#[tauri::command(rename_all = "camelCase")]
pub fn session_ack(
    registry: State<'_, SessionRegistry>,
    window: tauri::Window,
    session: SessionId,
    pane: Option<PaneId>,
    bytes: usize,
) {
    let key = AttachmentKey {
        session,
        window: window.label().to_string(),
        pane,
    };
    if let Some(sink) = registry.attachment(&key)
        && let Some(s) = registry.get(session)
    {
        s.ack(sink, bytes);
    }
}

/// Drop one pane's sink. The session and its child are untouched.
#[tauri::command(rename_all = "camelCase")]
pub fn session_detach(
    registry: State<'_, SessionRegistry>,
    window: tauri::Window,
    session: SessionId,
    pane: Option<PaneId>,
) {
    let key = AttachmentKey {
        session,
        window: window.label().to_string(),
        pane,
    };
    if let Some(sink) = registry.take_attachment(&key)
        && let Some(s) = registry.get(session)
    {
        s.detach(sink);
    }
}

/// The byte sequence that reconstructs the current screen on a fresh terminal.
///
/// Sent as the first frame after attaching. For a fullscreen TUI the frontend follows it
/// with a one-frame `cols-1 → cols` resize nudge, so the application repaints from its own
/// model — covering sequences the screen mirror does not track (OSC 8 hyperlinks, OSC 52
/// clipboard traffic, DEC 2026 synchronized-output framing).
///
/// `async` because serialising the mirror is not cheap: `state_formatted` walks every cell
/// of a screen that may hold ten thousand lines of scrollback, and every pane in a restored
/// workspace asks for one at the same moment.
#[tauri::command(rename_all = "camelCase")]
pub async fn session_scrollback(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
) -> Result<Response, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    let state = blocking(move || Ok(s.screen_state())).await?;
    Ok(Response::new(state))
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_in_alternate_screen(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
) -> Result<bool, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    Ok(s.in_alternate_screen())
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_write(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
    data: String,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.write(data.into_bytes());
    Ok(())
}

/// Push a new size at the child.
///
/// **Synchronous on purpose, unlike its neighbours.** `vt100::Screen::set_size` reflows the
/// whole scrollback, so this is the one blocking body here with a real case for the pool —
/// and it is the one body that must not go there. `PtySession::resize` takes three locks in
/// turn (`master`, then `vt`, then `geometry`) and holds none of them across the others, so
/// it is atomic only while its callers are serialised. Tauri runs synchronous commands one
/// at a time on the thread that receives the IPC message, which is exactly that guarantee;
/// `spawn_blocking` hands them to a pool and withdraws it.
///
/// Two overlapping resizes on the pool interleave into a state no single call asked for —
/// the kernel PTY at one size and the screen mirror at another — and nothing corrects it
/// until the next resize. `session_scrollback` then paints that mismatched mirror into every
/// pane that re-docks or rehydrates. `syncSize` fires from a `ResizeObserver` on every frame
/// of a window drag, and the reflow that justifies the pool is precisely what makes one call
/// still be running when the next arrives, so the race is likeliest where it costs most.
///
/// The fix that would earn the pool back belongs in `cide-pty` and is not this change's to
/// make: one lock across the whole of `resize`, and a coalescing queue per session so the
/// last size asked for is the one the child ends up at. Until then a stall is the honest
/// trade — a resize that is late is a frame behind, a resize that is inconsistent is wrong.
#[tauri::command(rename_all = "camelCase")]
pub fn session_resize(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
    geometry: Geometry,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.resize(pty_geometry(geometry))
        .map_err(|e| SessionError::Pty(e.to_string()))
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_has_exited(
    registry: State<'_, SessionRegistry>,
    session: SessionId,
) -> Result<bool, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    Ok(s.has_exited())
}

/// Every session the registry currently holds a child for.
///
/// The registry, not the workspace tree. That distinction is the whole claim M5 makes: a
/// pane names a session, but the session outlives the pane's position, its tab and its
/// window, and only the registry knows what is actually running.
#[tauri::command(rename_all = "camelCase")]
pub fn session_list(registry: State<'_, SessionRegistry>) -> Vec<SessionId> {
    registry.ids()
}

#[tauri::command(rename_all = "camelCase")]
pub fn session_kill(registry: State<'_, SessionRegistry>, session: SessionId) {
    if let Some(s) = registry.get(session) {
        s.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_session_only_names_itself() {
        let id = SessionId::new();
        assert_eq!(
            claude_args(id, None, false),
            vec!["--session-id".to_string(), id.to_string()]
        );
    }

    #[test]
    fn a_spawn_that_says_nothing_about_forking_does_not_fork() {
        // The signature is the point of this test as much as the value: `wants_fork` takes an
        // `Option`, so restoring `fork: bool` on the command stops this compiling. That
        // matters because the failure it guards is invisible from Rust — a plain `bool`
        // parameter makes Tauri reject the whole invocation for a missing key, and the
        // frontend only ever sends `fork` on a `forkPrimary` split.
        assert!(!wants_fork(None), "an unsent flag must mean 'no'");
        assert!(!wants_fork(Some(false)));
        assert!(wants_fork(Some(true)));

        let id = SessionId::new();
        assert_eq!(
            claude_args(id, None, wants_fork(None)),
            vec!["--session-id".to_string(), id.to_string()],
            "an ordinary pane spawns plain"
        );
    }

    #[test]
    fn resuming_names_the_parent_before_naming_the_new_session() {
        // `--resume <parent>` and `--session-id <ours>` both take a uuid, so a swapped order
        // is still a valid command line that resumes the wrong conversation.
        let id = SessionId::new();
        let parent = SessionId::new();
        assert_eq!(
            claude_args(id, Some(parent), false),
            vec![
                "--resume".to_string(),
                parent.to_string(),
                "--session-id".to_string(),
                id.to_string(),
            ]
        );
    }

    #[test]
    fn forking_branches_from_the_parent_and_keeps_our_id() {
        let id = SessionId::new();
        let parent = SessionId::new();
        assert_eq!(
            claude_args(id, Some(parent), true),
            vec![
                "--resume".to_string(),
                parent.to_string(),
                "--fork-session".to_string(),
                "--session-id".to_string(),
                id.to_string(),
            ]
        );
    }

    #[test]
    fn forking_with_nothing_to_fork_from_is_an_ordinary_new_session() {
        // Rather than passing `--fork-session` alone for the CLI to reject. A split that
        // asked to branch a project with no primary session should still give the user a
        // working pane.
        let id = SessionId::new();
        assert_eq!(
            claude_args(id, None, true),
            vec!["--session-id".to_string(), id.to_string()]
        );
    }

    #[test]
    fn the_program_string_the_frontend_actually_sends_is_recognised() {
        // `specFor` in TerminalPane.tsx sends exactly this, letting PATH resolve it.
        assert!(program_is_claude("claude"));
        assert!(program_is_claude("/usr/local/bin/claude"));
        assert!(!program_is_claude("/bin/bash"));
        assert!(!program_is_claude("claude-hook"));
    }

    #[test]
    fn a_version_resolved_path_is_not_recognised_and_that_is_a_known_limit() {
        // Documented rather than fixed, because it cannot be fixed by name-matching: this is
        // what `claude` resolves to on a real install, and its file name is a version number.
        // Nothing cide spawns takes this form today. If that ever changes, hooks silently
        // stop registering — see `program_is_claude`.
        assert!(!program_is_claude(
            "/home/u/.local/share/claude/versions/2.1.226"
        ));
    }
}
