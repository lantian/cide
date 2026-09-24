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

/// Environment every PTY child gets, folded into the spec it will be spawned from.
///
/// **The list is composed in [`cide_core::child_env::terminal_child_env`], and its long note is
/// the one to read**: the ordering of the passes, why `TERM=xterm-256color` and not
/// `xterm`, why `ANTHROPIC_API_KEY` is deliberately absent, why the `CLAUDE_CODE_*` switches are
/// folded late, and why `user_env` — alone among them — stops at a shell pane. All of it moved
/// there in M18 rather than being copied, because a subagent run needs the identical list and
/// `cide-agents` cannot reach into `cide-app`.
///
/// What is left here is the fold, which is exactly the half `cide-core` cannot do: it does not
/// link `cide-pty`, and must not. `SpawnSpec::apply` is the one implementation of it.
///
/// `CARGO_PKG_VERSION` is read *here* and passed in, so `TERM_PROGRAM_VERSION` keeps reporting
/// the application's version rather than whichever crate happened to compose the list.
///
/// The proxy variables are still a separate pass applied on top of this one — see
/// [`apply_proxy`] — and everything after that (`CLAUDE_CODE_SSE_PORT`, `CIDE_HOOK_SOCK`) is
/// added by `session_spawn` itself, in an order that is load-bearing and documented there.
fn base_env(
    spec: SpawnSpec,
    claude: &cide_ipc::ClaudeSettings,
    user_env: Vec<cide_core::child_env::EnvChange>,
) -> SpawnSpec {
    spec.apply(cide_core::child_env::terminal_child_env(
        claude,
        env!("CARGO_PKG_VERSION"),
        user_env,
    ))
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
    spec.apply(env.changes().to_vec())
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

/// The `cide-hook` binary, if it is where every packaging format this project ships puts it.
///
/// Beside `current_exe()`, and **absolute**, which is the whole point: the child's cwd is the
/// project root and its `PATH` is the user's, so a bare `cide-hook` handed to `--mcp-config`
/// resolves to nothing and the CLI reports `CONNECTION_CLOSED` from a process three levels below
/// anything cide logs. `hook_settings` makes the same assumption and performs the same `exists()`
/// check inline, for the same reason: a path that is merely *predicted* fails inside the child.
fn cide_hook_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let hook = exe.parent()?.join("cide-hook");
    hook.exists().then_some(hook)
}

/// The inline `--mcp-config` JSON attaching cide's own MCP server, or `None` when `cide-hook`
/// cannot be located.
///
/// # An inline string, not a file
///
/// `claude --help` documents `--mcp-config <configs...>` as *"Load MCP servers from JSON files or
/// strings"*, and the string form was verified end to end against the installed CLI — the model
/// called a probe server's tool. So nothing is written into the user's project and nothing is left
/// behind if cide is killed, which a temp file would be.
///
/// # The name is what the model sees
///
/// The CLI namespaces a server's tools as `mcp__<server>__<tool>`, so calling this server `cide`
/// is what makes the vocabulary arrive as `mcp__cide__cide_task_list`. That is the spelling to use
/// in an `--allowedTools` line or in prose; the bare `cide_task_list` never appears on the model's
/// side of the wire.
///
/// # `--strict-mcp-config` is deliberately absent
///
/// It would drop every MCP server the *user* configured, silently, in exchange for cide's one.
/// Attaching a tracker is not a reason to take somebody's own tooling away from their session.
fn agent_mcp_config() -> Option<String> {
    let hook = cide_hook_binary()?;
    // Built with serde rather than formatted, so a path containing a quote or a backslash — a
    // build directory under a name with an apostrophe in it — cannot produce a config the CLI
    // parses as something else.
    serde_json::to_string(&serde_json::json!({
        "mcpServers": {
            "cide": {
                "command": hook.to_string_lossy(),
                // `cide-hook mcp` is the bridge: stdio in, `$CIDE_AGENT_SOCK` out, and no
                // knowledge of the vocabulary at all. See its module doc.
                "args": ["mcp"],
            }
        }
    }))
    .ok()
}

/// Append cide's `--mcp-config` to a Claude pane's argv, if the injection is still on.
///
/// # Why a function rather than four lines at the spawn
///
/// `session_spawn` needs an `AppHandle`, a live `AgentRpcServer` and a real PTY, so the gate
/// inside it cannot be driven by a test — and a gate nothing exercises is one that will be
/// wrong the first time somebody edits around it. This is the same shape `base_env` has and for
/// the same reason: the decision is here, the ingredients are handed in.
///
/// `config` is a closure and not a value, for the reason the `--settings` block at the spawn
/// records: `agent_mcp_config` stats `cide-hook` beside the running binary, and a switched-off
/// injection must not pay that on every spawn — which is why `hook_settings` moved inside its
/// own gate.
///
/// A pane whose user switched this off is **silent** here, exactly as a pane with no
/// `--settings` is: the toggle in `ClaudeCliSection.tsx` states what it costs, and a warning
/// per spawn for a setting somebody chose is noise. The `None` arm is not the same thing — no
/// `cide-hook` beside the running binary is a packaging failure nobody chose, and it is the
/// only one of the two that gets a line.
fn with_task_tools(
    spec: SpawnSpec,
    inject: &cide_core::claude_cli::Injected,
    config: impl FnOnce() -> Option<String>,
) -> SpawnSpec {
    let Some(flag) = inject.flag(cide_core::claude_cli::Injection::McpConfig) else {
        return spec;
    };
    match config() {
        Some(json) => spec.arg(flag).arg(json),
        None => {
            tracing::warn!("cannot locate cide-hook; this session gets no task tools");
            spec
        }
    }
}

// ==========================================================================================
// The roster paragraph: telling a project's Claude panes about its subagents — the console
// that it is the product owner, every other pane that it may orchestrate as one. (M40 widened it
// from the console alone, with `agent_rpc`'s tool scope.)
// ==========================================================================================

/// The flag that cannot coexist with the one this file adds.
///
/// Spelled here rather than reached out of `cide_core::claude_cli::WARNED_ARGS`, deliberately.
/// The *behaviour* — add nothing when the user has set this — must not depend on a table entry
/// staying put, because deleting the row would silently turn the degradation back into a pane
/// that fails to start. The row's job is to *explain* the degradation on the Settings screen, and
/// `the_warned_row_and_this_file_still_describe_the_same_degradation` is what keeps the two
/// together.
const APPEND_SYSTEM_PROMPT_FILE: &str = "--append-system-prompt-file";

/// Does the user's launch configuration carry `--append-system-prompt-file`, in either spelling?
///
/// # Why this exists, measured
///
/// On 2.1.235 the CLI refuses `--append-system-prompt` and `--append-system-prompt-file` together
/// **outright** — `Error: Cannot use both --append-system-prompt and --append-system-prompt-file.
/// Please use only one.` — and the pane never starts. `fold_append_system_prompt` cannot repair
/// it: it folds two occurrences of *one* flag into one, and these are two mutually exclusive
/// flags with nothing to fold into. So the decision has to be made here, at the call site that
/// adds the paragraph, and the honest answer is to **degrade rather than refuse**: cide drops its
/// own paragraph, the user's file is read in full, the pane starts, and the roster still reaches
/// the model through the tool descriptions (`cide_agents::tools::description`), which are what
/// actually make it call `cide_agents_list`. Refusing the argument instead would take away a
/// field that works today to protect an addition of cide's own.
///
/// The `=` spelling is split the way `RefusedArg::matches` splits it, and matched on the **whole**
/// token: a `starts_with` here would also match `--append-system-prompt`, which is the flag this
/// must not confuse it with.
fn carries_append_system_prompt_file(args: &[String]) -> bool {
    args.iter().any(|token| {
        token
            .split_once('=')
            .map_or(token.as_str(), |(name, _)| name)
            == APPEND_SYSTEM_PROMPT_FILE
    })
}

