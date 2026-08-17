//! Session commands: spawn a PTY, attach a webview sink to it, write, resize.

use std::path::PathBuf;
use std::sync::Arc;

use cide_core::proxy::ProxyEnv;
use cide_ipc::{Geometry, PaneId, SessionExit, SessionId};
use cide_pty::{Geometry as PtyGeometry, PtySession, Sink, SpawnSpec};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{Manager, State};

use crate::state::{AttachmentKey, SessionRegistry};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("no such session")]
    NoSuchSession,
    /// A plain resume naming a conversation this app already has open. See `session_spawn`.
    #[error("session {0} is already open in this window; a conversation cannot be resumed twice")]
    AlreadyOpen(SessionId),
    /// The configured Claude binary cannot be executed. (M16)
    ///
    /// # Why a variant, when portable-pty would have failed anyway
    ///
    /// It fails as `SessionError::Pty("No such file or directory (os error 2)")`, which names
    /// neither the program nor the setting that chose it. A configurable binary makes that the
    /// *ordinary* failure rather than an exotic one — a typo in Settings kills every pane at
    /// once — so it gets a sentence that names the value and where to correct it.
    ///
    /// It needs no frontend change to be seen. `ui/src/panes/exitMarker.ts::spawnFailureText`
    /// prefers a tagged `message` verbatim and `TerminalPane` writes it into the pane's own
    /// transcript, so the sentence composed in `cide_core::claude_cli::BinaryProblem::message`
    /// is what the user reads, in the pane that failed.
    ///
    /// Deliberately **not** in `isRecoverableSessionError`: retrying cannot help, and a pane
    /// that retries a missing binary spins.
    #[error("{message}")]
    NoClaudeBinary { program: String, message: String },
    #[error("{0}")]
    Pty(String),
}

impl serde::Serialize for SessionError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Tagged, so the frontend branches on a variant rather than matching on prose.
        let (kind, message) = match self {
            Self::NoSuchSession => ("noSuchSession", self.to_string()),
            Self::AlreadyOpen(_) => ("alreadyOpen", self.to_string()),
            Self::NoClaudeBinary { .. } => ("noClaudeBinary", self.to_string()),
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
/// constant of the terminal, so they are a second pass — [`apply_proxy`], over the rule in
/// [`cide_core::proxy`] — applied on top of this one. Nothing about proxying changes the rule
/// in the paragraph above.
///
/// The first pass is [`cide_core::child_env`], which undoes what *our own* launcher did to the
/// environment before a pane ever sees it. Running from the AppImage, `AppRun` leaves
/// `PYTHONHOME` pointing inside a bundle that contains no Python, and every stdio MCP server a
/// pane's `claude` spawns dies on `No module named 'encodings'` before it can speak protocol —
/// reported by the CLI as `CONNECTION_CLOSED` against a configuration that is perfectly
/// correct. It runs first so that the explicit settings below are the ones that survive a
/// collision, and it is a no-op for every non-bundled launch.
///
/// # The `CLAUDE_CODE_*` pass, and why it is last
///
/// [`cide_core::child_env::claude_env`] turns the user's [`cide_ipc::ClaudeSettings`] into the
/// same `EnvChange` list, and is folded **after** the constants below so that a switch the user
/// actually set wins over anything this function assumed. It carries
/// `CLAUDE_CODE_SCROLL_SPEED`, which used to be a literal `3` here.
///
/// The comment that literal carried was wrong, and the correction is the point of this
/// paragraph. It read *"xterm.js reports one wheel event per notch, unamplified"*. Claude Code's
/// own renderer heuristic concludes the opposite: it classifies a terminal announcing itself as
/// `xterm.js` — which cide's XTVERSION reply deliberately does, see `ui/src/terminal/xterm.ts`
/// — as a wheel **flooder**, and on that branch its unset default is `1` rather than the `3` it
/// gives other renderers. So this variable was never the amplifier the comment described; it
/// was cancelling a penalty cide had asked for two files away, and landing back on the ordinary
/// default. Setting it remains right. The stated reason was not.
///
/// What a notch is actually worth is the product of two numbers, and cide only owns one of
/// them. xterm.js sends **at most one mouse report per DOM wheel event** — `sendEvent` computes
/// a line count and then discards it — and under a high-resolution wheel on Wayland one notch
/// arrives as several small deltas, each of which `CoreMouseService.consumeWheelEvent` scales by
/// `0.3` when `|deltaY| < 50` on the theory that it is a trackpad. Whether this machine's mouse
/// lands in that regime is not knowable from here, and is not knowable without a wheel and a
/// window. That is why the number is now the user's: it is the half of the product cide can
/// move, from a control, without guessing at the other half.
///
/// Applied to every pane rather than only to `claude` ones, which is deliberate and matches
/// what the literal did before. These variables mean nothing to `bash`, and a user who types
/// `claude` at a shell pane's prompt should get the settings they configured rather than the
/// defaults of a program cide did not notice starting.
///
/// # The user's own variables, and why *those* stop at a shell pane (M16)
///
/// [`cide_core::claude_cli::user_env`] is folded last of the three, and only when `is_claude`.
/// The inconsistency with the paragraph above is deliberate and is written down here so it is
/// not "fixed" later: the four `CLAUDE_CODE_*` names are inert to `bash` — a shell that
/// inherits them is a shell that ignores them — while an arbitrary `NODE_OPTIONS`,
/// `GIT_SSH_COMMAND` or `PATH` from that list is not inert to anything. A field labelled
/// *the environment claude panes are spawned with* must not quietly become the environment the
/// user's own shell is spawned with too.
///
/// Last of the three so that a variable the user set beats a constant this function assumed —
/// which is why `TERM`, `COLUMNS`, `LINES` and `TMUX` are on the refusal list rather than left
/// to be shadowed. Everything applied *after* this function — the proxy pass,
/// `CLAUDE_CODE_SSE_PORT`, `CIDE_HOOK_SOCK` — is out of the user's reach by construction, which
/// is the other half of why those names are refused rather than merely discouraged: a value
/// this list carried for one of them would be overwritten with nothing on screen saying so.
fn base_env(
    spec: SpawnSpec,
    claude: &cide_ipc::ClaudeSettings,
    user_env: Vec<cide_core::child_env::EnvChange>,
) -> SpawnSpec {
    let spec = apply_env_changes(spec, cide_core::child_env::bundle_scrub())
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "cide")
        .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        // A terminal that inherits stale COLUMNS/LINES lies to the child about its size
        // until the first SIGWINCH.
        .env_remove("COLUMNS")
        .env_remove("LINES")
        .env_remove("CI");
    let spec = apply_env_changes(spec, cide_core::child_env::claude_env(claude));
    apply_env_changes(spec, user_env)
}

