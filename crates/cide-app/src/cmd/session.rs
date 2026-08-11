//! Session commands: spawn a PTY, attach a webview sink to it, write, resize.

use std::path::PathBuf;
use std::sync::Arc;

use cide_ipc::{Geometry, PaneId, SessionExit, SessionId};
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
///
/// The proxy variables are **not** here: they are the user's configuration rather than a
/// constant of the terminal, so they are a second pass — [`proxy_env`] — applied on top of
/// this one. Nothing about proxying changes the rule in the paragraph above.
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

/// The three proxy variables, in the spelling the tooling on this platform expects.
///
/// **Both cases are always written, and it is not belt-and-braces.** curl documents
/// `http_proxy` as lower case *only* — the upper-case form is deliberately ignored because a
/// CGI environment turns an incoming `Proxy:` header into `HTTP_PROXY` — while Go's
/// `httpproxy.FromEnvironment` prefers the upper-case one and much of the Node ecosystem
/// (`proxy-from-env`, which is what axios and friends use) reads lower first and upper
/// second. A pane that sets one spelling proxies half the commands the user types in it, and
/// the other half fail as a connection timeout with nothing anywhere saying why. So each of
/// these names is set — or removed — in both cases, always to the same value.
const PROXY_URL_VARS: [&str; 3] = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"];

/// Hosts that must never go through a proxy, whatever the user configured.
///
/// This is not a nicety. cide's IDE integration is an MCP server bound to loopback and found
/// through `CLAUDE_CODE_SSE_PORT`; a proxy that accepts `127.0.0.1:<port>` and forwards it
/// somewhere else takes inline diffs, @-mentions and the editor selection with it, silently,
/// and the user has no reason to connect the two. `cide-hook` talks over a unix socket and is
/// unaffected — this covers the one loopback TCP thing we own.
const LOOPBACK_EXEMPT: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Put a name in the child's environment under both spellings.
fn env_both_cases(spec: SpawnSpec, name: &str, value: &str) -> SpawnSpec {
    spec.env(name, value).env(name.to_lowercase(), value)
}

/// Take a name out of the child's environment under both spellings.
fn env_remove_both_cases(spec: SpawnSpec, name: &str) -> SpawnSpec {
    spec.env_remove(name).env_remove(name.to_lowercase())
}

/// The `NO_PROXY` value: the loopback exemption, then whatever else was asked for.
///
/// Deduplicated case-insensitively so that a user who types `localhost` themselves does not
/// get it twice, and loopback-first so the non-negotiable part is the part you read.
fn no_proxy_value(extra: &str) -> String {
    let mut entries: Vec<String> = LOOPBACK_EXEMPT.iter().map(|s| (*s).to_string()).collect();
    for entry in extra.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if !entries.iter().any(|e| e.eq_ignore_ascii_case(entry)) {
            entries.push(entry.to_string());
        }
    }
    entries.join(",")
}