/// The system-prompt paragraph for a project's product-owner pane, or `None`.
///
/// `None` for every pane that is not the project's primary Claude pane, and for every project
/// whose `.cide/config.json` does not say `enabled: true` — which is almost all of them, and is
/// the default `cide_agents::config`'s module header calls the single most important line in that
/// crate. A pane that gets no paragraph is a pane whose argv is byte for byte what it was before
/// M18.
///
/// # Namespaced tool names, measured
///
/// The names here are `mcp__cide__cide_agent_dispatch`, not `cide_agent_dispatch`. The CLI
/// namespaces an MCP server's tools as `mcp__<server>__<tool>` — verified end to end against the
/// installed CLI while `agent_mcp_config` was written — so a paragraph naming the bare form would
/// be telling the model about tools it cannot see under that name.
///
/// # Why the roles are named here at all
///
/// `cide_agents_list` answers this better and stays current, and the paragraph is fixed at spawn.
/// Naming them anyway is what makes the model *ask*: a session told "you have roles" with no
/// names has no reason to spend a tool call finding out, and this channel is the only one that
/// arrives before the first turn. The paragraph therefore names the roles **and** the tool that
/// re-reads them, and says which of the two is current.
fn orchestrator_paragraph(
    app: &tauri::AppHandle,
    registry: &SessionRegistry,
    project: cide_ipc::ProjectId,
    resume: Option<SessionId>,
    forking: bool,
    // `Some(Voice::Acting)` for a tab cide opened itself; `None` for every spawn the webview
    // asks for, which is inferred from the tree below exactly as it always was. An override
    // rather than a widened predicate because `is_primary_console_spawn` answers a question
    // about the *console*, and this pane is not one and must not be mistaken for one.
    voice: Option<Voice>,
) -> Option<String> {
    let state = app.try_state::<crate::workspace_state::WorkspaceState>()?;
    // One lock acquisition, and the disk read happens after it is released: `WorkspaceState::with`
    // runs under a non-reentrant lock and `load_project` below opens a directory.
    // Every Claude pane of the project gets the paragraph since M40, because every one of them
    // is served the orchestration tools (`agent_rpc`'s scope table) and a pane with tools and no
    // manual is the model guessing. What the console predicate still decides is the *opening
    // sentence*: the product owner is told it is, and any other pane is told it may act as one.
    let (root, inferred) = state.with(|ws| {
        let primary = is_primary_console_spawn(ws, registry, project, resume, forking);
        let root = cide_core::workspace::project(ws, project)
            .ok()?
            .roots
            .first()
            .map(|root| root.path.clone())?;
        Some((
            root,
            if primary {
                Voice::ProductOwner
            } else {
                Voice::Pane
            },
        ))
    })?;

    // Read fresh, here, for `cide_agents`' stated reason: `.cide/*` is committed, so a teammate's
    // commit or a `git checkout` changes it under a running app and nothing caches it. It costs a
    // `read_dir`, a handful of small files and one `PATH` walk — on this thread, at most once per
    // console pane spawn, which is once per launch or restart. `claude_cli::resolve` above already
    // pays a comparable price on the same thread for the same reason: the alternative is a pane
    // that starts before cide knows what to tell it.
    let agents = cide_agents::load_project(&root);
    if !agents.enabled() {
        return None;
    }

    let roles: Vec<&cide_ipc::AgentDef> = agents.catalog.agents.iter().map(|a| &a.def).collect();
    // The same resolution the roster tool reports and a dispatch enforces, so the number this
    // paragraph states is the number `cide_agents_list` states. A failure drops the figures, not
    // the paragraph.
    let limits = crate::agents::resolutions_for(app, project)
        .ok()
        .map(|resolved| {
            let ready = resolved.iter().filter(|(id, _)| {
                roles
                    .iter()
                    .any(|def| def.id == *id && def.unavailable.is_none())
            });
            let project_max = agents.config.agents.max_concurrent;
            Limits {
                project: project_max,
                total: cide_agents::overrides::capacity(ready.map(|(_, r)| r), project_max),
            }
        });
    Some(roster_paragraph(&roles, voice.unwrap_or(inferred), limits))
}

/// Which opening sentence a pane's roster paragraph gets. (M79)
///
/// Two of these until M79, decided by a `bool`. The third arrived because cide started opening
/// Claude tabs **of its own accord** — a reviewer when a subagent finishes, a planner when the
/// project has been quiet — and neither of the two existing sentences is true of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Voice {
    /// The project's console. *You are the product owner.*
    ProductOwner,
    /// Any other Claude pane the **user** opened. *You may act as one.*
    ///
    /// Hedged deliberately, and the hedge is load-bearing for this case alone: a task's
    /// conversation pane opened from the Tasks panel has just been handed a task to *work*, and
    /// two identities in one prompt is a model guessing which to be.
    Pane,
    /// A tab **cide** opened to do the product owner's job. *You are acting as it, right now.*
    ///
    /// The hedge above is exactly wrong here, and was reported as such: these tabs are opened
    /// with a brief that tells them to read a task, judge it, merge the branch, close it or
    /// dispatch it back — which *is* the job — and a system prompt that meanwhile describes them
    /// as a bystander who *could* orchestrate leaves the model arguing with itself about whether
    /// it is allowed to. There is no second identity to be confused with, because nobody handed
    /// this session a task to work; cide opened it to run the loop.
    Acting,
}