/// Fold a list of [`cide_core::child_env::EnvChange`]s into a spec.
///
/// A separate function only so it can be tested: `bundle_scrub` reads the real process
/// environment, which a test cannot set up without `unsafe` and a race against every other
/// thread, but the folding is where an ordering or set-versus-remove mistake would live.
fn apply_env_changes(
    spec: SpawnSpec,
    changes: impl IntoIterator<Item = cide_core::child_env::EnvChange>,
) -> SpawnSpec {
    changes
        .into_iter()
        .fold(spec, |spec, (name, value)| match value {
            Some(value) => spec.env(name, value),
            None => spec.env_remove(name),
        })
}

/// Which column of [`cide_ipc::ProxyScope`] a pane about to be spawned falls in.
///
/// A function rather than an inline `if` because the decision is one this file already makes
/// once, badly, and must now make earlier: `program_is_claude` was computed *after* the proxy
/// pass, and the proxy pass is now the thing that needs it. Naming it keeps the two uses of
/// the same fact from drifting apart, and puts the `claude`-panes-and-one-shots-are-one-thing
/// rule where a reader of either can find it.
fn pane_proxy_target(scope: &cide_ipc::ProxyScope, is_claude: bool) -> cide_ipc::ProxyTarget {
    if is_claude {
        scope.claude
    } else {
        scope.shells
    }
}

/// Apply a resolved proxy environment to a pane's spec.
///
/// The rule itself lives in [`cide_core::proxy`] — three spawn sites in three crates need the
/// same answer now that [`cide_ipc::ProxyScope`] exists, and this is the one that speaks
/// `SpawnSpec`. What is left here is the fold, which is exactly the part `cide-core` cannot
/// do: it does not link `cide-pty`, and must not.
fn apply_proxy(spec: SpawnSpec, env: &ProxyEnv) -> SpawnSpec {
    apply_env_changes(spec, env.changes().to_vec())
}

/// One line for the log, with any credentials removed.
///
/// A proxy URL is the single most useful thing to have in a log when a pane cannot reach the
/// network, and `http://user:pass@proxy.corp:3128` is a perfectly ordinary value for it. So
/// the line exists, and it goes through [`cide_ipc::redact_proxy_url`] — the host survives,
/// the userinfo does not. `ProxySettings` and `ProxyEnv` both redact in their own `Debug`
/// impls, so the three ways of getting this wrong are all closed rather than one of them
/// being a convention.
///
/// It names the **target** as well as the values, and that is not decoration: the question a
/// log is read for is "why did this child not reach the network", and with a scope in play
/// "cide set nothing for this kind of child" is now one of the answers.
fn proxy_log_line(kind: &str, target: cide_ipc::ProxyTarget, env: &ProxyEnv) -> String {
    let target = match target {
        cide_ipc::ProxyTarget::Configured => "configured",
        cide_ipc::ProxyTarget::Untouched => "untouched",
        cide_ipc::ProxyTarget::Direct => "direct",
    };
    format!("proxy[{kind}]: {target} — {}", env.describe())
}

/// Whether this program is the Claude Code CLI, and so has hooks worth registering.
///
/// Matched on the file name, which is enough because the frontend spawns the bare string
/// `claude` and lets `PATH` resolve it.
///
/// **The limitation is worth stating, because breaking it is silent.** `claude` on this
/// machine resolves to `~/.local/share/claude/versions/2.1.233`, whose file name is a version
/// number and matches nothing here. That is harmless because the resolution happens in the OS,
/// after this decision.
///
/// # The configurable binary, and why this function did not have to change (M16)
///
/// The paragraph above used to end by predicting its own failure: *"the moment anything passes
/// an absolute path as the program — a configurable CLI location in Settings, say — this
/// returns false, no `--settings` is attached, and every session runs with no hooks"*. Settings
/// now has exactly that field, and the prediction is closed by **ordering** rather than by
/// teaching this function about paths.
///
/// `session_spawn` calls this on the program the *frontend* asked for, which is still the bare
/// string `claude` for every Claude pane, and only then substitutes `ClaudeCli::binary` into
/// the spec. So the decision is made from the one value that is reliably a name, and the
/// substitution happens downstream of it.
///
/// Teaching this to recognise a configured path was the alternative and it loses twice: it
/// would have to compare against a setting this function cannot see without a lock it must not
/// take, and it would still answer `false` for `~/.local/share/claude/versions/2.1.233` — the
/// value a user pins when they want a specific version, which is the whole reason the field
/// exists. A change to what is passed as `program` still needs a reader of this note.
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

// The conversation arguments for a Claude child — and the id that child will report — used to
// be built here, as `claude_args`. They now live in `cide_claude::session`, with the shapes,
// the reason a plain resume names no `--session-id`, and the unit tests. They moved because
// they are a fact about the CLI rather than about the Tauri command layer, and because
// `cide-claude` is where an `#[ignore]`d test can put those exact argv shapes in front of the
// installed binary — the check that would have caught 2.1.227 retracting the old ones.