/// Apply the user's proxy configuration to a child's environment.
///
/// Pure over `spec` and over `inherited`, which is why the tests below can prove every branch
/// without spawning anything or touching the process environment. `inherited` answers what
/// *this* process has, which is what a child would otherwise get — production passes
/// `std::env::var`.
///
/// # Inherit vs override
///
/// [`ProxyMode::Manual`] wins outright: every one of the six URL variables is set from these
/// settings or removed, so a `HTTP_PROXY` in the user's `.bashrc` cannot supply the half they
/// left blank. A setting that says "this is the proxy" and then silently loses to something
/// invisible in a shell profile is worse than no setting, because the user has no way to see
/// which one won.
///
/// [`ProxyMode::Inherit`] is the default and defers completely — with one exception that is
/// about loopback rather than about proxying. If the inherited environment already names a
/// proxy, `NO_PROXY` is rewritten to include [`LOOPBACK_EXEMPT`] on top of whatever it
/// already said. Without that, the common case — a corporate laptop with `HTTP_PROXY` in the
/// profile and no `NO_PROXY` — is one where cide's headline feature has never worked and
/// nothing reports it. When nothing is inherited, nothing is touched at all.
fn proxy_env(
    spec: SpawnSpec,
    proxy: &cide_ipc::ProxySettings,
    inherited: impl Fn(&str) -> Option<String>,
) -> SpawnSpec {
    use cide_ipc::{ProxyMode, normalize_proxy_url};

    /// Set from `value`, or remove the variable entirely when there is nothing to set.
    fn set_or_clear(spec: SpawnSpec, name: &str, value: Option<String>) -> SpawnSpec {
        match value {
            Some(v) => env_both_cases(spec, name, &v),
            None => env_remove_both_cases(spec, name),
        }
    }

    match proxy.mode {
        ProxyMode::Inherit => {
            let named = |name: &str| {
                inherited(name)
                    .or_else(|| inherited(&name.to_lowercase()))
                    .filter(|v| !v.trim().is_empty())
            };
            if !PROXY_URL_VARS.iter().any(|v| named(v).is_some()) {
                // The overwhelmingly common case, and the one where doing anything at all
                // would be meddling: no proxy anywhere, so no variable is added or removed.
                return spec;
            }
            let existing = named("NO_PROXY").unwrap_or_default();
            env_both_cases(spec, "NO_PROXY", &no_proxy_value(&existing))
        }
        ProxyMode::Manual => {
            let spec = set_or_clear(spec, "HTTP_PROXY", normalize_proxy_url(&proxy.http));
            let spec = set_or_clear(spec, "HTTPS_PROXY", proxy.https_url());
            let spec = set_or_clear(spec, "ALL_PROXY", normalize_proxy_url(&proxy.all));
            env_both_cases(spec, "NO_PROXY", &no_proxy_value(&proxy.no_proxy))
        }
        ProxyMode::Direct => {
            // `NO_PROXY` goes too. Leaving an inherited one behind would be harmless but
            // confusing: a child with no proxy and a long exemption list reads as though
            // something is still routing.
            let spec = PROXY_URL_VARS
                .iter()
                .fold(spec, |spec, name| env_remove_both_cases(spec, name));
            env_remove_both_cases(spec, "NO_PROXY")
        }
    }
}

/// One line for the log, with any credentials removed.
///
/// A proxy URL is the single most useful thing to have in a log when a pane cannot reach the
/// network, and `http://user:pass@proxy.corp:3128` is a perfectly ordinary value for it. So
/// the line exists, and it goes through [`cide_ipc::redact_proxy_url`] — the host survives,
/// the userinfo does not. `ProxySettings` also redacts in its own `Debug`, so the two ways of
/// getting this wrong are both closed rather than one of them being a convention.
fn proxy_log_line(proxy: &cide_ipc::ProxySettings) -> String {
    use cide_ipc::{ProxyMode, redact_proxy_url};
    match proxy.mode {
        ProxyMode::Inherit => "proxy: inherited from the environment".to_string(),
        ProxyMode::Direct => "proxy: none — scrubbed from this child".to_string(),
        ProxyMode::Manual => format!(
            "proxy: http={} https={} all={}",
            redact_proxy_url(&proxy.http),
            proxy
                .https_url()
                .map_or_else(String::new, |u| redact_proxy_url(&u)),
            redact_proxy_url(&proxy.all),
        ),
    }
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
    // Read once, here, rather than inside `proxy_env`: this is the only place that knows both
    // the app handle and that a child is about to exist, and `WorkspaceState::with` runs under
    // a non-reentrant lock that nothing further down should be holding. `try_state` because a
    // test harness may have no workspace, in which case the default — inherit, touch nothing —
    // is the right answer anyway.
    let proxy = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .map(|state| state.with(|ws| ws.settings.proxy.clone()))
        .unwrap_or_default();
    let mut spec = proxy_env(base_env(spec), &proxy, |name| std::env::var(name).ok());
    // Redacted, and at debug level: one line per spawn is worth it when a pane cannot reach
    // the network, but it is not worth it on every launch of a machine with no proxy at all.
    tracing::debug!("{}", proxy_log_line(&proxy));

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

/// Attach a webview sink to a session, and answer with the screen it should start from.
///
/// **The screen comes back from here rather than from [`session_scrollback`], and that is a
/// correctness change, not a round-trip saving.** Asking for the screen and then attaching are
/// two moments, and the mirror and the sinks are not in step between them: `cide-pty` feeds
/// the mirror the instant a chunk arrives but feeds sinks only on a flush, up to
/// `FLUSH_INTERVAL` later. Bytes that landed in that window are painted into the snapshot
/// *and* are still queued for the next flush, so the attaching pane was shown them twice —
/// a duplicated prompt on every re-dock and every rehydration.
/// [`cide_pty::PtySession::attach_with_snapshot`] does both on the coalescer thread, where a
/// point at which the two agree actually exists.
///
/// `pane` names *which* pane is attaching, and leaving it out is what broke mirroring: see
/// [`AttachmentKey`]. It is optional so a caller that does not name a pane still attaches,
/// on the old window-wide slot.
///
/// `Response` — raw bytes, no JSON, no base64 — for the same reason [`session_scrollback`]
/// uses it: a full screen of scrollback is not something to send through `serde_json`.
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
) -> Result<Response, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.resize(pty_geometry(geometry))
        .map_err(|e| SessionError::Pty(e.to_string()))?;

    let sink: Arc<dyn Sink> =
        Arc::new(move |bytes: &[u8]| sink.send(InvokeResponseBody::Raw(bytes.to_vec())).is_ok());
    let (id, screen) = s.attach_with_snapshot(sink);

    registry.record_attachment(
        AttachmentKey {
            session,
            window: window.label().to_string(),
            pane,
        },
        id,
    );
    Ok(Response::new(screen))
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
/// **No longer on the pane-open path** — [`session_attach`] answers with this itself, at a cut
/// point where the mirror and the sinks agree, which is the only way to get it without
/// double-painting the window between the two calls. This remains for callers that want the
/// screen and are not attaching: `cide-headless`, and anything scripting the app.
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