/// The paragraph itself, as a pure function of the roles and of which pane is being told.
///
/// Split from [`orchestrator_paragraph`] so the prose — which is a contract with a language model
/// and the only part of this that can be *wrong* rather than merely absent — is reachable from a
/// test with no `AppHandle`, no workspace and no `.cide/` directory.
///
/// `primary` picks the opening sentence and nothing else (M40): the console is the product owner
/// and is told so; any other Claude pane of the project has the same tools and is told it may act
/// as one — without being told it *is*, because a task's conversation pane opened from the Tasks
/// panel has just been handed a task to work, and two identities in one prompt is a model
/// guessing which to be.
fn roster_paragraph(roles: &[&cide_ipc::AgentDef], voice: Voice, limits: Option<Limits>) -> String {
    let roles = if roles.is_empty() {
        // Said rather than omitted: a session told it is the product owner and handed no roles
        // would call `cide_agents_list`, get an empty answer, and have no idea whether that is a
        // failure or the truth. The mechanics sentence below names the way to make one — this
        // used to claim "only the user can add one", which stopped being true the moment the
        // fs router (`crate::dotcide`) made a written role file take effect live.
        "This project defines no roles yet — write one as described below — so there is nobody \
         to dispatch to until one exists."
            .to_string()
    } else {
        format!(
            "The roles it defines right now are {}.",
            roles
                .iter()
                .map(|def| {
                    let description = one_line(&def.description);
                    if description.is_empty() {
                        format!("`{}`", one_line(def.id.as_str()))
                    } else {
                        format!("`{}` ({description})", one_line(def.id.as_str()))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    // The orchestrator's operating manual, and the only channel that arrives before the first
    // turn. Every mechanic named here is real (each has a pointer to the code that makes it
    // true); a sentence here that outlives its mechanism is a model confidently doing the wrong
    // thing, so treat this prose as code.
    let opening = match voice {
        Voice::ProductOwner => {
            "You are the product owner for this project in cide. You do not have to do \
             everything yourself: this project has subagents, and you can decompose a goal into \
             tasks, hand each one to a role, and check the result."
        }
        Voice::Pane => {
            "This is a Claude pane of a project in cide that has subagents, and you can hand \
             work to its roles exactly as the project's product owner — its primary Claude pane \
             — does: decompose a goal into tasks, hand each one to a role, and check the result."
        }
        Voice::Acting => {
            "You are acting as the product owner for this project in cide. cide opened this tab \
             by itself, with nobody at the keyboard, to run the orchestration loop: the \
             instructions you were given are the job, and you carry them out yourself rather \
             than reporting what somebody else should do. This project has subagents — decompose \
             a goal into tasks, hand each one to a role, take finished work back, and check the \
             result."
        }
    };

    // The numbers, when they could be read: a model told only that "caps exist" fans out ten
    // ways and learns the real figure from nine queued rows. Snapshotted at spawn like the role
    // list, so it points at the roster for the current figure the same way.
    let limits = match limits {
        Some(Limits { project, total }) => format!(
            " (at most {project} at once here) and, with these roles' `max-concurrent` and their \
             pools' running limits, at most {total} can actually be live at once — \
             `mcp__cide__cide_agents_list` gives the current figure"
        ),
        None => String::new(),
    };

    format!(
        "{opening} {roles} That list was read when this session \
         started; `mcp__cide__cide_agents_list` is the current one. A role is a markdown file at \
         `.cide/agents/<name>.md` — frontmatter `name:` and `description:` (optionally \
         `harness:`, `model:`, `tools:`, `permission-mode:`, `max-concurrent:`, and \
         `worktree: false` for a role that should work in the project root instead of its own \
         checkout — right for read-only roles, wrong for anything that edits), the body is the \
         role's system prompt, the name lowercase letters, digits and dashes, at most 32 \
         characters. Write one with `mcp__cide__cide_agent_create` and correct one with \
         `mcp__cide__cide_agent_update`, which change only the fields you name and refuse a \
         definition that would not load, naming the field: reach for them when the work in front \
         of you wants a kind of worker this project has not got. Either takes effect \
         immediately, no restart, and a project file shadows a global one of the same name. \
         Editing the markdown yourself still works, and is the only way to remove a role. The \
         roster names the scope every role is defined in, and lists this project's and your own \
         **Claude Code subagents** (`.claude/agents/*.md`, the `claudeProject` and \
         `claudeGlobal` scopes), which you may edit but cannot create — cide does not author \
         files in a directory it does not own, and a subagent is dispatched by naming it to \
         `claude --agent`, so its own `model`, `tools`, `permissionMode`, `skills` and `hooks` \
         apply and cide adds none of them; where two files declare one name, `.cide/agents/` \
         wins. Track the work itself with the `mcp__cide__cide_task_*` tools, which read and \
         write this project's shared task tracker at `.cide/tasks.json`: create the task before \
         you hand it to anybody, because a run is pointed at its task and reads the statement of \
         the work from there. Anything you notice in passing — a defect, a gap, a piece of work \
         something else turns out to need — goes on the board too, the moment you see it, \
         rather than into whatever is in flight or this conversation, because a conversation \
         ends and the board does not; but it goes to the **inbox** (status `inbox` — pass \
         `inbox: true` to `mcp__cide__cide_task_create`), where noticed work waits and nothing \
         starts it, and tasks the roles create land there by themselves. `todo` is for work you have decided the project does now. When the \
         project has milestones (`mcp__cide__cide_milestones`), that means work towards the \
         active one: link it `subtaskOf` the milestone's task, pull from the inbox only what \
         its gate needs, and let the gate — a command cide runs, not your judgement — say when \
         it is met. Assigning a todo or doing task to a role — with \
         `mcp__cide__cide_task_assign` or `mcp__cide__cide_task_update`, by creating the task \
         with an assignee, or by @mentioning a role in a task's body or a comment — starts that \
         role on it automatically; `mcp__cide__cide_agent_dispatch` (a role and a task id, \
         returns a run id immediately without waiting) is only needed to re-run a role or to add \
         a one-line extra instruction. **A role gets one run per task at a time**, so assigning \
         and then dispatching the same role onto the same task is one act asked for twice: the \
         second is refused, naming the run you already started. Stop that run or wait for it \
         rather than starting a second. Work goes through tasks; the one exception is \
         `mcp__cide__cide_agent_dispatch` with `instructions` and **no task**, which starts a \
         quick run in the project root — no worktree, no branch, nothing on the board, so \
         nothing reports back but the tree itself and the run's pane: use it to check or test \
         something, or for a small piece of work not worth a task, never for work whose result \
         you need to read, and keep it clear of files you or another run are editing. A role \
         runs up to its `max-concurrent` tasks at once, each task in its own worktree on branch \
         `cide/<role>-<task>`, and the project caps concurrent runs{limits} — anything past a \
         cap queues in order, so fan out across tasks and roles freely. A role on a model pool \
         is capped by the pool too: each of its models may carry a running limit, counted \
         across every project on this machine, a run starts on the first model with room, and \
         it waits in the queue when every one is full. A run that starts moves its \
         task to doing; when the work is done it sets the task to review and comments what it \
         did. A run reports back only through that tracker, so read those comments, take work \
         you accept into this branch with `mcp__cide__cide_agent_integrate` — naming the task, \
         which picks that task's branch — and set the task done, or comment what to change and \
         hand it back. Watch runs with `mcp__cide__cide_agent_runs`; stop one going the wrong \
         way with `mcp__cide__cide_agent_stop`. When a run hands its turn back or ends, cide \
         types one line about it into a Claude pane, and `notify` on \
         `mcp__cide__cide_agent_dispatch` says which: `here` (this pane, the default), `main` \
         (the project's primary pane) or `none` (nothing is typed — poll \
         `mcp__cide__cide_agent_runs` with includeFinished and read the task); a run started by \
         assigning or @mentioning a role reports to the pane that assigned it. Keep the plan in \
         the tracker: hold the goal in one task \
         and decompose from it, and when cide tells you a run ended, re-read that goal task and \
         the board before deciding what is next — the board, not your context, is the plan of \
         record. When you need the user, end your turn with a direct question: cide marks the \
         pane and the window title while you are awaiting input."
    )
}

/// The concurrency figures the roster paragraph states. See [`cide_agents::overrides::capacity`].
#[derive(Debug, Clone, Copy)]
struct Limits {
    /// The project's `maxConcurrent`.
    project: u16,
    /// Every cap at once: roles, pools and the project.
    total: u32,
}

/// Whatever it is handed, on one line, with runs of whitespace collapsed.
///
/// A role's description comes out of a committed markdown file, so it can be several lines. This
/// paragraph is one; the value of flattening is not the CLI's (an argv value may hold newlines
/// perfectly well) but the reader's — a wrapped sentence in the middle of a system prompt reads
/// as a new instruction.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Is this spawn the project's product-owner pane — the console's founding [`PaneRole::Primary`]
/// Claude pane?
///
/// # Why this is inferred rather than told
///
/// `session_spawn` is handed a program, a cwd, a geometry and a project. **It is never told which
/// pane it is spawning for**: the binding is made afterwards, by `pane_bind_session`, once the
/// frontend has the id back. Adding a pane parameter would be a change to `session.spawn`'s
/// options object in `ui/src/ipc/client.ts` and to every caller of it, which is a wider change
/// than the paragraph is worth — so this reads the workspace instead, and the reading is written
/// out here because it is the part a reviewer has to check.
///
/// Four rules, each closing a case that is otherwise wrong:
///
/// * **A fork is never the console.** `SplitIntent::ForkPrimary` spawns with the console's own
///   session as `resume` and `fork: true`, so without this the branch pane would be told it is
///   the product owner — the one false positive that is not merely theoretical.
/// * The pane is `tabs[0]`'s `PaneRole::Primary` Claude pane, **or the same pane sitting in
///   `project.detached`** — `detach_pane` removes a pane from its tree, and the console is
///   exactly the pane somebody tears into its own window to watch a long turn. This is the
///   lookup `cmd::file`'s default target makes, plus that correction.
/// * **With a `resume`**, it is the console iff the id being resumed is the one that pane holds.
///   `pane.conversation` counts as well as `pane.session`, because `restore_for` prefers the
///   conversation the CLI last reported.
/// * **Without one**, it is the console iff that pane holds no live child. A fresh console spawn
///   is either a pane whose session has never existed (first launch) or one whose child was just
///   killed (`claude.restart`); every *other* Claude pane spawns while the console's own child is
///   alive, so it answers false.
///
/// # The case this is wrong about, stated
///
/// A user who splits a **new** Claude pane during the window in which the console has no live
/// child — between a restart's kill and its respawn, or before the console has spawned at all —
/// gets the product owner's opening sentence on that pane. Since M40 that is the whole cost:
/// every Claude pane gets the paragraph and the tools, and this predicate only picks which of the
/// two opening sentences a pane reads. A second pane briefly told it *is* the product owner is a
/// wording slip, not a second path to anything.
fn is_primary_console_spawn(
    ws: &cide_ipc::Workspace,
    registry: &SessionRegistry,
    project: cide_ipc::ProjectId,
    resume: Option<SessionId>,
    forking: bool,
) -> bool {
    use cide_ipc::{PaneKind, PaneRole};

    if forking {
        return false;
    }
    let Ok(project) = cide_core::workspace::project(ws, project) else {
        return false;
    };
    let Some(console) = project
        .tabs
        .first()
        .into_iter()
        .flat_map(|tab| tab.tree.panes.values())
        .chain(project.detached.values())
        .find(|pane| pane.kind == PaneKind::Claude && pane.role == PaneRole::Primary)
    else {
        return false;
    };

    match resume {
        Some(resume) => console.session == Some(resume) || console.conversation == Some(resume),
        None => console
            .session
            .is_none_or(|held| registry.get(held).is_none_or(|pty| pty.has_exited())),
    }
}

/// Convert a wire geometry into the PTY crate's own, which clamps and derives pixel dims.
pub(crate) fn pty_geometry(g: Geometry) -> PtyGeometry {
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
    // A conversation to put the real harness back on. Optional on the wire like `fork`, and
    // for the same reason; see below.
    continues: Option<cide_ipc::HarnessSession>,
) -> Result<SessionId, SessionError> {
    // The command is a **shape**, and the work is below it. The split exists because M79 needs
    // this exact machinery — the user's `claude_cli` arguments, the folded system prompt, the
    // inline `--settings` that installs cide's hooks, the proxy pass, the exit watcher — from a
    // background thread with no webview in the story at all. Every one of those is a rule
    // somebody paid for, and a second spawn path would be a second place to forget them.
    //
    // Nothing moved except the signature: the body is `spawn_session` verbatim.
    spawn_session(
        &app,
        &registry,
        SpawnRequest {
            program,
            args,
            cwd,
            geometry,
            project,
            resume,
            fork,
            continues,
            // Inferred from the tree, as it always was: a webview cannot tell cide which pane
            // it is, and `is_primary_console_spawn` is the answer to that question.
            voice: None,
            // No caller on the wire can ask for extra environment, deliberately: a webview able
            // to set arbitrary variables on a `claude` sets them on a process that inherits
            // cide's own authentication.
            env: Vec::new(),
        },
    )
    .await
}

/// Everything a spawn needs, as one value. (M79)
///
/// A struct here and a flat parameter list on the command above, which is the opposite of the
/// usual advice and is right for the stated reason: the command's parameters *are* its JSON, and
/// this is an internal call where the `too_many_arguments` allow would be buying nothing.
pub(crate) struct SpawnRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub geometry: Geometry,
    pub project: Option<cide_ipc::ProjectId>,
    pub resume: Option<SessionId>,
    pub fork: Option<bool>,
    pub continues: Option<cide_ipc::HarnessSession>,
    /// Which opening sentence the roster paragraph gets, when the caller knows better than the
    /// tree does. `None` infers it, which is every spawn the webview asks for. (M79)
    pub voice: Option<Voice>,
    /// Extra variables for this child alone, applied last.
    ///
    /// Empty for every caller today, and kept because the alternative — reaching back into this
    /// function's body from a second spawn path — is the duplication the extraction removed. A
    /// variable set here outranks every pass above it; see the fold at the end of the body.
    pub env: Vec<(String, String)>,
}

/// Spawn a child for a pane. The body of [`session_spawn`], callable from Rust. (M79)
///
/// Takes `&AppHandle` and `&SessionRegistry` rather than Tauri's `State`, which is the whole of
/// the difference: every `try_state` below resolves off a plain handle, and `registry` is touched
/// in three places. A caller on a background OS thread reaches this through
/// `tauri::async_runtime::block_on`, which is legal off the runtime and off the GTK loop —
/// `agent_rpc`'s header states that rule.
pub(crate) async fn spawn_session(
    app: &tauri::AppHandle,
    registry: &SessionRegistry,
    request: SpawnRequest,
) -> Result<SessionId, SessionError> {
    let SpawnRequest {
        program,
        args,
        cwd,
        geometry,
        project,
        resume,
        fork,
        continues,
        voice,
        env: extra_env,
    } = request;
    let app = app.clone();
    // Read before the blocking closure: `WorkspaceState` is Tauri-managed state and the
    // closure below is `spawn_blocking`, which cannot hold a `State<'_, _>` across the await.
    // One bool's worth of work on the caller's thread, and it decides which way round the
    // child draws itself — see `hook_settings`.
    let theme = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .map(|ws| ws.with(|w| w.settings.theme))
        .unwrap_or_default();

    // **An empty `program` means "the user's login shell", and it is the frontend's only way
    // to say so.** A webview cannot read `$SHELL`, so `TerminalPane` used to name `/bin/bash`
    // for every shell pane — which on macOS is a 2007 bash that reads `~/.bash_profile` and
    // never the `~/.zshrc` where nvm and `brew shellenv` live, so the pane opened a shell the
    // user had never configured. `cide_core::shell` carries the whole argument and the ladder.
    //
    // Substituted here, before the spec exists, rather than in the block below that substitutes
    // the configured `claude` binary: this one brings *arguments* with it (`-l`), and the loop
    // that writes the frontend's arguments is on the next line.
    //
    // **A conversation to re-open overrides the program, the arguments, the cwd and the
    // resume** (M42). The harness spells the first two — `claude` with the id handed back as
    // `resume`, or `opencode --session <ses_…>` — and the conversation carries the directory
    // it was filed under, which is the run's worktree and not the root the frontend would
    // otherwise name. Everything else below is built exactly as for any pane, which is the
    // point: a `claude` continuation is a Claude pane's child in every respect (hooks,
    // `--settings`, the configured binary, the MCP config), and an `opencode` TUI is a program
    // in a terminal. Decided *here*, above the shell substitution and above
    // `program_is_claude`, so the placeholder `program: ''` the frontend sends for such a pane
    // never reaches either.
    let (program, args, cwd, resume) = match continues.as_ref() {
        Some(conversation) => {
            let harness = cide_agents::for_kind(conversation.harness).ok_or_else(|| {
                SessionError::Pty(format!(
                    "this build has no implementation for the {:?} harness",
                    conversation.harness
                ))
            })?;
            let spec = harness
                .continue_spec(conversation)
                .map_err(|error| SessionError::Pty(error.to_string()))?;
            (
                spec.program,
                spec.args,
                conversation.cwd.to_string_lossy().into_owned(),
                spec.resume.or(resume),
            )
        }
        None => (program, args, cwd, resume),
    };

    // Only for an empty string. A pane that names a program gets that program, so this cannot
    // reach a Claude pane, a test harness, or anything else that knows what it wants.
    let (program, args) = if program.trim().is_empty() {
        let (shell, login) = cide_core::shell::login_shell();
        (shell.to_string_lossy().into_owned(), login)
    } else {
        (program, args)
    };

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
    // The job threshold rides too: one more scalar out of the same lock, converted here so
    // the `!is_claude` branch below has a value and not a second state lookup.
    let (proxy, claude_settings, job_notify_after) = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .map(|state| {
            state.with(|ws| {
                (
                    ws.settings.proxy.clone(),
                    ws.settings.claude.clone(),
                    crate::lifecycle::job_notify_after(&ws.settings),
                )
            })
        })
        .unwrap_or_else(|| {
            (
                Default::default(),
                Default::default(),
                crate::lifecycle::job_notify_after(&cide_ipc::Settings::default()),
            )
        });

    // The user's launch configuration, filtered. Enforced *here* as well as on the Settings
    // screen and not instead of it: `workspace.json` is hand-editable and `settings_set` is one
    // `invoke` away from being bypassed, so a filter that only ran in the UI would let a
    // hand-edited file cost somebody their `--resume`. See `cide_core::claude_cli`.
    //
    // A shell pane gets neither half — see `base_env`'s note on why the env stops here, and
    // note that the arguments have nowhere sensible to go either: `--model opus` handed to
    // `bash` is a login shell that fails to start.
    let mut plan = if is_claude {
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

    // The roster paragraph, for a project's product-owner pane and for nothing else.
    //
    // **Folded into `plan.args`, before they are written, and never pushed at the end of the
    // argv.** Two facts make that the only correct place. `fold_append_system_prompt` rewrites
    // the user's own vector — a second `--append-system-prompt` occurrence does not error, the
    // *last* one silently wins and every earlier one is discarded (measured on 2.1.235), so a
    // raw push here would delete a paragraph the user set in Settings with no error anywhere.
    // And cide's arguments are appended after the user's, which is what makes cide's the later
    // occurrence and therefore theirs the one that would vanish. Folding leaves exactly one
    // occurrence, where the user put it, carrying their text and then cide's.
    //
    // The ordering below is untouched: the fold either merges in place or appends to the *user's*
    // block, and `--append-system-prompt` takes exactly one value, so nothing here can swallow a
    // token cide writes afterwards.
    //
    // Gated on `Injection::McpConfig` as well, because every instruction the paragraph carries is
    // a call to `mcp__cide__cide_agent*` or `mcp__cide__cide_task_*`. With that injection off
    // those tools are not attached, and the paragraph would be a system prompt telling a session
    // to reach for a vocabulary it does not have — the model would try, fail, and have nothing to
    // say about why. Silent is the right answer: the toggle states what it costs.
    if is_claude
        && plan.inject.has(cide_core::claude_cli::Injection::McpConfig)
        && let Some(project) = project
        && let Some(paragraph) =
            orchestrator_paragraph(&app, registry, project, resume, wants_fork(fork), voice)
    {
        if carries_append_system_prompt_file(&plan.args) {
            // Degrade, do not refuse: the two flags cannot coexist and the CLI refuses the pair
            // outright, so adding ours would make this pane fail to start. One line, on the one
            // spawn per project where it can apply — `WARNED_ARGS` carries the user-facing half
            // of this promise on the Settings screen. See `carries_append_system_prompt_file`.
            tracing::warn!(
                "this pane sets {APPEND_SYSTEM_PROMPT_FILE}, which the CLI refuses beside \
                 --append-system-prompt, so cide is not adding its subagent roster paragraph; \
                 the roster still reaches the session through the cide_agents_list tool"
            );
        } else {
            cide_core::claude_cli::fold_append_system_prompt(&mut plan.args, &paragraph);
        }
    }

    // The user's arguments go **first**, before every token cide adds.
    //
    // Not last, and the reason is a variadic flag. `--add-dir`, `--mcp-config`, `--allowedTools`
    // and `--tools` all collect every following token that does not begin with `-`, and every
    // argument cide appends below does begin with one (`--session-id`, `--resume`,
    // `--fork-session`, `--settings`, `--mcp-config`). So a user flag placed here can never
    // swallow a uuid,
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
    //
    // **That last paragraph is a claim about the CLI, and on 2.1.224 it is false.** A
    // `claude --resume <parent>` reports a *fresh* id in its hook frames, not the parent's,
    // and `/clear` mints another one mid-session. So the id chosen here is right for the
    // command line and wrong as a routing key the moment either happens. Rather than chase
    // the CLI's id, the child is told the id cide is filing it under (`CIDE_SESSION` below)
    // and `cide-hook` echoes it back, so routing no longer depends on the two agreeing.
    // Measured before the fix: 2 session ids arrived from hooks, 12 were held by panes, and
    // the two sets did not intersect at all.
    let minted = SessionId::new();
    let mut id = minted;
    if is_claude {
        // `plan.inject` and not a fresh resolution: the flags folded here are the same value
        // the refusal verdicts above were computed against, so cide can never refuse a user's
        // `--session-id` while passing none of its own — or pass its own beside theirs.
        let (effective, args) =
            cide_claude::conversation(minted, resume, wants_fork(fork), &plan.inject);
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
            // The routing key for every frame this child's hooks send. Set only for Claude
            // panes: a shell pane has no conversation, and a `claude` the user starts by hand
            // inside one must not drive that pane's busy/idle chrome.
            spec = spec.env("CIDE_SESSION", id.to_string());

            // Gated on the injection, and the `hook_settings` call moved *inside* the gate so
            // a disabled one does not pay the `cide-hook` `exists()` stat per spawn.
            //
            // There are now three ways to reach a pane with no hooks and they are not equally
            // surprising: the `None` arm below (no `cide-hook` binary beside ours, a packaging
            // failure), a user `--safe-mode` or `--bare`, and — since the injection switches —
            // the user having turned this off deliberately. The last one is silent by design,
            // says what it costs at the toggle in `ClaudeCliSection.tsx`, and gets no warning
            // line here: a log line per spawn for a setting somebody chose is noise.
            //
            // `CIDE_SESSION` and `CIDE_HOOK_SOCK` above stay set either way, deliberately.
            // They cost nothing to a harness that ignores them, and a wrapper that ends up
            // exec'ing the real `claude` still routes its hooks back to this pane.
            if let Some(flag) = plan.inject.flag(cide_core::claude_cli::Injection::Settings) {
                match hook_settings(theme) {
                    Some(json) => spec = spec.arg(flag).arg(json),
                    // Without an absolute path to `cide-hook` the child cannot run it: its cwd
                    // is the project root and its PATH is the user's. Skipping the flag leaves
                    // a working session with no hooks, which is the right way to fail here.
                    None => {
                        tracing::warn!("cannot locate cide-hook; this session reports no state")
                    }
                }
            }
        }
    }

    // `$EDITOR`, and the socket the editor it names reports back over. (M20)
    //
    // The rule is `cide_core::child_env::editor_env` and the diagnosis is in its header: Ctrl+G
    // in a Claude pane resolves `$EDITOR` and, finding none, guessed `code` — so a cide session
    // offered to edit its own plan *in VS Code*. It refuses to set either variable without the
    // other, and refuses both when the user already chose an editor, so this is a fold and not a
    // decision.
    //
    // **Every pane, not only Claude ones**, and for the reason `CLAUDE_CODE_SSE_PORT` above
    // gives: a `git commit` or a `crontab -e` in a cide shell wants the same editor a `claude`
    // there would get, and the socket does not care which program opened the connection.
    //
    // Applied *here* rather than inside `terminal_child_env`, which composes from constants and
    // from the user's settings and takes nothing that only the running application knows. The
    // socket path carries cide's pid and does not exist until `EditWaitServer` has bound it,
    // which puts it with the other two sockets rather than with the constants.
    if let Some(server) = app.try_state::<crate::edit_wait::EditWaitServer>() {
        spec = spec.apply(cide_core::child_env::editor_env(Some(server.socket())));
    }

    // The task tools. `CIDE_AGENT_SOCK` names the socket `crate::agent_rpc` bound at startup and
    // `--mcp-config` attaches the bridge that reaches it, so a pane's own Claude can read and
    // write `.cide/tasks.json` through `mcp__cide__cide_task_*` rather than by editing the file.
    //
    // **Claude panes only**, exactly as the `--settings` block above: a shell has no MCP client to
    // hand a config to, and a `claude` a user starts by hand inside one would connect with no
    // `CIDE_SESSION`, resolve to no project, and be served an empty tool list — correct, and not
    // worth an environment variable per shell.
    //
    // **The environment variable is set even when the flag is not written** — because the binary
    // could not be found, or because the user switched the injection off — deliberately, and for
    // the reason the hook block gives one scope up: it costs a harness that ignores it nothing,
    // and a wrapper that ends up exec'ing the real `claude` with an `--mcp-config` of its own
    // still finds this socket. It is inert on its own; nothing reads it but the bridge, and with
    // the injection off the bridge is never spawned. `claude_cli::REFUSED_ENV` carries the row
    // that stops a user's launch configuration pointing it at another cide's socket.
    //
    // **Last, so the variadic flag has nothing left to swallow.** `--mcp-config` collects every
    // following token that does not begin with `-`, which is the same wager `plan.args` refuses by
    // going first; putting it at the end of the argv means the only token after it is its own
    // JSON. A reader adding an argument below this line has to think about that.
    //
    // **Gated on `Injection::McpConfig`**, which is `INJECTIONS`' fifth row and the toggle on
    // Settings → Claude sessions. Off, the pane is an ordinary Claude Code pane: it keeps every
    // MCP server the user configured — cide has never passed `--strict-mcp-config` — and loses
    // cide's own, so no task tracker and, on the console pane, no subagents. The roster paragraph
    // above is gated on the same switch, or it would be a system prompt naming tools this session
    // does not have.
    if is_claude && let Some(agents) = app.try_state::<crate::agent_rpc::AgentRpcServer>() {
        spec = spec.env(
            "CIDE_AGENT_SOCK",
            agents.socket().to_string_lossy().to_string(),
        );
        spec = with_task_tools(spec, &plan.inject, agent_mcp_config);
    }

    // A pane standing in one of cide's agent checkouts — a finished run reopened on its
    // conversation (`continues`), a reviewer tab opened in the run's worktree — gets that
    // worktree's isolated directories (`agents.isolateEnv`), the very ones the run had and the
    // verify of its branch gets. Otherwise a person continuing a run would see a different
    // `user://` from the one the run's tests wrote, and a test that failed in the run would pass
    // in the pane for no reason anyone could find. Keyed on the directory rather than on
    // `continues`, so every road into a checkout agrees; empty for a project that isolates
    // nothing, so this is a no-op everywhere else.
    if let Some(root) = cide_git::worktree::root_of_checkout(&spec.cwd) {
        let isolated = cide_agents::config::load(&root)
            .agents
            .isolated_env(&spec.cwd);
        spec = spec.apply(isolated);
    }

    // The caller's own variables, **last**, so they outrank everything composed above. (M79)
    //
    // Last rather than first because a variable a caller states by name is the most specific
    // thing anybody said about this child, and the passes above are defaults: the proxy
    // environment, the toolchain `PATH`, the hook socket. Empty for every spawn the webview asks
    // for — `SpawnRequest::env` says why.
    for (key, value) in extra_env {
        spec = spec.env(key, value);
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

    // Watch the foreground process group, for a shell and only for a shell.
    //
    // This is what makes a finished `make` in a bash pane raise the same signal a finished
    // Claude turn raises — the pane dot, the tab and project badges, `Awaiting: n` in the
    // window title and the task-bar urgency hint. None of those surfaces were ever
    // Claude-specific: they key off a session id, and the reason a shell pane stayed dark is
    // that a shell emitted exactly one `SessionState` in its life, `Exited`, at death.
    //
    // Never for Claude, and that is not a performance decision: a Claude session's state
    // comes from its hooks, which know the difference between a turn ending and a permission
    // prompt, and every tool call the CLI forks takes a process group of its own. Watching
    // both would have two writers disagreeing about one `SessionState`, with whichever polled
    // last winning.
    //
    // See `cide_pty::jobs` for what is observed, and `lifecycle::job_notify_after` for why a
    // job has to run for a while before anything is said about it — how long is the user's
    // setting, read above out of the same lock as the proxy. A later settings change reaches
    // this session too: `settings_set` retunes every running watch, so this value is only
    // ever the starting point.
    if !is_claude {
        spec = spec.watch_jobs(job_notify_after);
        // Structured logs, rendered for a person — and for a shell only, for a reason as
        // firm as the one above. The Claude CLI paints a screen rather than printing lines:
        // its bytes carry no terminator to split on, and rewriting one that happened to look
        // like a document would corrupt the frame it belongs to. `json_log_render` is off
        // when the setting is, and off costs nothing (see its doc), so this is installed
        // unconditionally and the toggle reaches shells that are already open.
        spec = spec.render(crate::lifecycle::json_log_render(
            id,
            app.state::<std::sync::Arc<crate::logring::JsonLogRing>>()
                .inner()
                .clone(),
        ));
    }

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
    crate::lifecycle::watch_jobs(app.clone(), id, &session);

    registry.insert(id, session);

    // This pane is now the real harness on that conversation (M42): the agent registry refuses
    // to start a second harness process on it while this child lives — a respawn, or a Resume
    // of the interrupted run — and `report_exit` clears the row when the child is reaped.
    // After the insert, so a refusal that races this spawn finds a session it can check.
    if let Some(conversation) = continues.as_ref()
        && let Some(agents) = app.try_state::<Arc<crate::agents::AgentRegistry>>()
    {
        agents.note_viewer(conversation, id);
    }
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
    // The retained scrollback in front of the screen, for a sink with no buffer of its own —
    // a pane opened onto a run that has been printing for twenty minutes (M42). Optional on the
    // wire so every other caller stays byte-identical; the pane decides, because only it knows
    // whether it already holds a transcript. See `cide_pty::PtySession::attach_with_snapshot`.
    history: Option<bool>,
) -> Result<Response, SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    s.resize(pty_geometry(geometry))
        .map_err(|e| SessionError::Pty(e.to_string()))?;

    let sink: Arc<dyn Sink> =
        Arc::new(move |bytes: &[u8]| sink.send(InvokeResponseBody::Raw(bytes.to_vec())).is_ok());
    let (id, screen) = s.attach_with_snapshot(sink, history.unwrap_or(false));

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
pub(crate) fn cwd_of_pid(pid: u32) -> Option<PathBuf> {
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
/// that has to be observed rather than read. Written down in `docs/platforms.md`.
///
/// The BSDs want a third answer again (`sysctl KERN_PROC_CWD`), which is why this is
/// `not(target_os = "linux")` rather than a macOS arm: everything that is not Linux is honestly
/// unimplemented here, not merely untested.
#[cfg(not(target_os = "linux"))]
pub(crate) fn cwd_of_pid(_pid: u32) -> Option<PathBuf> {
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
/// **`epoch` is what makes the window label enough.** The label survives a page *reload* —
/// Vite performs one on any edit with no HMR boundary, and WebKit does after recovering a
/// webview — while the counter, a module-level `Map`, restarts at 1. Numbering that against
/// the watermark the old page left behind discards every keystroke until the new counter
/// climbs past it, which is a terminal that paints and scrolls and cannot be typed into. The
/// page therefore stamps its writes with an id minted at module load, and a change of id
/// resets the watermark. See [`SessionRegistry::accept_write`].
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
    epoch: Option<String>,
) -> Result<(), SessionError> {
    let s = registry.get(session).ok_or(SessionError::NoSuchSession)?;
    if !registry.accept_write(session, window.label(), epoch.as_deref(), seq) {
        tracing::debug!(%session, ?seq, ?epoch, window = window.label(), "session write: dropped a replayed frame");
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
///
/// # Why it reads the workspace (M17)
///
/// The `--resume` injection can be switched off, and then cide names no conversation on the
/// command line at all. A transcript that still exists on disk is no longer the whole
/// question: offering *Resume this conversation* would spawn a pane holding a **new**
/// conversation under a button promising the old one. `plan_restore` makes the same decision
/// for a launch; this is the same rule for a pane whose child has just died.
///
/// The lock is taken and dropped before anything else happens, matching `session_spawn`'s note
/// above: `WorkspaceState::with` runs under a non-reentrant lock, and `resumable` touches the
/// filesystem. `try_state` because a test harness may have no workspace, in which case the
/// shipped default — the injection is on — is the right answer.
///
/// A `State` parameter does not drift `contract/commands.json`: that file is a list of command
/// *names*, and Tauri injects state rather than taking it off the wire, so `ui/src/ipc/
/// client.ts` is untouched.
#[tauri::command(rename_all = "camelCase")]
pub fn session_resumable(app: tauri::AppHandle, cwd: String, session: SessionId) -> bool {
    let resume_enabled = app
        .try_state::<crate::workspace_state::WorkspaceState>()
        .map(|state| state.with(|ws| ws.settings.claude.cli.inject.resume.enabled))
        .unwrap_or(true);
    crate::lifecycle::resumable(std::path::Path::new(&cwd), session, resume_enabled)
}

/// Kill a session — **unless it is a subagent run's**, in which case closing the pane closes
/// the view and nothing else.
///
/// The refusal is the fix for the debug report's single-pane deaths (run e83a75fe: exit 129
/// after nine minutes of work, while its sibling in the same period lived). The chain: a
/// mirror pane's spawn plan is a **per-window JS map**, so in a multi-window layout the pane
/// can render in a window that never heard of the plan; `TerminalPane` then adopts the run's
/// session through the domain fallback *without* the `mirrored` flag, and `closePane` — whose
/// last line is this command — SIGHUPed the agent mid-turn. Silently: the run row just moved
/// to History as though it had finished, which is exactly what the report's table records.
///
/// Refused here, in the domain, rather than by a second ownership flag on `Pane`: the
/// no-`agent_open_pane` note in `cmd/agents.rs` records why two answers to "does this pane own
/// its child" is the design that rots. Runs die only through their owners — `agents_stop`, the
/// idle wind-down, a respawn, the shutdown ladder — every one of which calls `pty.kill()`
/// directly and never passes through this command, so the guard costs them nothing.
///
/// The `State` parameter does not drift `contract/commands.json` — `session_resumable`'s doc
/// above carries that argument.
#[tauri::command(rename_all = "camelCase")]
pub fn session_kill(
    registry: State<'_, SessionRegistry>,
    agents: State<'_, Arc<crate::agents::AgentRegistry>>,
    logs: State<'_, Arc<crate::logring::JsonLogRing>>,
    session: SessionId,
) {
    if agents.owns_session(session) {
        tracing::info!(%session, "refusing to kill a subagent run's session from a pane close; the view closes, the run continues");
        return;
    }
    if let Some(s) = registry.get(session) {
        s.kill();
    }
    // The raw log lines go with the scrollback that pointed at them. Here rather than at the
    // child's exit, which is the other candidate and the wrong one: an exited session's pane
    // stays open and its rendered lines stay clickable. This is the moment the pane itself is
    // going away, so nothing can ask again.
    logs.forget(session);
}

/// The whole event behind one rendered log line.
///
/// The rendering in the pane is a summary — see `cide_core::jsonlog` — and the original is
/// held only by [`crate::logring`], because the rewrite happens above the screen mirror. A
/// handle that has aged out of the ring answers `None`, which the pane shows as *no longer
/// kept* rather than as an error: scrolling far enough back is not a failure.
#[tauri::command(rename_all = "camelCase")]
pub fn session_log_detail(
    logs: State<'_, Arc<crate::logring::JsonLogRing>>,
    agents: State<'_, Arc<crate::agents::AgentRegistry>>,
    session: SessionId,
    handle: u64,
) -> Option<cide_ipc::LogLineDetail> {
    let crate::logring::Kept {
        raw,
        recorded_unix_ms,
    } = logs.get(session, handle)?;
    // Pretty-printed here rather than in the webview, so the one place that knows the bytes
    // is the one place that formats them — and so a pane in a detached window, which has its
    // own JavaScript realm, cannot render it differently from a docked one.
    let pretty = serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| raw.clone());
    Some(cide_ipc::LogLineDetail {
        raw,
        pretty,
        recorded_unix_ms,
        // Who ran it, on what, and how full the context is by now. (M80) Asked here rather than
        // through a second command for the reason the card's own header gives about `pretty`:
        // the one place that can answer is the one place that answers, and the card opens on a
        // single round trip with either the whole block or the knowledge that there is none.
        // `None` for every line a shell pane rendered, which is the case this card was built
        // for — see `LogLineDetail::run`.
        run: agents.log_run_info(session),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_core::claude_cli::Injected;

    // --- the roster paragraph ---------------------------------------------------------------

    fn role(id: &str, description: &str) -> cide_ipc::AgentDef {
        cide_ipc::AgentDef {
            id: cide_ipc::AgentId(id.to_string()),
            label: id.to_string(),
            harness: cide_ipc::Harness::Claude,
            scope: cide_ipc::agents::AgentScope::Project,
            description: description.to_string(),
            system_prompt: "You are …".into(),
            model: None,
            color: None,
            unavailable: None,
            max_concurrent: 1,
            worktree: true,
        }
    }

    #[test]
    fn the_roster_paragraph_names_the_roles_and_the_namespaced_tools() {
        let developer = role("developer", "Implements one task\n  end to end.");
        let qa = role("qa", "");
        let paragraph = roster_paragraph(&[&developer, &qa], Voice::ProductOwner, None);
        // Printed on purpose: this is prose handed to a language model, and the assertions below
        // check fragments of it. `cargo test -p cide-app roster_paragraph -- --nocapture`.
        eprintln!("{paragraph}");

        assert!(paragraph.starts_with("You are the product owner for this project in cide."));
        // A description spanning two lines in its markdown file arrives on one.
        assert!(
            paragraph.contains("`developer` (Implements one task end to end.), `qa`."),
            "{paragraph}"
        );
        // Measured: the CLI namespaces an MCP server's tools as `mcp__<server>__<tool>`, so the
        // bare names would be tools the model cannot call.
        for tool in [
            "mcp__cide__cide_agents_list",
            "mcp__cide__cide_agent_dispatch",
            "mcp__cide__cide_agent_runs",
            "mcp__cide__cide_agent_stop",
            "mcp__cide__cide_agent_integrate",
            "mcp__cide__cide_task_*",
        ] {
            assert!(paragraph.contains(tool), "{tool} is not named: {paragraph}");
        }
        assert!(
            !paragraph.contains(" cide_agent_dispatch"),
            "a bare tool name would be one the model cannot call: {paragraph}"
        );
        // The list is fixed at spawn, so it has to say which channel is current.
        assert!(paragraph.contains("is the current one"), "{paragraph}");
        // And it must not promise the run will speak up on its own.
        assert!(
            paragraph.contains("only through that tracker"),
            "{paragraph}"
        );

        // The mechanics the orchestrator can act on, each backed by real code — a sentence here
        // without its mechanism is a model confidently doing the wrong thing:
        // roles are files it may write, live (`crate::dotcide`)...
        assert!(paragraph.contains(".cide/agents/<name>.md"), "{paragraph}");
        assert!(
            paragraph.contains("takes effect immediately"),
            "{paragraph}"
        );
        // ...assignment and @mentions start the role (`crate::task_triggers`)...
        assert!(
            paragraph.contains("starts that role on it automatically"),
            "{paragraph}"
        );
        assert!(paragraph.contains("@mentioning a role"), "{paragraph}");
        // ...and that assigning and then dispatching is one act asked for twice, which is the
        // sentence M66 exists to make true (`cmd::agents::duplicate_refusal`). Nothing had ever
        // told the orchestrator that, and it assigned and dispatched in the same breath.
        assert!(paragraph.contains("one run per task"), "{paragraph}");
        // ...limits queue rather than refuse (`AgentRegistry::admit_a_pass`), and a role's
        // tasks genuinely parallelise, each in a per-task worktree (`checkout_name`)...
        assert!(paragraph.contains("queues in order"), "{paragraph}");
        assert!(paragraph.contains("its own worktree"), "{paragraph}");
        assert!(paragraph.contains("cide/<role>-<task>"), "{paragraph}");
        // ...the two roads (M40): a dispatch with no task stands in the project root and is the
        // exception, said as one (`cide_agents::run_checkout`, `ADHOC_PREAMBLE`)...
        assert!(paragraph.contains("Work goes through tasks"), "{paragraph}");
        assert!(paragraph.contains("**no task**"), "{paragraph}");
        assert!(paragraph.contains("project root"), "{paragraph}");
        assert!(
            !paragraph.contains("base worktree"),
            "the pre-M40 posture must not be described: {paragraph}"
        );
        // ...and where the finish is announced (`agent_rpc::note_run_over` reads `RunNotify`)...
        for word in ["`notify`", "`here`", "`main`", "`none`"] {
            assert!(paragraph.contains(word), "{word} is not named: {paragraph}");
        }
        assert!(
            paragraph.contains("reports to the pane that assigned it"),
            "{paragraph}"
        );
        // ...the done-workflow convention (`opening_prompt` / `TRACKER_PREAMBLE` teach the run's
        // half)...
        assert!(paragraph.contains("sets the task to review"), "{paragraph}");
        // ...the goal lives on the board, not in the context window...
        assert!(paragraph.contains("plan of record"), "{paragraph}");
        // Anything noticed in passing goes on the board. (M79) The orchestrator's half of the
        // rule `TRACKER_PREAMBLE` states for a run: a conversation ends and the board does not,
        // so a defect that lives only in this session's context is a defect nobody sees again.
        //
        // Into the **inbox** since M83, and the paragraph says so, or the orchestrator files
        // every observation as `todo` and its next planning turn plans from them — which is how a
        // real board reached 98 open tasks of which 36 were the plan. And it is told what `todo`
        // is for once there are milestones, and that the gate, not its own reading, says when one
        // is met.
        for word in [
            "**inbox**",
            "status `inbox`",
            "mcp__cide__cide_milestones",
            "subtaskOf",
        ] {
            assert!(paragraph.contains(word), "{word} is not named: {paragraph}");
        }

        // ...and the way to summon the user is to end the turn asking (`windows::set_awaiting`
        // badges the pane and retitles the window).
        assert!(
            paragraph.contains("end your turn with a direct question"),
            "{paragraph}"
        );
    }

    /// (M40) Every Claude pane is told about the subagents, and only the opening sentence says
    /// which pane it is: the console is the product owner, any other pane may act as one. The rest
    /// is byte-identical, so a rule taught to one is taught to both.
    #[test]
    fn a_second_pane_is_told_it_may_orchestrate_and_everything_else_is_the_same() {
        let developer = role("developer", "Implements one task end to end.");
        let console = roster_paragraph(&[&developer], Voice::ProductOwner, None);
        let pane = roster_paragraph(&[&developer], Voice::Pane, None);

        assert!(
            console.starts_with("You are the product owner"),
            "{console}"
        );
        assert!(pane.starts_with("This is a Claude pane"), "{pane}");
        assert!(!pane.contains("You are the product owner"), "{pane}");
        assert!(pane.contains("primary Claude pane"), "{pane}");

        let tail = |text: &str| {
            text["...".len()..]
                .split_once("That list was read")
                .map(|(_, tail)| tail.to_string())
        };
        assert_eq!(
            tail(&console),
            tail(&pane),
            "only the opening sentence may differ"
        );
        assert!(tail(&pane).is_some());
    }

    /// **A tab cide opened is told it *is* acting as the product owner.** (M79)
    ///
    /// `Voice::Pane`'s sentence is hedged — *you may act as one* — and the hedge is right for
    /// the case it was written for: a conversation pane the user opened onto a task has been
    /// handed work to *do*, and two identities in one prompt is a model guessing which to be.
    ///
    /// It is exactly wrong for a reviewer or a planner cide opened by itself. Those arrive with
    /// a brief telling them to judge a task, merge its branch, close it or dispatch it back —
    /// which is the job — and a system prompt meanwhile describing them as a bystander who
    /// *could* orchestrate left the model arguing with itself about whether it was allowed to.
    /// Reported as "the opened console still thinks it isn't an orchestrator, but it is".
    #[test]
    fn a_tab_cide_opened_is_told_it_is_acting_as_the_product_owner() {
        let developer = role("developer", "Implements one task end to end.");
        let acting = roster_paragraph(&[&developer], Voice::Acting, None);
        let pane = roster_paragraph(&[&developer], Voice::Pane, None);
        let console = roster_paragraph(&[&developer], Voice::ProductOwner, None);

        assert!(
            acting.starts_with("You are acting as the product owner"),
            "{acting}"
        );
        // The three are distinct, and the failure this guards is two of them collapsing: a
        // build where `Acting` fell through to `Pane` would pass every other test here.
        assert_ne!(acting, pane);
        assert_ne!(acting, console);

        // It is told the instructions are the job, because the reported symptom was a tab that
        // reported what somebody else should do instead of doing it.
        assert!(acting.contains("carry them out yourself"), "{acting}");
        assert!(acting.contains("nobody at the keyboard"), "{acting}");
        // And that taking finished work back is part of it — the merge the first cut left out.
        assert!(acting.contains("take finished work back"), "{acting}");

        // Everything after the opening sentence is byte-identical across all three, so a rule
        // taught to one is taught to all: the same claim `a_second_pane_is_told…` makes.
        let tail = |text: &str| {
            text.split_once("That list was read")
                .map(|(_, tail)| tail.to_string())
        };
        assert_eq!(tail(&acting), tail(&console), "only the opening may differ");
        assert_eq!(tail(&acting), tail(&pane), "only the opening may differ");
        assert!(tail(&acting).is_some());
    }

    #[test]
    fn a_project_with_no_roles_gets_a_paragraph_that_says_so_and_names_the_file() {
        let paragraph = roster_paragraph(&[], Voice::ProductOwner, None);
        assert!(paragraph.contains(".cide/agents/<name>.md"), "{paragraph}");
        assert!(paragraph.contains("nobody to dispatch to"), "{paragraph}");
    }

    /// The orchestrator is told the numbers, not only that caps exist: the project's own cap
    /// and the total every cap allows, and that pool models carry running limits.
    #[test]
    fn the_paragraph_states_how_many_runs_can_be_live() {
        let paragraph = roster_paragraph(
            &[],
            Voice::ProductOwner,
            Some(Limits {
                project: 4,
                total: 3,
            }),
        );
        assert!(
            paragraph.contains("(at most 4 at once here)"),
            "{paragraph}"
        );
        assert!(
            paragraph.contains("at most 3 can actually be live at once"),
            "{paragraph}"
        );
        assert!(paragraph.contains("running limit"), "{paragraph}");

        let unknown = roster_paragraph(&[], Voice::ProductOwner, None);
        assert!(!unknown.contains("can actually be live"), "{unknown}");
    }

    /// The degradation `cide_core::claude_cli::WARNED_ARGS` promises the user, from this side.
    #[test]
    fn an_append_system_prompt_file_is_recognised_in_both_spellings() {
        assert!(carries_append_system_prompt_file(&[
            "--append-system-prompt-file".into(),
            "notes.md".into()
        ]));
        assert!(carries_append_system_prompt_file(&[
            "--append-system-prompt-file=notes.md".into()
        ]));
        // And the neighbour it must never be confused with, which is the flag cide adds.
        assert!(!carries_append_system_prompt_file(&[
            "--append-system-prompt".into(),
            "hello".into()
        ]));
        assert!(!carries_append_system_prompt_file(&[
            "--append-system-prompt=hello".into()
        ]));
        assert!(!carries_append_system_prompt_file(&[
            "--model".into(),
            "opus".into()
        ]));
    }

    /// The row on the Settings screen and the code in this file are one promise in two places.
    ///
    /// `WARNED_ARGS` tells the user, in prose, that cide drops its own paragraph on a pane where
    /// they have set `--append-system-prompt-file` so the pane still starts. Deleting the row
    /// would leave the behaviour unexplained; deleting the behaviour would make the row a lie.
    #[test]
    fn the_warned_row_still_promises_what_this_file_does() {
        let row = cide_core::claude_cli::WARNED_ARGS
            .iter()
            .find(|entry| entry.flag == APPEND_SYSTEM_PROMPT_FILE)
            .expect("the row that explains this degradation");
        assert!(row.takes_value);
        assert!(
            row.reason.contains("drops its own paragraph"),
            "{}",
            row.reason
        );
    }

    /// The measured failure the fold exists for, at this call site.
    ///
    /// A second `--append-system-prompt` does not error: the last one silently wins and the
    /// earlier is discarded. cide's arguments come after the user's, so a raw push here would
    /// delete a prompt they set in Settings with nothing on screen saying so.
    #[test]
    fn the_paragraph_folds_into_the_users_own_append_system_prompt() {
        let developer = role("developer", "Implements one task end to end.");
        let paragraph = roster_paragraph(&[&developer], Voice::ProductOwner, None);

        let mut args = vec![
            "--append-system-prompt".to_string(),
            "always reply in British English".to_string(),
            "--model".to_string(),
            "opus".to_string(),
        ];
        cide_core::claude_cli::fold_append_system_prompt(&mut args, &paragraph);

        assert_eq!(
            args.iter()
                .filter(|token| *token == "--append-system-prompt")
                .count(),
            1,
            "exactly one occurrence may reach the child: {args:?}"
        );
        assert!(args[1].starts_with("always reply in British English"));
        assert!(args[1].contains("You are the product owner"));
        // Nothing else moved: the user's other tokens keep their order and their neighbours.
        assert_eq!(&args[2..4], ["--model".to_string(), "opus".to_string()]);
    }

    #[test]
    fn only_the_consoles_primary_pane_is_the_product_owner() {
        let mut ws = cide_ipc::Workspace::default();
        let root = std::env::temp_dir().join(format!("cide-owner-{}", std::process::id()));
        let project = cide_core::workspace::open_project(&mut ws, vec![root], None).expect("open");
        let console = ws.projects[&project].primary_session;
        let registry = SessionRegistry::default();

        // A fresh console spawn: the pane's session has never had a child.
        assert!(is_primary_console_spawn(
            &ws, &registry, project, None, false
        ));
        // A restore resumes exactly what the pane holds.
        assert!(is_primary_console_spawn(
            &ws,
            &registry,
            project,
            Some(console),
            false
        ));
        // Somebody else's conversation is somebody else's pane.
        assert!(!is_primary_console_spawn(
            &ws,
            &registry,
            project,
            Some(SessionId::new()),
            false
        ));
        // `SplitIntent::ForkPrimary` resumes the console's own session and branches from it. The
        // branch is a new pane and must never be told it is the product owner — this is the one
        // false positive that would happen in ordinary use.
        assert!(!is_primary_console_spawn(
            &ws,
            &registry,
            project,
            Some(console),
            true
        ));
        // An unknown project is nobody's console.
        assert!(!is_primary_console_spawn(
            &ws,
            &registry,
            cide_ipc::ProjectId::new(),
            None,
            false
        ));

        // And while the console's own child is alive, a fresh spawn is some *other* pane: the
        // console does not spawn twice.
        let live = PtySession::spawn(
            SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("sleep 30"),
        )
        .expect("spawn sh");
        registry.insert(console, Arc::clone(&live));
        assert!(!is_primary_console_spawn(
            &ws, &registry, project, None, false
        ));

        // Until it is killed, which is what `claude.restart` does before it respawns.
        live.kill();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while live.exit_status().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(live.exit_status().is_some(), "child was never reaped");
        assert!(is_primary_console_spawn(
            &ws, &registry, project, None, false
        ));
    }

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
            cide_claude::conversation(id, None, wants_fork(None), &Injected::defaults()),
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
            cide_claude::conversation(
                minted,
                Some(parent),
                wants_fork(None),
                &Injected::defaults()
            )
            .0,
            parent,
            "a plain resume runs under the parent's id"
        );
        assert_eq!(
            cide_claude::conversation(
                minted,
                Some(parent),
                wants_fork(Some(true)),
                &Injected::defaults()
            )
            .0,
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
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir()).apply([
            ("PYTHONHOME".to_string(), None),
            ("PATH".to_string(), Some("/usr/bin".to_string())),
        ]);
        assert_eq!(value_of(&spec, "PATH"), Some("/usr/bin"));
        assert!(
            spec.env_remove.iter().any(|k| k == "PYTHONHOME"),
            "removed {:?}",
            spec.env_remove
        );
    }

    /// The scrub and the `PATH` pass reach the spec as one `PATH`, in that order.
    ///
    /// `terminal_child_env` chains them, and a chain is exactly the shape a later edit turns
    /// back into two folds in the wrong order. Both halves are asserted over synthetic inputs
    /// because `bundle_scrub` and `child_path` read the real process environment; `cide-core`'s
    /// own tests own the rules, and what is only checkable here is that the composition survives
    /// the trip into a `SpawnSpec` — a `SpawnSpec` that carried the scrub's `PATH` and dropped
    /// the appended one would put the M17 bug straight back with nothing to say so.
    #[test]
    fn the_scrubbed_path_and_the_appended_directories_arrive_as_one_entry() {
        let scrub = vec![("PATH".to_string(), Some("/usr/bin".to_string()))];
        let path = cide_core::child_env::child_path_in(
            &scrub,
            Some(std::ffi::OsStr::new(
                "/tmp/.mount_cide_0OOoGFm/usr/bin:/usr/bin",
            )),
            std::slice::from_ref(&std::path::PathBuf::from("/home/u/go/bin")),
        );
        let spec =
            SpawnSpec::new("/bin/sh", std::env::temp_dir()).apply(scrub.into_iter().chain(path));
        // The *last* one, not the first: `SpawnSpec::env` is an ordered list and
        // `PtySession::spawn` replays it into `CommandBuilder::env`, where a later write wins.
        // `value_of` finds the first, which is the scrub's — so asserting with it here would
        // pass while the child got the unappended `PATH`.
        let last = spec
            .env
            .iter()
            .rfind(|(k, _)| k == "PATH")
            .map(|(_, v)| v.as_str());
        assert_eq!(last, Some("/usr/bin:/home/u/go/bin"));
        assert_eq!(
            value_of(&spec, "PATH"),
            Some("/usr/bin"),
            "the scrub's own PATH must still be first, or the two passes were folded backwards"
        );
    }

    /// Nothing to scrub must mean nothing added, so a non-bundled launch is byte-identical.
    #[test]
    fn an_empty_scrub_leaves_the_spec_alone() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir()).apply([]);
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

    // --- M18: the task tools, and the switch that declines them ------------------------------

    /// The default: cide writes `--mcp-config` and its JSON, and nothing else.
    ///
    /// The spelling comes from `INJECTIONS`, so a rename in Settings moves it; asserted through
    /// the resolved `Injected` rather than against a literal for exactly that reason.
    #[test]
    fn a_pane_carries_the_task_tools_by_default() {
        let inject = cide_core::claude_cli::plan(&cide_ipc::ClaudeCli::default(), None).inject;
        let spec = with_task_tools(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &inject,
            || Some("{\"mcpServers\":{}}".to_string()),
        );
        assert_eq!(spec.args, ["--mcp-config", "{\"mcpServers\":{}}"]);
    }

    /// The switch, and the whole reason it exists: **off, the flag is simply absent**.
    ///
    /// Not an empty config, not a `--strict-mcp-config`, not a different server — absent. The
    /// pane is then an ordinary Claude Code pane that keeps every MCP server the user configured
    /// and loses cide's own. `CIDE_AGENT_SOCK` stays in the environment either way (see the call
    /// site): nothing reads it but the bridge, and the bridge is never spawned.
    #[test]
    fn switching_the_task_tools_off_writes_no_flag_at_all() {
        let mut cli = cide_ipc::ClaudeCli::default();
        cli.inject.mcp_config.enabled = false;
        let inject = cide_core::claude_cli::plan(&cli, None).inject;
        let spec = with_task_tools(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &inject,
            || panic!("a switched-off injection must not even look for cide-hook"),
        );
        assert!(spec.args.is_empty(), "{:?}", spec.args);
    }

    /// A rename reaches the argv, because the flag is read out of the resolved injection rather
    /// than written as a literal — which is the defect `check:claude-cli` asserts is gone for
    /// `--settings` and would have to assert again here.
    #[test]
    fn a_renamed_task_tools_flag_is_the_one_written() {
        let mut cli = cide_ipc::ClaudeCli::default();
        cli.inject.mcp_config.flag = "--mcp".into();
        let inject = cide_core::claude_cli::plan(&cli, None).inject;
        let spec = with_task_tools(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &inject,
            || Some("{}".to_string()),
        );
        assert_eq!(spec.args, ["--mcp", "{}"]);
    }

    /// No `cide-hook` beside the running binary is a packaging failure, not a choice: the flag
    /// is skipped and the pane still starts, because a session with no task tools works and a
    /// pane that refuses to open does not.
    #[test]
    fn a_missing_bridge_costs_the_tools_and_not_the_pane() {
        let inject = cide_core::claude_cli::plan(&cide_ipc::ClaudeCli::default(), None).inject;
        let spec = with_task_tools(
            SpawnSpec::new("claude", std::env::temp_dir()),
            &inject,
            || None,
        );
        assert!(spec.args.is_empty(), "{:?}", spec.args);
    }

    /// The paragraph and the tools are one decision.
    ///
    /// The roster paragraph is nothing but instructions to call `mcp__cide__*`, so a pane that
    /// gets it without the server is a session told to reach for a vocabulary it does not have.
    /// The spawn gates both on the same injection; this pins the fact the prose depends on it,
    /// so a future edit that moves the paragraph out from behind the gate has to answer for it.
    #[test]
    fn the_roster_paragraph_is_only_worth_sending_with_the_tools_that_back_it() {
        let paragraph = roster_paragraph(&[], Voice::ProductOwner, None);
        for tool in ["mcp__cide__cide_agents_list", "mcp__cide__cide_task_"] {
            assert!(paragraph.contains(tool), "{paragraph}");
        }
        let mut cli = cide_ipc::ClaudeCli::default();
        cli.inject.mcp_config.enabled = false;
        assert!(
            !cide_core::claude_cli::plan(&cli, None)
                .inject
                .has(cide_core::claude_cli::Injection::McpConfig),
            "and this is the condition the spawn gates the paragraph on"
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
             the one to turn on — see docs/platforms.md"
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