/// The inline `--settings` JSON, or `None` when `cide-hook` cannot be located.
///
/// Looked up beside our own executable, which is where every packaging format this project
/// ships puts the two binaries together.
fn hook_settings(theme: cide_ipc::Theme) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let hook = exe.parent()?.join("cide-hook");
    if !hook.exists() {
        return None;
    }
    let settings = cide_claude::inline_settings(
        &hook.to_string_lossy(),
        &cide_claude::StatusLine::Ours,
        match theme {
            cide_ipc::Theme::Light => cide_claude::ClaudeTheme::Light,
            cide_ipc::Theme::Dark => cide_claude::ClaudeTheme::Dark,
        },
    );
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
    // Read before the blocking closure: `WorkspaceState` is Tauri-managed state and the
    // closure below is `spawn_blocking`, which cannot hold a `State<'_, _>` across the await.
    // One bool's worth of work on the caller's thread, and it decides which way round the
    // child draws itself — see `hook_settings`.
    let theme = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .map(|ws| ws.with(|w| w.settings.theme))
        .unwrap_or_default();

    let mut spec = SpawnSpec::new(program, PathBuf::from(cwd)).geometry(pty_geometry(geometry));
    for a in args {
        spec = spec.arg(a);
    }
    // Only Claude children get the settings payload. A shell has no hooks to register, and
    // handing it a `--settings` argument would simply be a bad argv.
    //
    // **Computed here rather than thirty lines further down, where it used to be.** The proxy
    // pass below now needs it too — `ProxyScope` answers separately for `claude` and for the
    // user's shell — and a scope decided after the environment had already been built would
    // have been a scope that could not reach it.
    //
    // **And it is computed from what the frontend asked for, before the configured binary is
    // substituted below.** That ordering is the whole of the bug `program_is_claude`'s note
    // predicted: decide after substituting, and an absolute path from Settings answers `false`,
    // no `--settings` is attached, and every session runs with no hooks — no token figures, no
    // fast buffer reload, and a close confirm that cannot tell busy from idle. Nothing fails;
    // the features simply are not there.
    let is_claude = program_is_claude(&spec.program);

    // Read once, here, rather than inside the proxy pass: this is the only place that knows
    // both the app handle and that a child is about to exist, and `WorkspaceState::with` runs
    // under a non-reentrant lock that nothing further down should be holding. `try_state`
    // because a test harness may have no workspace, in which case the default — inherit,
    // touch nothing — is the right answer anyway.
    // The Claude environment toggles come out of the same read, for the same reason and at the
    // same cost: one lock acquisition on this thread rather than two, and none at all inside
    // the spawn. Cloned, both of them: `ClaudeSettings` stopped being `Copy` when it grew the
    // launch configuration, which is a `String` and two `Vec`s. One allocation on a path that
    // is about to `fork`.
    let (proxy, claude_settings) = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .map(|state| state.with(|ws| (ws.settings.proxy.clone(), ws.settings.claude.clone())))
        .unwrap_or_default();

    // The user's launch configuration, filtered. Enforced *here* as well as on the Settings
    // screen and not instead of it: `workspace.json` is hand-editable and `settings_set` is one
    // `invoke` away from being bypassed, so a filter that only ran in the UI would let a
    // hand-edited file cost somebody their `--resume`. See `cide_core::claude_cli`.
    //
    // A shell pane gets neither half — see `base_env`'s note on why the env stops here, and
    // note that the arguments have nowhere sensible to go either: `--model opus` handed to
    // `bash` is a login shell that fails to start.
    let plan = if is_claude {
        cide_core::claude_cli::plan_here(&claude_settings.cli)
    } else {
        cide_core::claude_cli::Plan::default()
    };
    // One line, once, naming what was dropped. A hand-edited `workspace.json` is the case this
    // exists for: there is no screen involved in that path, so the log is the only place the
    // refusal can be seen at all.
    for refused in plan.refusals() {
        tracing::warn!(
            token = %refused.text,
            "refusing a claude launch argument or variable: {}",
            refused.verdict.note().unwrap_or_default()
        );
    }

    // The configured binary, substituted after the decision above and never before it.
    //
    // Passed through **exactly as stored**, so a bare `claude` is still resolved by the OS at
    // this spawn rather than pinned to whatever `which` answered at launch — the CLI updates
    // itself underneath a running app. `resolve` below is a *check*; its answer is thrown away.
    if is_claude {
        let configured = claude_settings.cli.binary.trim();
        // Checked before the fork so the failure is a sentence in the pane's own transcript
        // rather than portable-pty's `ENOENT`, which names a file the user never typed. The
        // cost is a `stat` per `PATH` entry, on the caller's thread, once per Claude spawn.
        if let Err(problem) = cide_core::claude_cli::resolve(configured) {
            return Err(SessionError::NoClaudeBinary {
                program: configured.to_string(),
                message: problem.message(),
            });
        }
        spec.program = configured.to_string();
    }

    // The user's arguments go **first**, before every token cide adds.
    //
    // Not last, and the reason is a variadic flag. `--add-dir`, `--mcp-config`, `--allowedTools`
    // and `--tools` all collect every following token that does not begin with `-`, and every
    // argument cide appends below does begin with one (`--session-id`, `--resume`,
    // `--fork-session`, `--settings`). So a user flag placed here can never swallow a uuid,
    // whereas the same flag placed last would swallow whatever cide had already written.
    // `cide_claude::headless::argv` refuses the same wager for the same reason and says so.
    for a in plan.args {
        spec = spec.arg(a);
    }

    let target = pane_proxy_target(&proxy.scope, is_claude);
    let proxy_env = ProxyEnv::for_target(&proxy, target);
    let mut spec = apply_proxy(base_env(spec, &claude_settings, plan.env), &proxy_env);
    // Redacted, and at debug level: one line per spawn is worth it when a pane cannot reach
    // the network, but it is not worth it on every launch of a machine with no proxy at all.
    tracing::debug!(
        "{}",
        proxy_log_line(
            if is_claude { "claude" } else { "shell" },
            target,
            &proxy_env
        )
    );

    // Minted before the spawn, not after, because for a Claude pane this id is *usually* the
    // value passed to `--session-id`. That equality is what makes everything downstream work:
    // a hook reports the CLI's `session_id`, and unless the CLI is using ours, every frame it
    // sends names a uuid this process has never heard of and is dropped. It is also what lets
    // a restored pane resume with `--resume <id>` and no extra bookkeeping.
    //
    // **"Usually", because a plain resume is the exception and it is not ours to choose.**
    // The CLI keeps the parent's id when it is not forking, so `cide_claude::conversation`
    // answers with the id the child will report, and everything below — the exit watcher,
    // the registry key, the value returned to the frontend and therefore
    // `pane_bind_session`'s `workspace.json` entry — uses that instead of the minted one.
    let minted = SessionId::new();
    let mut id = minted;
    if is_claude {
        let (effective, args) = cide_claude::conversation(minted, resume, wants_fork(fork));
        id = effective;
        for a in args {
            spec = spec.arg(a);
        }
    }

    // A plain resume adopts an id the registry may already hold, which no other spawn can do:
    // every other shape mints a fresh uuid. Inserting over a live entry would replace the
    // `Arc<PtySession>` that is the only handle to a running child — nothing could then write
    // to it, kill it, or reap it, and quitting the app would leave it behind. Two panes
    // resuming one conversation is also not a thing the CLI supports; the honest answer is to
    // refuse the second, with a message that says which session and why.
    if id != minted
        && let Some(existing) = registry.get(id)
        && !existing.has_exited()
    {
        return Err(SessionError::AlreadyOpen(id));
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
            match hook_settings(theme) {
                Some(json) => spec = spec.arg("--settings").arg(json),
                // Without an absolute path to `cide-hook` the child cannot run it: its cwd is
                // the project root and its PATH is the user's. Skipping the flag leaves a
                // working session with no hooks, which is the right way to fail here.
                None => tracing::warn!("cannot locate cide-hook; this session reports no state"),
            }
        }
    }

    // What a restored *shell* gets instead of a resume. `resume` on a non-Claude program has
    // never meant `--resume` — `bash` has no such flag and the argument was silently dropped —
    // so it carries the one thing it can honestly carry: "this pane is continuing session X".
    // For a shell that means replaying X's parting screen into the new child's mirror, plus a
    // line saying the text is dead. See `lifecycle::shell_preload`.
    //
    // Naming it `resume` rather than adding a parameter is deliberate. The alternative was a
    // second optional `replay` argument, which would have meant editing `session.spawn`'s
    // options object in the middle of `ui/src/ipc/client.ts` — a file whose house rule is
    // append-only because it has conflicted three rounds running. The field already exists,
    // already means "this pane continues session X", and the two readings differ only in what
    // a program can do with it.
    let replay_for = (!is_claude).then_some(resume).flatten();

    let session = blocking(move || {
        // Inside the blocking closure, not outside it: the first call reads `screens.json`
        // off disk, and every Tauri command that touches a filesystem in this app does it
        // here rather than on the webview's thread.
        if let Some(prior) = replay_for {
            spec = spec.preload(crate::lifecycle::shell_preload(prior));
        }
        PtySession::spawn(spec).map_err(|e| SessionError::Pty(e.to_string()))
    })
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