/// Send bytes to the child.
///
/// `seq` is a strictly increasing number minted by the caller, and it is what makes a
/// keystroke arrive exactly once over a transport that guarantees at-least-once — see
/// [`SessionRegistry::accept_write`] for the retry it defends against. It is optional so the
/// old wire shape still works; a write that names no `seq` is applied unconditionally, as it
/// always was.
///
/// **`window` is not decoration: the counter is minted per webview.** Every window runs its
/// own copy of `ui/src/ipc/client.ts` and starts counting at 1, and two windows share a
/// session whenever a pane is mirrored or torn out — `session.attach` attaches the new
/// window's sink before the old one detaches, on purpose. Deduplicating by session alone
/// would therefore have discarded the new window's first *n* keystrokes as replays, where
/// *n* is however much had been typed in the old one. Injected by Tauri, so it costs the wire
/// nothing.
///
/// A duplicate is `Ok(())`, not an error. The caller has no repair to make — the bytes *are*
/// in the child, delivered by the attempt this one is a replay of — and reporting a failure
/// would put a red line in the log for the mechanism working.
#[tauri::command(rename_all = "camelCase")]
pub fn session_write(
    registry: State<'_, SessionRegistry>,
    window: tauri::Window,
    session: SessionId,
    data: String,
    seq: Option<u64>,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    if !registry.accept_write(session, window.label(), seq) {
        tracing::debug!(%session, ?seq, window = window.label(), "session write: dropped a replayed frame");
        return Ok(());
    }
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

/// The same question as [`session_has_exited`], answered with the code instead of a `bool`.
///
/// Both exist because they are asked by different callers for different reasons, and the
/// difference is not stylistic. `session_has_exited` is a liveness predicate — the window
/// audit uses it to decide whether a pane still has a child — and a predicate is the right
/// shape for that. This one is asked by a pane that is *about to print a line to the user*,
/// where a `bool` throws away the only part anybody reads.
///
/// That loss was the whole defect: `cide-pty` maps a signalled child to the shell's
/// `128 + signum` specifically so an OOM kill (137), the SIGTERM this app sends on quit (143)
/// and an ordinary failure (1) can be told apart — and then the rehydration path asked a
/// question whose answer could not carry any of them. A pane that happened to be mounted when
/// its child died showed `— exited (137) —`; the identical pane rehydrated a moment later
/// showed `— exited —`. Same session, same corpse, different sentence.
///
/// **An unknown session is [`SessionExit::Unknown`], not an error.** Restoring a workspace from
/// `workspace.json` produces `SessionId`s whose processes died with the previous run, so that
/// is a routine answer with a correct rendering — and returning `NoSuchSession` here would put
/// the one case that legitimately has no code down the same path as a genuinely failed call.
#[tauri::command(rename_all = "camelCase")]
pub fn session_exit(registry: State<'_, SessionRegistry>, session: SessionId) -> SessionExit {
    exit_answer(&registry, session)
}

/// The body of [`session_exit`], as a free function over the registry.
///
/// Split out purely so it is testable: a `State<'_, T>` can only be built by a running Tauri
/// app, and this crate's tests have no app — the same wall `hooks::live_in` and
/// `ide::servable_projects` document. Everything that can be *wrong* here is in this function,
/// and the command is the one line that cannot be.
fn exit_answer(registry: &SessionRegistry, session: SessionId) -> SessionExit {
    let Some(s) = registry.get(session) else {
        return SessionExit::Unknown;
    };
    match (s.has_exited(), s.exit_status()) {
        // The status is read second but matched first, so it wins if the reaper lands between
        // the two reads. A code we have is never worth discarding for a flag that is merely
        // about to agree with it — and that ordering is what makes `Reaping` genuinely mean
        // "no status yet" rather than "we looked too early".
        (_, Some(exit)) => SessionExit::Exited { code: exit.code },
        (true, None) => SessionExit::Reaping,
        (false, None) => SessionExit::Running,
    }
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

    /// Spawn a shell that ends with `code`, and wait until the reaper has the status.
    ///
    /// Waits on `exit_status()` rather than `has_exited()`, which is the same trap a test in
    /// `cide-pty` already fell into: `has_exited` flips on EOF, and EOF is a *different event
    /// on a different thread* from `wait()` returning. Polling the flag and then asserting on
    /// the status is a race that passes on an idle machine and fails under load.
    fn reaped_with(code: i32) -> Arc<PtySession> {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg(format!("exit {code}"));
        let session = PtySession::spawn(spec).expect("spawn sh");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while session.exit_status().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(session.exit_status().is_some(), "child was never reaped");
        session
    }

    /// A session the registry has never held answers `Unknown` — **not an error**.
    ///
    /// This is the case the change exists for. Restoring a workspace produces `SessionId`s
    /// whose processes died with the previous run, so this is the routine answer on every
    /// launch, and it is the only one for which a bare `— exited —` is the truth. The command
    /// this replaced returned `NoSuchSession` here, which the frontend could only interpret as
    /// a failed call.
    #[test]
    fn a_session_the_registry_never_held_is_unknown() {
        let registry = SessionRegistry::default();
        assert_eq!(
            exit_answer(&registry, SessionId::new()),
            SessionExit::Unknown
        );
    }

    /// The code survives the round trip, which is the entire point of the command.
    ///
    /// `42` rather than `1`: a status the shell would never invent on its own, so a bug that
    /// substitutes a generic failure code cannot pass this. The predicate command it replaced
    /// answered `true` here and lost the number.
    #[test]
    fn a_reaped_session_reports_its_code() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();
        registry.insert(id, reaped_with(42));
        assert_eq!(exit_answer(&registry, id), SessionExit::Exited { code: 42 });
    }

    /// Zero is a code like any other on the wire.
    ///
    /// Suppressing `(0)` is a *rendering* decision and it lives in `exitMarker.ts`, where
    /// `showsCode` owns it and a check script covers it. Deciding it here as well would put
    /// the same judgement in two places and make the wire unable to distinguish "finished
    /// cleanly" from "nothing can say" — the exact collapse this whole path exists to undo.
    #[test]
    fn a_clean_exit_still_carries_its_zero() {
        let registry = SessionRegistry::default();
        let id = SessionId::new();
        registry.insert(id, reaped_with(0));
        assert_eq!(exit_answer(&registry, id), SessionExit::Exited { code: 0 });
    }

    /// A live child is `Running`, so a rehydrating pane adopts it instead of marking it dead.
    #[test]
    fn a_live_session_is_running() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 30");
        let session = PtySession::spawn(spec).expect("spawn sh");
        let registry = SessionRegistry::default();
        let id = SessionId::new();
        registry.insert(id, Arc::clone(&session));

        assert_eq!(exit_answer(&registry, id), SessionExit::Running);
        session.kill();
    }

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

    // --- proxy environment ---------------------------------------------------------------

    use cide_ipc::{ProxyMode, ProxySettings};

    /// A bare spec, so a test asserts on exactly what the proxy pass added.
    ///
    /// `base_env` is deliberately not applied: mixing its dozen entries in would make every
    /// assertion below a search through noise, and the two passes compose by construction —
    /// `proxy_env` only ever appends.
    fn proxy_spec(proxy: &ProxySettings, inherited: &[(&str, &str)]) -> SpawnSpec {
        let inherited: Vec<(String, String)> = inherited
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        proxy_env(
            SpawnSpec::new("/bin/sh", std::env::temp_dir()),
            proxy,
            move |name| {
                inherited
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            },
        )
    }

    fn value_of<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn manual(http: &str, https: &str, all: &str, no_proxy: &str) -> ProxySettings {
        ProxySettings {
            mode: ProxyMode::Manual,
            http: http.into(),
            https: https.into(),
            all: all.into(),
            no_proxy: no_proxy.into(),
        }
    }

    /// The default is inherit-and-do-nothing, and "nothing" has to mean *nothing*.
    ///
    /// A single spurious `NO_PROXY` here would be a behaviour change for every user who has
    /// never opened this screen, which is almost all of them.
    #[test]
    fn the_no_proxy_default_touches_nothing() {
        let spec = proxy_spec(&ProxySettings::default(), &[]);
        assert!(spec.env.is_empty(), "set {:?}", spec.env);
        assert!(spec.env_remove.is_empty(), "removed {:?}", spec.env_remove);
    }

    /// Every variable, in both spellings, from one configuration.
    #[test]
    fn a_manual_proxy_sets_each_variable_in_both_cases() {
        let spec = proxy_spec(
            &manual(
                "http://proxy.corp:3128",
                "http://tls.corp:3129",
                "socks5://socks.corp:1080",
                "",
            ),
            &[],
        );

        for (upper, expected) in [
            ("HTTP_PROXY", "http://proxy.corp:3128"),
            ("HTTPS_PROXY", "http://tls.corp:3129"),
            ("ALL_PROXY", "socks5://socks.corp:1080"),
        ] {
            assert_eq!(value_of(&spec, upper), Some(expected), "{upper}");
            assert_eq!(
                value_of(&spec, &upper.to_lowercase()),
                Some(expected),
                "{upper} lower case — curl reads only this spelling of http_proxy"
            );
        }
    }

    /// One proxy typed once reaches both HTTP and HTTPS.
    #[test]
    fn https_defaults_to_the_http_proxy_and_all_proxy_does_not() {
        let spec = proxy_spec(&manual("proxy.corp:3128", "", "", ""), &[]);
        assert_eq!(
            value_of(&spec, "HTTPS_PROXY"),
            Some("http://proxy.corp:3128")
        );
        // ALL_PROXY covers protocols the user never said anything about, so a blank field
        // stays blank — and, in Manual mode, is actively removed.
        assert_eq!(value_of(&spec, "ALL_PROXY"), None);
        assert!(spec.env_remove.iter().any(|k| k == "ALL_PROXY"));
        assert!(spec.env_remove.iter().any(|k| k == "all_proxy"));
    }

    /// A blank field in Manual mode removes the variable rather than deferring to the profile.
    ///
    /// This is the inherit-vs-override decision, written as a test: a user who configured a
    /// proxy here and left `ALL_PROXY` empty must not silently get the one their `.bashrc`
    /// exported, because nothing on screen would ever say so.
    #[test]
    fn manual_mode_overrides_an_inherited_proxy_rather_than_merging_with_it() {
        let spec = proxy_spec(
            &manual("http://proxy.corp:3128", "", "", ""),
            &[("ALL_PROXY", "socks5://legacy:1080")],
        );
        assert_eq!(value_of(&spec, "ALL_PROXY"), None, "not carried over");
        assert!(
            spec.env_remove.iter().any(|k| k == "ALL_PROXY"),
            "and scrubbed, so the inherited one cannot reach the child"
        );
    }

    /// Loopback is exempt in Manual mode, ahead of anything the user added.
    #[test]
    fn loopback_is_exempt_and_cannot_be_configured_away() {
        let spec = proxy_spec(
            &manual(
                "http://proxy.corp:3128",
                "",
                "",
                "corp.internal, .example.com",
            ),
            &[],
        );
        assert_eq!(
            value_of(&spec, "NO_PROXY"),
            Some("localhost,127.0.0.1,::1,corp.internal,.example.com")
        );
        assert_eq!(
            value_of(&spec, "no_proxy"),
            Some("localhost,127.0.0.1,::1,corp.internal,.example.com")
        );
    }

    /// A user who lists loopback themselves gets it once, not twice.
    #[test]
    fn the_exemption_list_does_not_repeat_what_the_user_already_wrote() {
        let spec = proxy_spec(&manual("http://p:3128", "", "", "LocalHost,corp"), &[]);
        assert_eq!(
            value_of(&spec, "NO_PROXY"),
            Some("localhost,127.0.0.1,::1,corp")
        );
    }

    /// The case this exists for: a proxy in the shell profile, no `NO_PROXY`, IDE server on
    /// loopback. cide defers on the proxy itself and still rescues the loopback.
    #[test]
    fn an_inherited_proxy_still_gets_the_loopback_exemption() {
        let spec = proxy_spec(
            &ProxySettings::default(),
            &[("http_proxy", "http://profile.corp:3128")],
        );
        assert_eq!(
            value_of(&spec, "NO_PROXY"),
            Some("localhost,127.0.0.1,::1"),
            "the IDE MCP server is on loopback and a proxy that swallows it is silent"
        );
        // And nothing else — the inherited proxy is left exactly as the profile set it.
        assert_eq!(value_of(&spec, "HTTP_PROXY"), None);
        assert!(spec.env_remove.is_empty());
    }

    /// An inherited `NO_PROXY` is extended, not replaced.
    #[test]
    fn an_inherited_no_proxy_keeps_its_entries() {
        let spec = proxy_spec(
            &ProxySettings::default(),
            &[
                ("HTTPS_PROXY", "http://profile.corp:3128"),
                ("NO_PROXY", "corp.internal"),
            ],
        );
        assert_eq!(
            value_of(&spec, "NO_PROXY"),
            Some("localhost,127.0.0.1,::1,corp.internal")
        );
    }

    /// Direct means direct: every spelling gone, nothing set.
    #[test]
    fn direct_scrubs_every_spelling() {
        let spec = proxy_spec(
            &ProxySettings {
                mode: ProxyMode::Direct,
                ..ProxySettings::default()
            },
            &[("HTTP_PROXY", "http://profile.corp:3128")],
        );
        assert!(spec.env.is_empty(), "set {:?}", spec.env);
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
            assert!(spec.env_remove.iter().any(|k| k == name), "{name}");
            assert!(
                spec.env_remove.iter().any(|k| *k == name.to_lowercase()),
                "{name} lower case"
            );
        }
    }

    /// A credentialed proxy reaches the child intact and the log line never sees it.
    #[test]
    fn a_credentialed_proxy_never_reaches_a_log_line() {
        let proxy = manual(
            "http://alice:hunter2@proxy.corp:3128",
            "",
            "socks5://alice:hunter2@socks.corp:1080",
            "",
        );

        // The child does get the real thing — a redaction that reached the environment would
        // be a proxy that cannot authenticate.
        let spec = proxy_spec(&proxy, &[]);
        assert_eq!(
            value_of(&spec, "HTTP_PROXY"),
            Some("http://alice:hunter2@proxy.corp:3128")
        );

        // Every string this module can put in front of a human.
        let line = proxy_log_line(&proxy);
        let debug = format!("{proxy:?}");
        for text in [&line, &debug] {
            assert!(!text.contains("hunter2"), "password leaked: {text}");
            assert!(!text.contains("alice"), "username leaked: {text}");
        }
        assert!(
            line.contains("proxy.corp:3128"),
            "the host is what makes the line worth having: {line}"
        );
    }

    /// The modes that carry no URL say so without inventing one.
    #[test]
    fn the_log_line_names_the_mode_when_there_is_no_url() {
        assert!(proxy_log_line(&ProxySettings::default()).contains("inherited"));
        assert!(
            proxy_log_line(&ProxySettings {
                mode: ProxyMode::Direct,
                ..ProxySettings::default()
            })
            .contains("none")
        );
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