/// Where this pane's child is *now*, when that is inside the project — otherwise nothing.
///
/// # Why the frontend cannot answer this itself
///
/// A session does not carry its cwd: `PtySession` keeps `child_pid` but not `SpawnSpec.cwd`,
/// and the only cwd the frontend knows is the one the pane was *spawned* with — the project's
/// primary root. That is right for every Claude pane (the CLI does not change directory, and
/// its output is relative to where it started) and wrong for a shell pane the moment somebody
/// types `cd ui`, which is this repository's own documented gate. Without this, every tsc,
/// vite and esbuild diagnostic printed from `ui/` names a file cide cannot find.
///
/// One `read_link` on `/proc/<pid>/cwd`, from the pid `pane_bind_session` already uses to bind
/// the IDE server. The frontend caches the answer for a second, so a fast drag down a build log
/// costs one call and not one per line.
///
/// # What it is not, and why each is acceptable
///
/// * It is the **direct child's** cwd. A `cd` in a subshell, or `make -C`, is invisible.
/// * It is the cwd **now**, not the cwd the line was printed from. The frontend never lets a
///   cwd out-rank a root for that reason: a path that resolves under both comes back ambiguous
///   and opens nothing rather than opening the wrong file.
/// * **It is Linux-only, and off Linux the feature degrades rather than failing.** See
///   [`cwd_of_pid`]: there is no `/proc` on macOS, so this answers `None` for every pane and
///   relative paths in terminal output stop resolving. Nothing errors and nothing is logged per
///   click — the links simply only work for absolute paths, which is the quietest kind of
///   missing feature and is why the arm is written out rather than left to a failing syscall.
///
/// # Why a cwd outside the project is `None` rather than the truth
///
/// The child is untrusted — it is a build, a tool, an agent — and it can `chdir` anywhere it
/// likes. Handing back `/home/you/.ssh` would make it a *base* for resolving relative paths
/// out of that same child's output, which is a way to name files outside the project using
/// nothing but text the child controls. Refusing here is the cheap lock; `terminal_open_path`
/// re-checks containment on the click, which is the real one.
#[tauri::command(rename_all = "camelCase")]
pub fn session_cwd(
    state: State<'_, crate::workspace_state::WorkspaceState>,
    registry: State<'_, SessionRegistry>,
    project: cide_ipc::ProjectId,
    session: SessionId,
) -> Result<Option<PathBuf>, SessionError> {
    let Some(s) = registry.get(session) else {
        // Not an error. A pane asks this while hovering, and a session that has exited or been
        // detached under the pointer is ordinary rather than exceptional.
        return Ok(None);
    };
    let Some(pid) = s.child_pid() else {
        return Ok(None);
    };
    let roots: Vec<PathBuf> = state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|p| p.roots.iter().map(|r| r.path.clone()).collect())
            .unwrap_or_default()
    });
    Ok(contained_cwd(pid, &roots))
}

/// The body of [`session_cwd`], as a free function over a pid and the roots.
///
/// Split out so the two things that can actually be got wrong here — reading `/proc` at all,
/// and refusing a cwd outside the project — are exercised against a real process in the test at
/// the foot of this file, rather than only against a running `claude`.
fn contained_cwd(pid: u32, roots: &[PathBuf]) -> Option<PathBuf> {
    contain(cwd_of_pid(pid)?, roots)
}

/// The containment half of [`contained_cwd`], over a cwd that has already been read.
///
/// Separated from the reading so the security-relevant rule is testable on **every** host, which
/// is what [`cwd_of_pid`]'s comment below already claims and what the code did not deliver: with
/// the two fused, every containment assertion off Linux ran against a `cwd_of_pid` that returns
/// `None`, so each one passed by getting `None` for the wrong reason. A refusal test that cannot
/// tell "refused because it was outside the project" from "refused because this platform cannot
/// look" is not evidence of anything, and it is the shape a reader is least likely to doubt.
fn contain(cwd: PathBuf, roots: &[PathBuf]) -> Option<PathBuf> {
    // A deleted working directory reads back as `/path (deleted)`, which is neither a directory
    // nor a path anybody has — `is_dir` refuses that and a cwd that has since been removed with
    // one syscall.
    if !cwd.is_dir() {
        return None;
    }
    if cide_fs::ops::check_within(roots, &cwd).is_err() {
        return None;
    }
    Some(cwd)
}

/// A process's current working directory, on a platform that can be asked.
///
/// Split from [`contained_cwd`] so that the platform question and the containment question are
/// separable: the containment rule is the security-relevant half and is the same everywhere, so
/// it stays platform-independent and keeps its tests on every host. Only the *reading* is
/// per-platform.
#[cfg(target_os = "linux")]
fn cwd_of_pid(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Off Linux there is nothing to read, and this returns `None` deliberately rather than by
/// accident.
///
/// **The accident is what was here before.** The `/proc` `read_link` above compiled everywhere
/// and simply failed on macOS, where there is no `/proc` at all — so the whole feature switched
/// itself off with no arm, no comment and no log line. What the user would see is not an error
/// but a *narrower* set of working terminal links: `src/main.rs` printed by a build running in a
/// subdirectory stops opening anything, while `/home/you/p/src/main.rs` still does. Diagnosing
/// that from the outside means knowing this function exists.
///
/// **What macOS needs.** `libproc`'s `proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &info, size)`,
/// whose `pvi_cdir.vip_path` is the answer — about twenty lines of `libc` plus a `#[repr(C)]`
/// struct, or the `libproc` crate. Two things stop it being written here: it cannot be compiled,
/// let alone run, on this machine, and it is the *permission* model that decides whether it is
/// worth having — `PROC_PIDVNODEPATHINFO` on another user's process needs root, and whether it
/// answers for a same-user child under macOS's hardened runtime is exactly the sort of thing
/// that has to be observed rather than read. Written down in `README.md` under Platforms.
///
/// The BSDs want a third answer again (`sysctl KERN_PROC_CWD`), which is why this is
/// `not(target_os = "linux")` rather than a macOS arm: everything that is not Linux is honestly
/// unimplemented here, not merely untested.
#[cfg(not(target_os = "linux"))]
fn cwd_of_pid(_pid: u32) -> Option<PathBuf> {
    None
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

/// Whether this pane could resume `session` — i.e. whether Claude Code still holds its
/// transcript.
///
/// Asked by a pane whose child has just died, to decide whether the bar it puts over the dead
/// terminal offers *Resume this conversation* as well as *Start a new session*. The same
/// question [`crate::lifecycle::plan_restore`] answers for every pane at launch, asked for one
/// pane at a moment when the launch plan is long stale.
///
/// **Not a registry question**, which is why it takes a `cwd` rather than reading one: a
/// transcript outlives the process that wrote it, and after a restart the registry has never
/// heard of the id at all. `cwd` is the directory the session was spawned in, which is the
/// project root the pane already passes to [`session_spawn`].
///
/// `false` for every way of being unable to tell. A wrong `false` costs a button; a wrong
/// `true` costs a `claude --resume` that fails in front of the user.
#[tauri::command(rename_all = "camelCase")]
pub fn session_resumable(cwd: String, session: SessionId) -> bool {
    crate::lifecycle::resumable(std::path::Path::new(&cwd), session)
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
            cide_claude::conversation(id, None, wants_fork(None)),
            (id, vec!["--session-id".to_string(), id.to_string()]),
            "an ordinary pane spawns plain"
        );
    }

    #[test]
    fn the_id_this_command_registers_is_the_one_the_child_will_report() {
        // The wiring, which is this file's half of the fix — the argument shapes and their
        // reasoning are `cide_claude::session`'s, and tested there.
        //
        // `session_spawn` keys the registry, the exit watcher and its own return value (and so
        // `pane_bind_session`, and so `workspace.json`) on the *effective* id rather than the
        // minted one. On a plain resume those differ, and using the minted one gives a session
        // that exists and nothing can find: hook frames name the parent, so no status, no
        // token figures, no busy-vs-idle close confirm — and a saved workspace entry with no
        // transcript behind it, so the next launch has nothing to resume either.
        let minted = SessionId::new();
        let parent = SessionId::new();
        assert_eq!(
            cide_claude::conversation(minted, Some(parent), wants_fork(None)).0,
            parent,
            "a plain resume runs under the parent's id"
        );
        assert_eq!(
            cide_claude::conversation(minted, Some(parent), wants_fork(Some(true))).0,
            minted,
            "a fork runs under the id we minted, because `--session-id` is passed"
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

    use cide_ipc::{ProxyMode, ProxyScope, ProxySettings};

    /// A bare spec, so a test asserts on exactly what the proxy pass added.
    ///
    /// `base_env` is deliberately not applied: mixing its dozen entries in would make every
    /// assertion below a search through noise, and the two passes compose by construction —
    /// the proxy fold only ever appends.
    fn value_of<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The bundle scrub reaches a child as sets *and* removals, and neither may be dropped.
    ///
    /// `cide-core` decides which is which — including the case that matters most, a variable
    /// whose bundle entries were the only ones it had — and its own tests cover that rule.
    /// What is only checkable here is that both halves survive the trip into a `SpawnSpec`,
    /// since a fold that quietly handled one arm would leave `PYTHONHOME` on a pane's child
    /// and nothing would say so.
    #[test]
    fn a_bundle_scrub_reaches_the_spec_as_both_sets_and_removals() {
        let spec = apply_env_changes(
            SpawnSpec::new("/bin/sh", std::env::temp_dir()),
            [
                ("PYTHONHOME".to_string(), None),
                ("PATH".to_string(), Some("/usr/bin".to_string())),
            ],
        );
        assert_eq!(value_of(&spec, "PATH"), Some("/usr/bin"));
        assert!(
            spec.env_remove.iter().any(|k| k == "PYTHONHOME"),
            "removed {:?}",
            spec.env_remove
        );
    }

    /// Nothing to scrub must mean nothing added, so a non-bundled launch is byte-identical.
    #[test]
    fn an_empty_scrub_leaves_the_spec_alone() {
        let spec = apply_env_changes(SpawnSpec::new("/bin/sh", std::env::temp_dir()), []);
        assert!(spec.env.is_empty(), "set {:?}", spec.env);
        assert!(spec.env_remove.is_empty(), "removed {:?}", spec.env_remove);
    }

    // --- the Claude settings reach a real spec --------------------------------------------
    //
    // `cide_core::child_env::claude_env` decides *what* the switches mean and has its own
    // tests. What is only checkable here is that `base_env` folds that list at all — which is
    // the exact step that did not exist: for three releases the switches were declared,
    // persisted, bound to TypeScript and drawn on screen under a panel promising they were
    // "applied at spawn", and no code anywhere read them. A rule with no call site is this
    // project's most-repeated defect, and it is invisible to every test of the rule itself.

    /// The switches survive the trip into the spec a pane is actually spawned from.
    #[test]
    fn base_env_carries_the_claude_settings_into_the_spec() {
        let spec = base_env(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &cide_ipc::ClaudeSettings {
                disable_alternate_screen: true,
                scroll_speed: 12,
                ..Default::default()
            },
            Vec::new(),
        );
        assert_eq!(
            value_of(&spec, "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"),
            Some("1"),
            "the alternate-screen switch is the one a user chasing a scrollable transcript \
             presses; if it does not reach the spec it reaches nothing at all"
        );
        assert_eq!(
            value_of(&spec, "CLAUDE_CODE_SCROLL_SPEED"),
            Some("12"),
            "and the scroll rate is the user's number, not the constant this used to hardcode"
        );
    }

    /// A switch left off removes its variable from the child rather than ignoring it.
    ///
    /// Asserted at *this* end and not only in `cide-core` because the two arms of an
    /// `EnvChange` land in two different fields of a `SpawnSpec`, and a fold that dropped the
    /// removal arm would leave a user's exported `CLAUDE_CODE_DISABLE_MOUSE=1` in force under
    /// a switch sitting at off.
    #[test]
    fn a_claude_switch_left_off_reaches_the_spec_as_a_removal() {
        let spec = base_env(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &cide_ipc::ClaudeSettings::default(),
            Vec::new(),
        );
        for var in [
            "CLAUDE_CODE_DISABLE_MOUSE",
            "CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT",
            "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN",
        ] {
            assert!(
                spec.env_remove.iter().any(|k| k == var),
                "{var} is off, so it must be removed from the inherited environment; \
                 removed {:?}",
                spec.env_remove
            );
            assert!(
                value_of(&spec, var).is_none(),
                "{var} is off and must not also be set — the CLI reads any defined value as on"
            );
        }
    }

    /// The settings pass runs after the constants, so a user's value is the one that survives.
    ///
    /// The ordering is the whole reason `claude_env` is folded last rather than first. Were it
    /// first, the literal `CLAUDE_CODE_SCROLL_SPEED` this function used to carry would have
    /// overwritten it, and the control would have moved a number nothing read.
    #[test]
    fn the_settings_pass_wins_over_the_constants_above_it() {
        let spec = base_env(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &cide_ipc::ClaudeSettings {
                scroll_speed: 9,
                ..Default::default()
            },
            Vec::new(),
        );
        // `SpawnSpec::env` is applied in order, so the *last* entry for a name is the one the
        // child gets; asserting on the last is asserting on what the child sees.
        let last = spec
            .env
            .iter()
            .rfind(|(k, _)| k == "CLAUDE_CODE_SCROLL_SPEED")
            .map(|(_, v)| v.as_str());
        assert_eq!(last, Some("9"));
    }

    /// A shell pane gets them too, and that is deliberate.
    ///
    /// `CLAUDE_CODE_*` means nothing to `bash`, and a user who types `claude` at a shell
    /// pane's prompt is running the same program with the same settings. Gating the pass on
    /// `program_is_claude` was the tempting version and would have made the Settings screen
    /// true for panes cide spawned and false for the identical session started by hand.
    #[test]
    fn a_shell_pane_is_given_the_same_switches() {
        let spec = base_env(
            SpawnSpec::new("/bin/bash", std::env::temp_dir()),
            &cide_ipc::ClaudeSettings {
                disable_mouse: true,
                ..Default::default()
            },
            Vec::new(),
        );
        assert_eq!(value_of(&spec, "CLAUDE_CODE_DISABLE_MOUSE"), Some("1"));
    }

    // --- M16: the user's own variables ------------------------------------------------------

    /// The launch configuration's environment reaches the spec, and reaches it **last**.
    ///
    /// Last is the whole point: `TERM` and the `CLAUDE_CODE_*` names are refused precisely
    /// *because* a user's value would win here, so the two rules only compose if this fold runs
    /// after both of the ones above it. A fold placed first would silently invert the refusal
    /// list's justification while every test of the list itself kept passing.
    #[test]
    fn the_launch_configurations_variables_are_folded_after_everything_cide_assumed() {
        let plan = cide_core::claude_cli::plan(
            &cide_ipc::ClaudeCli {
                env: vec![
                    cide_ipc::ClaudeEnvVar {
                        name: "MY_MCP_TOKEN".into(),
                        value: "hunter2".into(),
                    },
                    // Refused, so it must not appear at all — and the refusal has to bite here
                    // rather than only on the screen, because `workspace.json` is hand-editable.
                    cide_ipc::ClaudeEnvVar {
                        name: "ANTHROPIC_API_KEY".into(),
                        value: "sk-ant-nope".into(),
                    },
                    cide_ipc::ClaudeEnvVar {
                        name: "TERM".into(),
                        value: "xterm".into(),
                    },
                ],
                ..Default::default()
            },
            None,
        );
        let spec = base_env(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &cide_ipc::ClaudeSettings::default(),
            plan.env,
        );
        assert_eq!(value_of(&spec, "MY_MCP_TOKEN"), Some("hunter2"));
        assert_eq!(
            value_of(&spec, "ANTHROPIC_API_KEY"),
            None,
            "a key outranks subscription OAuth and would bill a Console org for a Max user"
        );
        assert_eq!(
            spec.env
                .iter()
                .rfind(|(k, _)| k == "TERM")
                .map(|(_, v)| v.as_str()),
            Some("xterm-256color"),
            "cide's TERM survives, which it only does because the name is refused — this fold \
             is last, so an accepted TERM would have overwritten it"
        );
    }

    /// The pass is fed an empty list for a shell pane, and this is the assertion that says the
    /// gate is *at the call site* rather than inside the fold.
    ///
    /// `base_env` itself is unconditional and must stay so: it is folded for every pane, and a
    /// `if is_claude` buried in here would be a second place the decision lives. What
    /// `session_spawn` does is hand it `Plan::default()`, whose `env` is empty.
    #[test]
    fn base_env_folds_whatever_it_is_handed_and_decides_nothing() {
        let spec = base_env(
            SpawnSpec::new("/bin/bash", std::env::temp_dir()),
            &cide_ipc::ClaudeSettings::default(),
            cide_core::claude_cli::Plan::default().env,
        );
        assert_eq!(
            value_of(&spec, "MY_MCP_TOKEN"),
            None,
            "an empty plan adds nothing, which is what a shell pane is given"
        );
    }

    /// The pane's own transcript is where a bad binary is read, and `spawnFailureText` prefers
    /// a tagged `message` verbatim — so the message has to survive serialization intact.
    #[test]
    fn a_missing_binary_serializes_as_its_own_kind_with_a_sentence() {
        let error = SessionError::NoClaudeBinary {
            program: "cluade".into(),
            message: cide_core::claude_cli::BinaryProblem::NotOnPath {
                name: "cluade".into(),
            }
            .message(),
        };
        let json = serde_json::to_value(&error).expect("serializes");
        assert_eq!(json["kind"], "noClaudeBinary");
        let message = json["message"].as_str().expect("a message");
        assert!(
            message.contains("cluade") && message.contains("Settings"),
            "the sentence has to name the value and where to correct it: {message}"
        );
    }

    fn manual(http: &str, https: &str, all: &str, no_proxy: &str) -> ProxySettings {
        ProxySettings {
            mode: ProxyMode::Manual,
            scope: ProxyScope::default(),
            http: http.into(),
            https: https.into(),
            all: all.into(),
            no_proxy: no_proxy.into(),
        }
    }

    // --- what moved, and what is left here to check ---------------------------------------
    //
    // The proxy *rule* — six variables, two spellings, the HTTPS fallback, the loopback
    // exemption, the empty-variable trap — now lives in `cide_core::proxy` because three spawn
    // sites in three crates need the same answer, and its seventeen tests went with it.
    // Re-asserting it here would be asserting a constant against itself.
    //
    // Three things are only checkable at *this* end, and they are what is below:
    //
    //   * the resolved changes reach a `SpawnSpec` as sets **and** removals, since the fold is
    //     the one arm `cide-core` cannot write;
    //   * the right column of `ProxyScope` is picked for the child actually being spawned,
    //     which is a decision about `claude`-versus-`$SHELL` that only this file makes;
    //   * the log line names the target, because "cide set nothing for this kind of child" is
    //     now one of the answers to "why can this pane not reach the network".

    /// A removal is not a set, and the difference decides whether a pane is on a proxy.
    ///
    /// The fold is three lines and would be right by inspection, which is exactly the class
    /// of code that ships wrong: a `Direct` scope produces *only* removals, so a fold that
    /// dropped that arm would leave every inherited proxy in place while the screen said the
    /// child was direct.
    #[test]
    fn a_direct_scope_reaches_the_spec_as_removals_rather_than_as_nothing() {
        let proxy = manual("http://proxy.corp:3128", "", "", "");
        let env = ProxyEnv::resolve(&proxy, cide_ipc::ProxyTarget::Direct, |_| None);
        let spec = apply_proxy(SpawnSpec::new("/bin/sh", std::env::temp_dir()), &env);

        for name in ["HTTP_PROXY", "http_proxy", "NO_PROXY", "no_proxy"] {
            assert!(
                spec.env_remove.iter().any(|k| k == name),
                "{name} was not removed: {:?}",
                spec.env_remove
            );
        }
        assert!(spec.env.is_empty(), "nothing is set: {:?}", spec.env);
    }

    /// And a configured one reaches it as sets, in both spellings.
    #[test]
    fn a_configured_scope_reaches_the_spec_as_sets_in_both_spellings() {
        let proxy = manual("proxy.corp:3128", "", "", "");
        let env = ProxyEnv::resolve(&proxy, cide_ipc::ProxyTarget::Configured, |_| None);
        let spec = apply_proxy(SpawnSpec::new("/bin/sh", std::env::temp_dir()), &env);

        assert_eq!(
            value_of(&spec, "HTTP_PROXY"),
            Some("http://proxy.corp:3128")
        );
        assert_eq!(
            value_of(&spec, "http_proxy"),
            Some("http://proxy.corp:3128")
        );
    }

    /// An out-of-scope child gets a spec the proxy pass did not touch at all.
    ///
    /// Byte-identical, not merely proxy-free: `Untouched` means cide inherits its own
    /// environment onward, and a spurious `NO_PROXY` added here would be a behaviour change
    /// for whichever child the user had just taken *out* of scope.
    #[test]
    fn an_untouched_child_gets_a_spec_the_proxy_pass_did_not_write_to() {
        let proxy = manual("http://proxy.corp:3128", "", "", "corp.internal");
        let env = ProxyEnv::resolve(&proxy, cide_ipc::ProxyTarget::Untouched, |_| None);
        let spec = apply_proxy(SpawnSpec::new("/bin/sh", std::env::temp_dir()), &env);

        assert!(spec.env.is_empty(), "set {:?}", spec.env);
        assert!(spec.env_remove.is_empty(), "removed {:?}", spec.env_remove);
    }

    /// The pane's column of the scope, and the reason `is_claude` had to move up the
    /// function: a shell and a `claude` in the same window can now be answered differently.
    #[test]
    fn a_claude_pane_and_a_shell_pane_read_different_columns_of_the_scope() {
        let scope = ProxyScope {
            claude: cide_ipc::ProxyTarget::Configured,
            shells: cide_ipc::ProxyTarget::Direct,
            git: cide_ipc::ProxyTarget::Untouched,
        };
        assert_eq!(
            pane_proxy_target(&scope, true),
            cide_ipc::ProxyTarget::Configured
        );
        assert_eq!(
            pane_proxy_target(&scope, false),
            cide_ipc::ProxyTarget::Direct
        );
        // And the git column is nobody's pane. A pane that read it would put the Git tool
        // window's answer onto the user's shell.
        assert_ne!(pane_proxy_target(&scope, true), scope.git);
        assert_ne!(pane_proxy_target(&scope, false), scope.git);
    }

    /// The default scope, spelled out where a pane spawn can see it: both pane kinds are
    /// proxied exactly as they were before `ProxyScope` existed.
    #[test]
    fn the_default_scope_leaves_both_pane_kinds_where_they_were() {
        let scope = ProxyScope::default();
        for is_claude in [true, false] {
            assert_eq!(
                pane_proxy_target(&scope, is_claude),
                cide_ipc::ProxyTarget::Configured,
                "is_claude={is_claude}"
            );
        }
    }

    /// A credentialed proxy reaches the child intact and the log line never sees it.
    #[test]
    fn a_credentialed_proxy_never_reaches_a_log_line() {
        let proxy = manual(
            "http://alice:hunter2@proxy.corp:3128",
            "",
            "socks5://bob:s3cret@socks.corp:1080",
            "",
        );
        let env = ProxyEnv::resolve(&proxy, cide_ipc::ProxyTarget::Configured, |_| None);

        // The child gets it whole — a redacted value would simply be a proxy that cannot
        // authenticate.
        let spec = apply_proxy(SpawnSpec::new("/bin/sh", std::env::temp_dir()), &env);
        assert_eq!(
            value_of(&spec, "HTTP_PROXY"),
            Some("http://alice:hunter2@proxy.corp:3128")
        );

        let line = proxy_log_line("claude", cide_ipc::ProxyTarget::Configured, &env);
        for printed in [line.clone(), format!("{proxy:?}"), format!("{env:?}")] {
            assert!(!printed.contains("hunter2"), "password leaked: {printed}");
            assert!(!printed.contains("s3cret"), "password leaked: {printed}");
            assert!(!printed.contains("alice"), "username leaked: {printed}");
        }
        assert!(
            line.contains("proxy.corp:3128"),
            "the host is what makes the line worth having: {line}"
        );
    }

    /// The log line names which column answered, including when the answer was "nothing".
    ///
    /// Without the target in it, an untouched child and a machine with no proxy configured
    /// produce the same line, and they are the two states a reader most needs to tell apart.
    #[test]
    fn the_log_line_names_the_target_even_when_nothing_was_done() {
        let proxy = manual("http://proxy.corp:3128", "", "", "");

        let untouched = ProxyEnv::resolve(&proxy, cide_ipc::ProxyTarget::Untouched, |_| None);
        let line = proxy_log_line("shell", cide_ipc::ProxyTarget::Untouched, &untouched);
        assert!(line.contains("shell"), "{line}");
        assert!(line.contains("untouched"), "{line}");

        let direct = ProxyEnv::resolve(&proxy, cide_ipc::ProxyTarget::Direct, |_| None);
        let line = proxy_log_line("claude", cide_ipc::ProxyTarget::Direct, &direct);
        assert!(line.contains("claude"), "{line}");
        assert!(line.contains("direct"), "{line}");
        assert!(
            line.contains("<removed>"),
            "a scrub is a removal and the line has to say so: {line}"
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

    // --- session_cwd -----------------------------------------------------------------------

    /// The containment rule, on every host.
    ///
    /// This is the half that matters — the child chooses its own cwd, so a `chdir` into `~/.ssh`
    /// must not become a base for resolving relative paths out of that same child's output — and
    /// it is identical on every platform, so it is driven through [`contain`] with the cwd handed
    /// in rather than through `contained_cwd`, which would first have to read `/proc`. Before the
    /// split these assertions lived in the `/proc` test below and passed off Linux for the wrong
    /// reason: `cwd_of_pid` answers `None` there, so every refusal was already `None` before the
    /// rule was consulted, and the two accepting cases failed outright.
    #[test]
    fn a_cwd_is_reported_only_when_it_is_inside_the_project() {
        let here = std::env::current_dir().expect("a cwd");

        assert_eq!(
            contain(here.clone(), std::slice::from_ref(&here)),
            Some(here.clone()),
            "a cwd inside a root is exactly what the resolver wants"
        );

        let parent = here.parent().expect("a parent").to_path_buf();
        assert_eq!(
            contain(here.clone(), std::slice::from_ref(&parent)),
            Some(here.clone()),
            "and being *under* a root, not equal to it, is the ordinary case"
        );

        let elsewhere = std::env::temp_dir();
        assert_eq!(
            contain(here.clone(), std::slice::from_ref(&elsewhere)),
            None,
            "a cwd outside every root is refused rather than reported: it is a base a program \
             could choose, and choosing it is how output names files outside the project"
        );

        assert_eq!(
            contain(here.clone(), &[]),
            None,
            "a project with no roots contains nothing"
        );

        // The `(deleted)` suffix a removed cwd reads back with, and any other path that is not a
        // directory: refused before containment is even asked, so a root that happens to be a
        // prefix of the string cannot let it through.
        assert_eq!(
            contain(here.join("no-such-directory"), std::slice::from_ref(&here)),
            None,
            "a cwd that is not a directory is refused even inside a root"
        );
    }

    /// And that the reading half really does answer, where there is a `/proc` to read.
    ///
    /// Driven against *this* process, which is the only pid a test can be sure exists. Gated to
    /// Linux because that is the only platform [`cwd_of_pid`] is implemented on; the arm below
    /// covers everywhere else, so neither platform is left with a silently absent assertion.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_cwd_is_read_from_proc() {
        let here = std::env::current_dir().expect("a cwd");
        assert_eq!(
            cwd_of_pid(std::process::id()),
            Some(here.clone()),
            "/proc/<pid>/cwd is what makes a relative path in a child's output resolvable"
        );
        assert_eq!(
            contained_cwd(std::process::id(), std::slice::from_ref(&here)),
            Some(here)
        );
    }

    /// Off Linux the answer is `None` **by design**, and this is the guard on that being noticed.
    ///
    /// It looks like a test of nothing. It is a tripwire: the day somebody implements
    /// `cwd_of_pid` for macOS with `proc_pidinfo`/`PROC_PIDVNODEPATHINFO` — the twenty lines its
    /// comment sketches — this fails, and whoever wrote them is sent to re-enable the real
    /// assertions above instead of shipping a feature no test covers on the platform that just
    /// gained it. Without it the new code would be silently untested exactly where it is new.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_child_cwd_cannot_be_read_off_linux_and_says_so_by_answering_nothing() {
        assert_eq!(
            cwd_of_pid(std::process::id()),
            None,
            "cwd_of_pid is unimplemented off Linux; if this now answers, the test above it is \
             the one to turn on — see README.md's Platforms section"
        );
        let here = std::env::current_dir().expect("a cwd");
        assert_eq!(
            contained_cwd(std::process::id(), std::slice::from_ref(&here)),
            None,
            "so terminal links resolve only from absolute paths on this platform"
        );
    }

    /// A pid nothing holds is `None`, not an error: a pane asks this on hover and its session
    /// can have exited under the pointer.
    #[test]
    fn a_dead_pid_answers_nothing_at_all() {
        // The kernel's own maximum plus one cannot name a live process.
        let impossible = u32::MAX;
        assert_eq!(contained_cwd(impossible, &[std::env::temp_dir()]), None);
    }
}
