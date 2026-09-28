//! Running a role on OpenAI's Codex CLI (`codex`). (M44, rehosted in M93)
//!
//! # Hosted like `claude`, since M93
//!
//! M44 measured 0.153.2 and hosted codex like opencode: one `codex exec --json` child per turn,
//! the run's state read off the JSONL it printed, every follow-up a fresh `exec resume` child.
//! The reason was the hooks. Codex's are Claude-shaped, but they lived only in `config.toml` and
//! `.codex/hooks.json` behind a persisted trust hash, and cide writes into neither — so an
//! interactive TUI in the pane would have had no state at all. That header named
//! `-c hooks.… --dangerously-bypass-hook-trust` as the unmeasured upgrade path.
//!
//! M93 measured it on 0.155.1 in a real TUI, and it works. The hook table rides as `-c`
//! overrides, the trust bypass makes the TUI run them, and the frames are Claude's frames:
//! `session_id`, `transcript_path`, `hook_event_name`, `turn_id`, `last_assistant_message`, with
//! the TUI's own environment inherited by the hook command. [`cide_core::codex_cli`] carries
//! the measurement and the spelling. So a run is now what a claude run is:
//!
//! * the **real TUI** in the PTY, at the pane's geometry — the person who opens the run reads
//!   codex itself, and can answer an approval prompt in it;
//! * **state from hooks**, through the same [`cide_claude::next_state`] a claude run and a
//!   console use, plus the one event claude does not have: codex names its approval prompt
//!   (`PermissionRequest`), so [`RunState::AwaitingPermission`] exists on this harness now;
//! * **[`SessionBinding::Caller`]**: cide mints the run's routing id and hands it over as
//!   `CIDE_SESSION`, so a hook frame is routable before codex has said anything. The
//!   *conversation* is still codex's to mint — its TUI cannot be handed an id — and it arrives
//!   in the `SessionStart` payload, which [`CodexHarness::capture_hook`] reads. That thread id is
//!   the run's harness session, and it is what `codex resume` takes;
//! * a follow-up is **typed** ([`Delivery::Stdin`]), and a stop is **Esc**.
//!
//! What was dropped with `exec`: the JSON-to-prose rendering, the `#handle` ring and the
//! per-turn token figures `turn.completed` carried. The first two existed only because a pane
//! showed a machine stream; it shows the TUI now. The third is a real loss for the panel's usage
//! line, and the rollout file's `token_count` events are where it would come back from.
//!
//! # The argv
//!
//! `codex [resume] <the user's args> -C <cwd> <sandbox> [-m <model>] [-c effort] <quiet start>
//! [-c developer_instructions] [<hook table> --dangerously-bypass-hook-trust]
//! [-c mcp_servers.cide.*] [<thread>] [<prompt>]`. The `resume` subcommand accepts every TUI
//! option (measured in `codex resume --help`), so the options come after it where they certainly
//! parse, and the thread and prompt are the trailing positionals: nothing may follow them.
//!
//! The opening prompt is **positional**, not typed. Claude's has to be typed because a variadic
//! `--mcp-config` would swallow a positional; nothing here is variadic, and a prompt in the argv
//! reaches the model without waiting on a composer being ready — the startup is where codex has
//! modals ([`cide_core::codex_cli::QUIET_START`] on the two that were measured eating keys).
//!
//! # The brief, the tracker and the environment
//!
//! Unchanged from M44 and argued there: the brief is one TOML-encoded `-c
//! developer_instructions=`; cide's server is `-c mcp_servers.cide.*` with **its environment
//! spelled into the server's own table**, because codex starts a stdio server with a
//! whitelisted environment; `CODEX_API_KEY` is neither set nor removed. `CIDE_SESSION` joins
//! that table now, beside `CIDE_RUN` and `CIDE_AGENT_SOCK`.
//!
//! # Permissions
//!
//! The TUI can ask, which `exec` could not, so the definition's claude-vocabulary
//! `permission-mode` maps onto codex's two axes — sandbox (`-s`) and approval (`-a`) — rather
//! than onto sandboxes alone ([`permission_policy`]). `default` is finally expressible: the
//! workspace-write sandbox with approval on request, a prompt a person answers in the pane.
//! An unattended run keeps M44's rule: `--dangerously-bypass-approvals-and-sandbox`, because
//! the workspace-write sandbox re-binds `.git` read-only and the brief asks every task run to
//! commit.
//!
//! **A sandboxed run in a worktree can commit, since M118.** A role that names a mode itself —
//! `auto` above all, which on claude and opencode means "run unattended" — used to land in the
//! workspace-write sandbox with its git metadata read-only: the checkout's admin directory
//! (`<repo>/.git/worktrees/<name>`) and the common `.git` are outside the one writable root, and
//! codex re-binds `.git` read-only on top. Every such run in selfcraft stopped at its first
//! commit (t-518, t-572, t-574, …), and M114's verify gate handed the dirty branch back to a run
//! that could not act on it. [`RunPlan::git_dirs`] now rides as `--add-dir` under every policy
//! [`sandboxes_writes`] names, which keeps the sandbox and makes the commit work (measured on
//! 0.157.1, step 4 of [`assemble`]). What the sandbox still denies — an X display, audio — is
//! [`sandbox_caveat`]'s to say.
//!
//! **Always an explicit pair**, never codex's own default: with neither flag, codex consults
//! the directory's trust and may open a "trust this folder?" chooser first — in a fresh
//! worktree, every time. (Unmeasured in a worktree; measured absent with explicit flags.)
//!
//! `tools:` is dropped, as M44 argued.
//!
//! # Finding the binary
//!
//! Settings → Harness → Codex → Binary ([`RunPlan::codex`]), a bare `codex` by default and
//! resolved by the OS at the spawn — the console and every codex run start the same program.

use std::path::{Path, PathBuf};

use cide_claude::{HookEvent, HookFrame};
use cide_core::codex_cli;
use cide_ipc::{HarnessSession, RunState, SessionState};
use cide_pty::{Geometry as PtyGeometry, SpawnSpec};
use serde::Deserialize;

use super::claude::{run_state_of, session_state_of};
use super::{
    ADHOC_PREAMBLE, ContinueSpec, Delivery, ESC, Harness, HarnessError, HarnessSpawn, Observation,
    RunPlan, SERVER, SessionBinding, spec_preamble, tracker_preamble,
};
use crate::config::Unattended;

/// The Codex CLI as a harness. A unit struct: it holds nothing, and must not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodexHarness;

impl Harness for CodexHarness {
    fn kind(&self) -> cide_ipc::Harness {
        cide_ipc::Harness::Codex
    }

    /// `mcp__<server>__<tool>` — claude's namespacing, which is codex's own
    /// (`codex-rs/codex-mcp/src/tools.rs`, M44).
    fn tool_name(&self, tool: &str) -> String {
        format!("mcp__{SERVER}__{tool}")
    }

    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError> {
        assemble(plan, None)
    }

    /// The TUI back on the thread: `codex resume … <thread> [<prompt>]`. `session` is the thread
    /// id [`Self::capture_hook`] read off the first child's `SessionStart`, never cide's routing
    /// id — which the run keeps, so hook routing and an open pane do not move.
    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        let thread = session.trim();
        if thread.parse::<cide_ipc::SessionId>().is_err() {
            return Err(HarnessError::NotAConversation {
                harness: cide_ipc::Harness::Codex,
                id: session.to_string(),
            });
        }
        assemble(plan, Some(thread))
    }

    /// Typed, as a bracketed paste and then Enter. The paste is what keeps a multi-line
    /// follow-up one message: inside it a newline is text, where a bare newline typed into the
    /// composer would be read as a key. Measured on 0.155.1: a paste followed by `\r` submits.
    fn deliver(&self, text: &str) -> Delivery {
        Delivery::Stdin(submit(text))
    }

    /// Esc: the TUI's own "esc to interrupt", shown under every turn in flight.
    fn interrupt(&self) -> Option<Vec<u8>> {
        Some(vec![ESC])
    }

    /// The codex TUI on the conversation, as a console pane opens it: the program, and the
    /// thread as the `resume` a console spawn hands to `codex resume`. A codex thread id is a
    /// uuid (v7), so it travels as a [`cide_ipc::SessionId`] the way claude's does — and the
    /// pane that opens gets everything a codex console gets: hooks, cide's server, the
    /// configured binary.
    fn continue_spec(&self, conversation: &HarnessSession) -> Result<ContinueSpec, HarnessError> {
        if conversation.harness != cide_ipc::Harness::Codex {
            return Err(HarnessError::WrongHarness {
                plan: conversation.harness,
                harness: cide_ipc::Harness::Codex,
            });
        }
        let id: cide_ipc::SessionId =
            conversation
                .id
                .trim()
                .parse()
                .map_err(|_| HarnessError::NotAConversation {
                    harness: cide_ipc::Harness::Codex,
                    id: conversation.id.clone(),
                })?;
        Ok(ContinueSpec {
            program: crate::defs::harness_binary(cide_ipc::Harness::Codex).to_string(),
            args: Vec::new(),
            resume: Some(id),
        })
    }

    /// Hooks, through claude's state machine — the frames are claude's frames — and the exit.
    ///
    /// | observation | answer |
    /// | --- | --- |
    /// | `Exit(code)` | `Finished { code }` from any state |
    /// | `Hook` — `PermissionRequest` | `AwaitingPermission` |
    /// | `Hook` — anything else | [`cide_claude::next_state`], mapped as a claude run's is |
    /// | `Line` | `None` — the TUI paints a screen; nothing is read off it |
    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState> {
        match ob {
            Observation::Exit(code) => {
                let next = RunState::Finished { code };
                (next != current).then_some(next)
            }
            Observation::Line(_) => None,
            Observation::Hook(frame) => {
                let event = frame.kind()?;
                let session: SessionState = session_state_of(&current)?;
                let next = run_state_of(cide_claude::next_state(session, event)?)?;
                (next != current).then_some(next)
            }
        }
    }

    /// The thread, from `SessionStart` and from nothing else — the first frame of a TUI, and the
    /// one whose `session_id` is certainly this child's conversation.
    fn capture_hook(&self, frame: &HookFrame) -> Option<String> {
        (frame.kind()? == HookEvent::SessionStart)
            .then(|| frame.session_id())
            .flatten()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    /// `codex debug models`, run for real.
    ///
    /// A probe and not a table, for `opencode.rs`'s reason: the catalog is a property of this
    /// machine — which models the account can reach, under what names — and it refreshes
    /// under the CLI's own etag. Measured at 0.8 s with a warm cache. The `visibility` filter
    /// is the catalog's own: a `hide` entry is one the CLI's picker would not offer either.
    fn models(
        &self,
        cwd: Option<&Path>,
        _llm: &cide_ipc::LlmSettings,
    ) -> Result<Vec<String>, String> {
        let binary =
            cide_core::toolchain::which(crate::defs::harness_binary(cide_ipc::Harness::Codex))
                // The *same* search `defs::installed` uses, and the same sentence when it misses.
                .ok_or_else(|| {
                    crate::defs::installed(cide_ipc::Harness::Codex)
                        .unwrap_or_else(|| "`codex` could not be found".to_string())
                })?;

        let mut command = std::process::Command::new(&binary);
        command.args(["debug", "models"]);
        command.env("NO_COLOR", "1");
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        // `run_filter_with`, the workspace's spawn chokepoint, so `prepare_command` and `arm`
        // are applied. The binary's own directory rides on the child's PATH for the
        // version-manager-shim case, as opencode's probe does.
        let bin_dir: Vec<std::path::PathBuf> = binary
            .parent()
            .map(|d| vec![d.to_path_buf()])
            .unwrap_or_default();
        let filtered =
            cide_core::child_env::run_filter_with(command, None, MODELS_DEADLINE, &bin_dir)
                .map_err(|error| describe(&error))?;

        if !filtered.ok {
            let detail = filtered
                .stderr
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty());
            return Err(match detail {
                Some(line) => format!("`codex debug models` failed: {line}"),
                None => "`codex debug models` failed and said nothing".to_string(),
            });
        }

        model_slugs(&String::from_utf8_lossy(&filtered.stdout))
    }
}

// ==========================================================================================
// The child.
// ==========================================================================================

/// Build the child, fresh (`resume: None`) or back on the thread the first child named. One
/// function so the two differ in exactly the identity tokens — `claude.rs::assemble`'s shape.
fn assemble(plan: &RunPlan<'_>, resume: Option<&str>) -> Result<HarnessSpawn, HarnessError> {
    // `plan.harness`, the *resolved* one: a role a local override moved onto this CLI must not be
    // refused by it. See `RunPlan::harness`.
    if plan.harness != cide_ipc::Harness::Codex {
        return Err(HarnessError::WrongHarness {
            plan: plan.harness,
            harness: cide_ipc::Harness::Codex,
        });
    }
    // A fresh run with nothing to do is a TUI sitting at an empty composer holding a slot and a
    // worktree. A resumed one may legitimately have nothing new to say.
    if resume.is_none() && plan.prompt.trim().is_empty() {
        return Err(HarnessError::NoPrompt);
    }
    // Refused before anything is built, so a role naming a mode this harness cannot honour is
    // refused at dispatch with the mode in the sentence.
    let review_permissions = !plan.tracker_paragraphs && plan.codex.cli.inject.review_permissions;
    let policy = if review_permissions {
        // The shorthand also selects a sandbox and conflicts with wrappers that pass -s.
        // Route requests to the automatic reviewer without overriding the wrapper's sandbox.
        &[
            "-a",
            "on-request",
            "-c",
            "approvals_reviewer=\"auto_review\"",
        ][..]
    } else {
        permission_policy(plan.agent.permission_mode.as_deref(), plan.unattended)?
    };

    // The user's launch configuration — Settings → Harness → Codex — filtered as a console's is.
    let cli = codex_cli::plan_here(&plan.codex.cli);
    for refused in cli.refusals() {
        tracing::warn!(
            token = %refused.text,
            "refusing a codex launch argument or variable for a run: {}",
            refused.verdict.note().unwrap_or_default()
        );
    }
    // As stored: a bare name stays bare and the OS resolves it at this spawn.
    let program = match plan.codex.cli.binary.trim() {
        "" => crate::defs::harness_binary(cide_ipc::Harness::Codex).to_string(),
        configured => configured.to_string(),
    };

    let mut args: Vec<String> = Vec::new();

    // ---- 1. the subcommand, ahead of everything it parses ----
    if resume.is_some() {
        args.push("resume".into());
    }

    // ---- 2. the user's own arguments, before cide's ----
    // A wrapper separator belongs after cide's options, otherwise `-C <cwd>` is parsed by the
    // wrapper as a positional argument instead of reaching Codex.
    let mut user_args = cli.args;
    let wrapper_separator = codex_cli::remove_wrapper_separator(&mut user_args);
    args.extend(user_args);

    // ---- 3. where the work is ----
    //
    // Both `-C` and the spawn's cwd, to the same directory: the cwd is what the child inherits,
    // `-C` is what codex resolves the workspace root, the sandbox's writable root and
    // `AGENTS.md` from.
    args.push("-C".into());
    args.push(plan.cwd.to_string_lossy().to_string());

    // ---- 4. the sandbox and the approval policy ----
    //
    // Unless Settings says a wrapper decides (M108, `CodexInjections::permissions`). The policy
    // is still computed above, so a role naming a mode codex cannot honour is still refused.
    if plan.codex.cli.inject.permissions || review_permissions {
        args.extend(policy.iter().map(|token| token.to_string()));
        // The checkout's git metadata, writable beside it (M118). The sandbox's one writable
        // root is `-C`, and codex re-binds any `.git` in it read-only on top; the linked
        // worktree's admin directory and the common `.git` are outside it anyway. Without these
        // a sandboxed task run cannot `git commit`, `merge` or `rebase` — selfcraft's t-572,
        // `index.lock: Read-only file system`. Measured on 0.157.1: `--approve-for-me --add-dir
        // <git-dir> --add-dir <common-dir>` commits from a linked worktree, and the same turn
        // without them does not; `codex resume` takes `--add-dir` too.
        if sandboxes_writes(policy) {
            for dir in &plan.git_dirs {
                args.push("--add-dir".into());
                args.push(dir.to_string_lossy().to_string());
            }
            // The role's grant (M119). `needs` is the network switch, because codex's seccomp
            // filter refuses **every** `AF_UNIX` socket while the network is off — Xvfb cannot
            // listen and Blender hangs at exit on PulseAudio's wake-up write, measured on
            // 0.157.1 with `codex sandbox`. `writable-dirs` are more of what `git_dirs` are.
            let grant = &plan.agent.sandbox;
            if grant.wants_network() {
                codex_cli::push_config(
                    &mut args,
                    "sandbox_workspace_write.network_access",
                    "true".into(),
                );
            }
            let home = std::env::var_os("HOME").map(PathBuf::from);
            for dir in grant.writable_paths(home.as_deref()) {
                args.push("--add-dir".into());
                args.push(dir.to_string_lossy().to_string());
            }
        }
    }
    // Trust for the project whose worktree carries this run's `.codex/rules/cide.rules`: codex
    // reads a project layer's rules only for a trusted project, and for a linked worktree it
    // asks about the main repository's root (measured on 0.157.1; `RunPlan::codex_trust_root`).
    // Under any policy, not only a sandboxed one — the rules also take the approval prompt away
    // from an allowed command under `default`. Only when cide wrote the file, so a role without
    // `allow-commands` leaves the user's trust decisions exactly as they were.
    if let Some(root) = &plan.codex_trust_root {
        codex_cli::push_config(
            &mut args,
            &format!(
                "projects.{}.trust_level",
                codex_cli::toml_string(&root.to_string_lossy())
            ),
            codex_cli::toml_string("trusted"),
        );
    }

    // ---- 5. what the definition asked for ----
    if let Some(model) = &plan.agent.def.model {
        args.push("-m".into());
        args.push(model.clone());
    }
    if let Some(effort) = &plan.agent.effort {
        codex_cli::push_config(
            &mut args,
            "model_reasoning_effort",
            codex_cli::toml_string(effort),
        );
    }

    // ---- 6. no modal before the first turn ----
    args.extend(codex_cli::quiet_start());

    // ---- 7. the brief ----
    if cli.inject.developer_instructions {
        args.extend(codex_cli::developer_instructions(&developer_brief(plan)));
    }

    // ---- 8. the hooks, and the tracker with its own environment ----
    //
    // Both gated on `hook_bin`: without an absolute path to `cide-hook` the child cannot run
    // either, and a run with no hooks is a row that never moves.
    if let Some(hook) = &plan.hook_bin {
        let hook = hook.to_string_lossy();
        if cli.inject.hooks {
            let events: Vec<&str> = HookEvent::CODEX.iter().map(|e| e.as_str()).collect();
            args.extend(codex_cli::hook_overrides(&hook, &events));
        }
        if cli.inject.mcp_config {
            let mut env = vec![
                ("CIDE_RUN", plan.run.to_string()),
                ("CIDE_SESSION", plan.session.to_string()),
            ];
            if let Some(sock) = &plan.agent_sock {
                env.push(("CIDE_AGENT_SOCK", sock.to_string_lossy().to_string()));
            }
            args.extend(codex_cli::mcp_overrides(&hook, &env));
        }
    }

    // ---- 9. last, and nothing may be added after them ----
    if let Some(thread) = resume {
        args.push(thread.to_string());
    }
    if wrapper_separator {
        args.push("--".into());
    }
    if !plan.prompt.trim().is_empty() {
        args.push(plan.prompt.clone());
    }

    let mut spec = SpawnSpec::new(program, plan.cwd.clone()).geometry(PtyGeometry::new(
        plan.geometry.cols,
        plan.geometry.rows,
        plan.geometry.cell_width,
        plan.geometry.cell_height,
    ));
    for arg in args {
        spec = spec.arg(arg);
    }

    // A pane's order: the terminal environment with the user's launch variables folded in, the
    // proxy, the isolated directories, then cide's routing keys — which nothing above may shadow.
    spec = spec.apply(cide_core::child_env::terminal_child_env(
        &plan.claude,
        env!("CARGO_PKG_VERSION"),
        cli.env,
    ));
    spec = spec.apply(plan.proxy.changes().to_vec());
    spec = spec.apply(plan.env.clone());
    // GL for a role that needs a display (M119): the sandbox's `/dev` has no GPU nodes, and
    // glvnd's first choice on an NVIDIA machine crashes Xvfb's GLX setup inside it; pointed at
    // Mesa, Xvfb runs GLX on llvmpipe (measured under `codex sandbox`, 0.157.1). Only where the
    // sandbox is actually on, and only when this machine has Mesa's vendor file at all.
    if plan.codex.cli.inject.permissions
        && sandboxes_writes(policy)
        && plan
            .agent
            .sandbox
            .needs(crate::sandbox::SandboxNeed::Display)
        && let Some(mesa) = mesa_egl_vendor()
    {
        spec = spec.env(
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            mesa.to_string_lossy().to_string(),
        );
        spec = spec.env("__GLX_VENDOR_LIBRARY_NAME", "mesa".to_string());
    }
    // The real GPU for a role that needs it (M125): a `bwrap` shim first on codex's `PATH`, which
    // is where 0.157.1 finds the bwrap it builds the sandbox with. Under the same guard as the
    // Mesa pass above, whose GLX override it leaves alone — that is GL for an Xvfb, and the GPU
    // this is for is Vulkan's, which glvnd's vendor variables do not steer.
    if plan.codex.cli.inject.permissions
        && sandboxes_writes(policy)
        && plan.agent.sandbox.needs(crate::sandbox::SandboxNeed::Gpu)
        && let Some(dir) = &plan.codex_path_prepend
    {
        // The `PATH` the child would otherwise get: the last one the passes above set, which
        // `child_path` always does, and cide's own when none did.
        let current = spec
            .env
            .iter()
            .rev()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var("PATH").ok());
        let path = match current {
            Some(rest) if !rest.is_empty() => format!("{}:{rest}", dir.display()),
            _ => dir.display().to_string(),
        };
        spec = spec.env("PATH", path);
    }
    spec = spec.env("CIDE_SESSION", plan.session.to_string());
    spec = spec.env("CIDE_RUN", plan.run.to_string());
    if let Some(sock) = &plan.hook_sock {
        spec = spec.env("CIDE_HOOK_SOCK", sock.to_string_lossy().to_string());
    }
    if let Some(sock) = &plan.agent_sock {
        spec = spec.env("CIDE_AGENT_SOCK", sock.to_string_lossy().to_string());
    }

    Ok(HarnessSpawn {
        spec,
        opening: None,
        binding: SessionBinding::Caller,
        // Hooks are this harness's channel now.
        events: None,
    })
}

/// Mesa's glvnd EGL vendor file, where distributions put it.
fn mesa_egl_vendor() -> Option<PathBuf> {
    [
        "/usr/share/glvnd/egl_vendor.d/50_mesa.json",
        "/etc/glvnd/egl_vendor.d/50_mesa.json",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

/// A follow-up as the TUI takes one: a bracketed paste, then Enter.
pub(crate) fn submit(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 13);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes.push(b'\r');
    bytes
}

/// The paragraphs a role is told, joined the way every harness joins them.
///
/// The role's own words; then, only when the tracker is attached, the tracker paragraph, the
/// OpenSpec paragraph for a run working a change, and the ad-hoc correction for a run with no
/// task — `claude.rs`'s fold order and gate. `pub(crate)` for `harness.rs`'s parity tests.
pub(crate) fn developer_brief(plan: &RunPlan<'_>) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut say = |text: String| {
        if !text.trim().is_empty() {
            paragraphs.push(text);
        }
    };
    say(plan.agent.def.system_prompt.clone());
    // `&& tracker_paragraphs`: an MR review keeps the bridge and hears none of this (M85).
    if plan.hook_bin.is_some() && plan.tracker_paragraphs {
        say(tracker_preamble(&CodexHarness));
        if let Some(change) = &plan.change {
            say(spec_preamble(
                change,
                plan.spec_cli.as_deref(),
                plan.spec_apply.as_deref(),
                &CodexHarness,
            ));
        }
        if plan.adhoc() {
            say(ADHOC_PREAMBLE.to_string());
        }
    }
    // Outside the tracker gate: what the sandbox denies is true of a review too. (M119)
    if let Some(note) = &plan.sandbox_brief {
        say(note.clone());
    }
    paragraphs.join("\n\n")
}

const BYPASS: &[&str] = &["--dangerously-bypass-approvals-and-sandbox"];
/// Edits inside the checkout without asking; anything beyond it asks a person.
const ASK_BEYOND_WORKSPACE: &[&str] = &["-s", "workspace-write", "-a", "on-request"];
/// Edits inside the checkout without asking; anything beyond it is refused and returned to the
/// model — claude's `dontAsk`, which never puts a prompt in front of anyone.
const NEVER_ASK: &[&str] = &["-s", "workspace-write", "-a", "never"];
const READ_ONLY: &[&str] = &["-s", "read-only", "-a", "on-request"];
/// Claude's `auto` is a classifier approving on the user's behalf; codex's flag routes approvals
/// through its own reviewer under the workspace-write sandbox. clap refuses it beside `-a`
/// (measured on 0.157.1), so it cannot be told `never` as well.
const AUTO_REVIEW: &[&str] = &["--approve-for-me"];

/// Whether a [`permission_policy`] answer runs the child in codex's **`workspace-write`**
/// sandbox — the one where [`RunPlan::git_dirs`] must be added as writable roots for a commit to
/// work. Not bypass (no sandbox, nothing to add) and not `read-only` (nothing is writable, and
/// adding the git metadata alone would be a sandbox that can commit but not edit). (M118)
pub fn sandboxes_writes(policy: &[&str]) -> bool {
    [ASK_BEYOND_WORKSPACE, NEVER_ASK, AUTO_REVIEW].contains(&policy)
}

/// Codex's sandbox and approval flags for a definition's claude-vocabulary permission mode, or
/// for the project default when the definition names none. Always a sandbox *and* an approval
/// policy (or the one flag that sets both) — the module header on why codex's own default is
/// never left to decide.
pub fn permission_policy(
    mode: Option<&str>,
    unattended: Unattended,
) -> Result<&'static [&'static str], HarnessError> {
    Ok(match mode {
        Some("bypassPermissions") => BYPASS,
        Some("default") | Some("acceptEdits") => ASK_BEYOND_WORKSPACE,
        Some("dontAsk") => NEVER_ASK,
        Some("plan") => READ_ONLY,
        Some("auto") => AUTO_REVIEW,
        Some(other) => {
            return Err(HarnessError::NoEquivalent {
                harness: cide_ipc::Harness::Codex,
                what: format!("`permission-mode: {other}`"),
            });
        }
        // The project default. `auto` and `bypass` keep M44's answer — the only flag under
        // which a task run can still commit — and `ask` is a person answering in the pane,
        // which the TUI makes possible.
        None => match unattended {
            Unattended::Auto | Unattended::Bypass => BYPASS,
            Unattended::Ask => ASK_BEYOND_WORKSPACE,
        },
    })
}

/// What codex's sandbox will stop a worktree run of this role from doing, said before it fails
/// inside the run rather than after. (M118; the grant since M119)
///
/// The two failures selfcraft's codex runs hit and could not name: a commit refused because the
/// git metadata was read-only, and an e2e runner that could not bind `/tmp/.X11-unix` or a
/// Blender render that stalled on denied audio. The first is fixed when [`RunPlan::git_dirs`]
/// is known (`git_dirs_known`), and said when it is not — a wrapper deciding the sandbox
/// (`inject_permissions` off), a repository that would not open, `plan`'s read-only sandbox. The
/// second is **one** denial, measured in M119: with the network off codex's seccomp filter
/// refuses every `AF_UNIX` socket, and a role's `needs:` turns the network on. So the sentence
/// says which of the two the role is in, and names the grant rather than `bypassPermissions`,
/// which drops the whole sandbox for what one switch fixes.
///
/// `None` for bypass, and for a mode codex refuses (the dispatch refusal says that instead). A
/// run in the project root is `None` too — it is told not to commit, and it stands where the
/// user does — unless its role allows commands, which cide writes into a worktree only.
pub fn sandbox_caveat(
    mode: Option<&str>,
    unattended: Unattended,
    worktree: bool,
    git_dirs_known: bool,
    inject_permissions: bool,
    grant: &crate::sandbox::SandboxGrant,
) -> Option<&'static str> {
    if !worktree {
        return (!grant.allow_commands.is_empty()).then_some(
            "its `allow-commands:` are written into a run's worktree, and this run stands in the \
             project root, so they do not apply here",
        );
    }
    if !inject_permissions {
        return Some(
            "a wrapper decides codex's sandbox here (Settings → Harness → Codex), so cide cannot \
             make the worktree's git metadata writable or apply the role's `needs:`; if that \
             sandbox is workspace-write, the run cannot commit",
        );
    }
    let policy = permission_policy(mode, unattended).ok()?;
    if policy == READ_ONLY {
        return Some(
            "codex runs it in a read-only sandbox (permission-mode: plan): it can neither edit nor commit",
        );
    }
    if !sandboxes_writes(policy) {
        return None;
    }
    Some(match (git_dirs_known, grant.wants_network()) {
        (true, true) if grant.needs(crate::sandbox::SandboxNeed::Gpu) => {
            "codex runs it in its workspace-write sandbox with the network on for the role's \
             `needs:` and the machine's GPU bound in for `needs: [gpu]`, so an Xvfb of its own, \
             audio and Vulkan on the real GPU work there; it can edit and commit in its worktree"
        }
        (true, true) => {
            "codex runs it in its workspace-write sandbox with the network on for the role's \
             `needs:`, so an Xvfb of its own and audio work there, but Vulkan and GL run on \
             llvmpipe without `needs: [gpu]`; it can edit and commit in its worktree"
        }
        (true, false) => {
            "codex runs it in its workspace-write sandbox: it can edit and commit in its worktree, \
             but the sandbox refuses unix sockets while the network is off, so it has no X display \
             and no audio and an Xvfb e2e run or a Blender render fails there, and it has no GPU; \
             a role that needs them says `needs: [display, audio, gpu]`, or lets the one command \
             out with `allow-commands:`"
        }
        (false, _) => {
            "codex runs it in its workspace-write sandbox and the worktree's git directories could \
             not be resolved, so it cannot commit"
        }
    })
}

/// What the run itself is told about its sandbox, appended to its brief. (M119)
///
/// [`sandbox_caveat`] is the same facts for a person; this is them for the model, as
/// instructions, because a run that is not told plans the capture it cannot make and finds out
/// by failing — t-586's Blender render, t-588's Xvfb run, retried. `rules_written` is whether
/// cide put the role's `allow-commands` where codex reads them (a worktree run only); the list
/// is repeated here with the one rule that decides whether codex lets a command out — every part
/// of it must be allowed, so a `timeout` or a pipe around it keeps it in. `gpu_bound` is whether
/// the caller wrote the `needs: [gpu]` shim ([`RunPlan::codex_path_prepend`]). (M125)
// Eight flat facts from one call site, each a separate decision the caller has already made; a
// struct for them would be a second copy of `RunPlan`'s fields with nothing gained.
#[allow(clippy::too_many_arguments)]
pub fn sandbox_brief(
    mode: Option<&str>,
    unattended: Unattended,
    worktree: bool,
    git_dirs_known: bool,
    inject_permissions: bool,
    grant: &crate::sandbox::SandboxGrant,
    rules_written: bool,
    gpu_bound: bool,
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let sandboxed = inject_permissions
        && permission_policy(mode, unattended)
            .ok()
            .is_some_and(|policy| policy == READ_ONLY || sandboxes_writes(policy));
    if sandboxed {
        let policy = permission_policy(mode, unattended).ok();
        if policy == Some(READ_ONLY) {
            lines.push("Your sandbox is read-only: you cannot edit files or commit here.".into());
        } else {
            if worktree && !git_dirs_known {
                lines.push(
                    "Your sandbox cannot write this worktree's git metadata, so `git commit` will \
                     fail: do not retry it — say in your comment that the work is uncommitted."
                        .into(),
                );
            }
            if grant.wants_network() {
                lines.push(
                    "Your sandbox has the network on for this role's needs, so unix sockets work: \
                     you may start an X server of your own (`Xvfb :<n> -nolisten tcp`) and programs \
                     that open audio will not hang."
                        .into(),
                );
                // The GPU: codex binds a minimal `/dev` with no `/dev/dri` or `/dev/nvidia*`
                // (0.157.1), and only `needs: [gpu]`'s bwrap shim puts them back (M125). Without
                // it Vulkan and GL fall to llvmpipe, which is what selfcraft's e2e runs got —
                // Godot's Forward+ fell back to opengl3, or aborted. `gpu_bound` is whether the
                // shim was actually written, not whether the role asked: a run whose shim could
                // not be written is told it has no GPU rather than promised one. Said either
                // way, so a run does not debug a device.
                if gpu_bound {
                    lines.push(
                        "This role needs the GPU, so your sandbox has the machine's real devices \
                         (/dev/dri, /dev/nvidia*) bound in: Vulkan and CUDA see the real GPU, not \
                         llvmpipe. Do not force software rendering."
                            .into(),
                    );
                } else {
                    lines.push(
                        "There is no GPU in this sandbox: Vulkan and GL run on llvmpipe (software). \
                         Work that needs the GPU goes through one of the role's allowed commands, \
                         which run outside the sandbox with the real devices, or needs the role to \
                         say `needs: [gpu]`; if neither is there, say so in your comment rather \
                         than debugging the device."
                            .into(),
                    );
                }
            } else {
                lines.push(
                    "Your sandbox refuses unix sockets (the network is off), so it has no X display \
                     and no audio: do not start Xvfb, a windowed Godot, Blender or anything that \
                     opens PulseAudio here — they fail, or hang and never exit. Leave captures and \
                     renders to the reviewer, and say so in your comment."
                        .into(),
                );
            }
        }
    }
    if rules_written && !grant.allow_commands.is_empty() {
        let list = grant
            .allow_commands
            .iter()
            .map(|command| format!("`{command}`"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "These commands run without asking{}: {list}. Only when you invoke one directly, as \
             a whole command — under `timeout`, after `cd … &&`, in a pipe or inside `sh -c` it is \
             treated as any other command. Something started this way is not stopped by the \
             sandbox if it hangs, so prefer a script that exits by itself.",
            if sandboxed {
                " and outside the sandbox"
            } else {
                ""
            }
        ));
    }
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// One sentence for a [`cide_core::child_env::FilterError`], read in a settings dialog.
fn describe(error: &cide_core::child_env::FilterError) -> String {
    use cide_core::child_env::FilterError;
    match error {
        FilterError::Spawn(io) => format!("`codex debug models` could not be started: {io}"),
        FilterError::Timeout => "`codex debug models` did not answer in time and was stopped. \
                                 The catalog may be refreshing from the network — the model can \
                                 still be typed in below."
            .to_string(),
        FilterError::Unreadable => {
            "`codex debug models` ran and its output could not be read".to_string()
        }
        FilterError::Wait(io) => format!("`codex debug models` ran and could not be reaped: {io}"),
    }
}

/// How long `codex debug models` is given before it is killed. Measured well under a second
/// with a warm cache, and an order of magnitude plus a network round trip on top, because the
/// catalog refreshes on a stale etag — the cost of the ceiling is a hint under a typable box.
const MODELS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// The shape of `codex debug models`'s document, in the two fields the probe reads.
#[derive(Debug, Deserialize)]
struct Catalog {
    #[serde(default)]
    models: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct CatalogEntry {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    visibility: Option<String>,
}

/// The listed slugs in the catalog's order, without repeats.
///
/// Skips to the first `{` first: a warning the CLI prints above its document — an update
/// notice, a deprecation — would otherwise be a parse failure, and the whole point of a probe
/// is that "today it prints nothing else" is not a guarantee.
fn model_slugs(stdout: &str) -> Result<Vec<String>, String> {
    let start = stdout.find('{').ok_or_else(|| {
        "`codex debug models` printed nothing that looks like its catalog".to_string()
    })?;
    let catalog: Catalog = serde_json::from_str(&stdout[start..]).map_err(|_| {
        "`codex debug models` printed something that is not its JSON catalog".to_string()
    })?;
    let mut seen = std::collections::HashSet::new();
    Ok(catalog
        .models
        .into_iter()
        .filter(|entry| entry.visibility.as_deref() == Some("list"))
        .map(|entry| entry.slug)
        .filter(|slug| !slug.is_empty() && !slug.chars().any(char::is_whitespace))
        .filter(|slug| seen.insert(slug.clone()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, TaskId, Theme};
    use codex_cli::toml_string;

    use crate::LoadedAgent;

    const THREAD: &str = "0199a8f2-6d1e-7c3a-9b2e-4f5d6a7b8c9d";

    fn role() -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope: cide_ipc::agents::AgentScope::Project,
                harness: cide_ipc::Harness::Codex,
                description: "Implements one task end to end.".into(),
                system_prompt: "You are the developer agent. Finish the task.".into(),
                model: None,
                color: None,
                unavailable: None,
                max_concurrent: 1,
                worktree: true,
            },
            origin: PathBuf::from("/repo/.cide/agents/developer.md"),
            shadows: None,
            tools: Vec::new(),
            permission_mode: None,
            effort: None,
            extras: Vec::new(),
            sandbox: Default::default(),
        }
    }

    fn plan_for<'a>(agent: &'a LoadedAgent, session: SessionId) -> RunPlan<'a> {
        RunPlan {
            run: RunId::new(),
            session,
            agent,
            cwd: PathBuf::from("/repo/.cide/worktrees/developer-t-14"),
            project: ProjectId::new(),
            task: Some(TaskId("t-14".into())),
            task_title: Some("Teach the parser about tabs".into()),
            change: None,
            spec_cli: None,
            spec_apply: None,
            prompt: "Work on task t-14 (Teach the parser about tabs).".into(),
            hook_bin: Some(PathBuf::from("/opt/cide/cide-hook")),
            hook_sock: Some(PathBuf::from("/run/user/1000/cide-hooks-42.sock")),
            agent_sock: Some(PathBuf::from("/run/user/1000/cide-agents-42.sock")),
            events_path: Some(PathBuf::from("/run/user/1000/cide-run-7.events")),
            theme: Theme::Dark,
            proxy: cide_core::proxy::ProxyEnv::default(),
            env: Vec::new(),
            geometry: Geometry::default(),
            claude: cide_ipc::ClaudeSettings::default(),
            codex: cide_ipc::CodexSettings::default(),
            llm: cide_ipc::LlmSettings::default(),
            choice: None,
            harness: agent.def.harness,
            unattended: Unattended::Ask,
            tracker_paragraphs: true,
            server: None,
            git_dirs: Vec::new(),
            sandbox_brief: None,
            codex_trust_root: None,
            codex_path_prepend: None,
        }
    }

    fn at(args: &[String], token: &str) -> usize {
        args.iter()
            .position(|a| a == token)
            .unwrap_or_else(|| panic!("no {token} in {args:?}"))
    }

    /// The value of the one `-c <key>=…` override for `key`, panicking on none or two.
    fn config_value(args: &[String], key: &str) -> String {
        let prefix = format!("{key}=");
        let values: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *i > 0 && args[i - 1] == "-c" && a.starts_with(&prefix))
            .map(|(_, a)| a)
            .collect();
        assert_eq!(values.len(), 1, "one `{key}` override: {args:?}");
        values[0][prefix.len()..].to_string()
    }

    fn has_config(args: &[String], key: &str) -> bool {
        let prefix = format!("{key}=");
        args.iter()
            .enumerate()
            .any(|(i, a)| i > 0 && args[i - 1] == "-c" && a.starts_with(&prefix))
    }

    fn env_value<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The whole shape: the TUI (no `exec`), at the pane's geometry, bound by the caller, the
    /// prompt positional and last, and cide's routing keys in the environment.
    #[test]
    fn a_run_is_the_interactive_tui_with_the_prompt_last() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let spawned = CodexHarness.spawn_spec(&plan).expect("spawnable");
        let args = &spawned.spec.args;

        assert_eq!(spawned.spec.program, "codex");
        assert!(
            !args.iter().any(|a| a == "exec" || a == "--json"),
            "{args:?}"
        );
        assert_eq!(args[at(args, "-C") + 1], plan.cwd.to_string_lossy());
        assert_eq!(args.last().map(String::as_str), Some(plan.prompt.as_str()));
        assert!(spawned.opening.is_none(), "the prompt travels in the argv");
        assert!(matches!(spawned.binding, SessionBinding::Caller));
        assert!(spawned.events.is_none());

        // The modals that eat a first key are switched off.
        assert_eq!(config_value(args, "check_for_update_on_startup"), "false");
        assert_eq!(
            config_value(args, "notice.hide_rate_limit_model_nudge"),
            "true"
        );

        assert_eq!(
            env_value(&spawned.spec, "CIDE_SESSION"),
            Some(session.to_string().as_str())
        );
        assert_eq!(
            env_value(&spawned.spec, "CIDE_RUN"),
            Some(plan.run.to_string().as_str())
        );
        assert_eq!(
            env_value(&spawned.spec, "CIDE_HOOK_SOCK"),
            Some("/run/user/1000/cide-hooks-42.sock")
        );
    }

    #[test]
    fn a_review_can_use_the_checkout_without_approval_prompts() {
        let agent = role();
        let session = SessionId::new();
        let mut plan = plan_for(&agent, session);
        plan.tracker_paragraphs = false;
        plan.codex.cli.inject.permissions = false;
        plan.codex.cli.inject.review_permissions = true;

        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert!(args.windows(2).any(|pair| pair == ["-a", "on-request"]));
        assert_eq!(config_value(&args, "approvals_reviewer"), "\"auto_review\"");
        assert!(!args.iter().any(|arg| arg == "--approve-for-me"));
        assert!(!args.iter().any(|arg| arg == "-s"));
    }

    /// The hook table: one override per codex event, then the trust bypass without which none
    /// of them fires — and none of it without a `cide-hook` or with the injection off.
    #[test]
    fn the_hooks_ride_as_overrides_with_the_trust_bypass() {
        let agent = role();
        let plan = plan_for(&agent, SessionId::new());
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        for event in HookEvent::CODEX {
            let value = config_value(&args, &format!("hooks.{event}"));
            assert!(
                value.contains(&format!("command=\"/opt/cide/cide-hook {event}\"")),
                "{value}"
            );
        }
        assert!(
            !has_config(&args, "hooks.Notification"),
            "codex has no such event"
        );
        assert!(args.iter().any(|a| a == codex_cli::BYPASS_HOOK_TRUST));

        let mut off = plan_for(&agent, SessionId::new());
        off.codex.cli.inject.hooks = false;
        let args = CodexHarness.spawn_spec(&off).expect("spawnable").spec.args;
        assert!(!args.iter().any(|a| a.starts_with("hooks.")), "{args:?}");
        assert!(!args.iter().any(|a| a == codex_cli::BYPASS_HOOK_TRUST));

        let mut bare = plan_for(&agent, SessionId::new());
        bare.hook_bin = None;
        let args = CodexHarness.spawn_spec(&bare).expect("spawnable").spec.args;
        assert!(!args.iter().any(|a| a.starts_with("hooks.")), "{args:?}");
    }

    #[test]
    fn the_brief_rides_in_one_toml_encoded_developer_instructions_override() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;

        let brief = developer_brief(&plan);
        assert_eq!(
            brief,
            format!(
                "You are the developer agent. Finish the task.\n\n{}",
                tracker_preamble(&CodexHarness)
            )
        );
        assert!(brief.contains("mcp__cide__cide_task_get"), "{brief}");
        assert_eq!(
            config_value(&args, "developer_instructions"),
            toml_string(&brief)
        );

        let mut adhoc = plan_for(&agent, session);
        adhoc.task = None;
        // No title either: a run with a title and no task is one on external work (M104), which
        // has a worktree and is told to commit — `RunPlan::adhoc`.
        adhoc.task_title = None;
        adhoc.change = Some("m28-openspec".into());
        let brief = developer_brief(&adhoc);
        assert!(brief.ends_with(ADHOC_PREAMBLE), "{brief}");
        // External work keeps its title and loses the ad-hoc paragraph: it stands in a worktree
        // of its own and must commit there.
        let mut external = plan_for(&agent, session);
        external.task = None;
        assert!(!external.adhoc());
        assert!(!developer_brief(&external).contains(ADHOC_PREAMBLE));

        let mut silent = role();
        silent.def.system_prompt = "   ".into();
        let mut plan = plan_for(&silent, session);
        plan.hook_bin = None;
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert!(!has_config(&args, "developer_instructions"), "{args:?}");

        let mut off = plan_for(&agent, session);
        off.codex.cli.inject.developer_instructions = false;
        let args = CodexHarness.spawn_spec(&off).expect("spawnable").spec.args;
        assert!(!has_config(&args, "developer_instructions"), "{args:?}");
    }

    /// cide's server, with the three variables the bridge would otherwise never see.
    #[test]
    fn the_tracker_is_a_config_override_with_its_own_environment() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;

        assert_eq!(
            config_value(&args, "mcp_servers.cide.command"),
            "\"/opt/cide/cide-hook\""
        );
        assert_eq!(config_value(&args, "mcp_servers.cide.args"), r#"["mcp"]"#);
        assert_eq!(
            config_value(&args, "mcp_servers.cide.env.CIDE_RUN"),
            toml_string(&plan.run.to_string())
        );
        assert_eq!(
            config_value(&args, "mcp_servers.cide.env.CIDE_SESSION"),
            toml_string(&session.to_string())
        );
        assert_eq!(
            config_value(&args, "mcp_servers.cide.env.CIDE_AGENT_SOCK"),
            "\"/run/user/1000/cide-agents-42.sock\""
        );

        let mut off = plan_for(&agent, session);
        off.codex.cli.inject.mcp_config = false;
        let args = CodexHarness.spawn_spec(&off).expect("spawnable").spec.args;
        assert!(!args.iter().any(|a| a.contains("mcp_servers.")), "{args:?}");
    }

    /// Every row of the mapping, the project default, and the structural refusals.
    #[test]
    fn a_wrapper_that_decides_the_sandbox_is_given_no_policy_flags() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let on = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert!(
            on.iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox" || a == "-s"),
            "{on:?}"
        );
        let mut off = plan_for(&agent, session);
        off.codex.cli.inject.permissions = false;
        let args = CodexHarness.spawn_spec(&off).expect("spawnable").spec.args;
        for flag in [
            "-s",
            "-a",
            "--dangerously-bypass-approvals-and-sandbox",
            "--approve-for-me",
        ] {
            assert!(!args.iter().any(|a| a == flag), "{flag} in {args:?}");
        }
    }

    /// A sandboxed run in a worktree is given its git metadata as writable roots, and only a
    /// sandboxed one: bypass has no sandbox to widen, `plan` must stay read-only, and a wrapper
    /// that decides the sandbox is given nothing of cide's. (M118, selfcraft t-572)
    #[test]
    fn a_worktree_run_can_write_its_git_metadata() {
        let dirs = [
            std::path::PathBuf::from("/work/game/.git/worktrees/qa-t-572"),
            std::path::PathBuf::from("/work/game/.git"),
        ];
        let session = SessionId::new();
        let args_for_under = |mode: Option<&str>, inject: bool, unattended: Unattended| {
            let mut agent = role();
            agent.permission_mode = mode.map(str::to_string);
            let mut plan = plan_for(&agent, session);
            plan.git_dirs = dirs.to_vec();
            plan.unattended = unattended;
            plan.codex.cli.inject.permissions = inject;
            CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args
        };
        let args_for = |mode, inject| args_for_under(mode, inject, Unattended::Auto);
        let added = |args: &[String]| -> Vec<String> {
            args.windows(2)
                .filter(|w| w[0] == "--add-dir")
                .map(|w| w[1].clone())
                .collect()
        };
        let wanted: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        for mode in ["auto", "default", "acceptEdits", "dontAsk"] {
            assert_eq!(added(&args_for(Some(mode), true)), wanted, "{mode}");
        }
        for mode in [Some("plan"), Some("bypassPermissions"), None] {
            assert!(added(&args_for(mode, true)).is_empty(), "{mode:?}");
        }
        assert!(
            added(&args_for(Some("auto"), false)).is_empty(),
            "a wrapper decides"
        );
        assert_eq!(
            added(&args_for_under(None, true, Unattended::Ask)),
            wanted,
            "a project that asks is sandboxed, and its person should not have to approve a commit"
        );

        // A run in the root has no list, and nothing is added.
        let mut agent = role();
        agent.permission_mode = Some("auto".into());
        let args = CodexHarness
            .spawn_spec(&plan_for(&agent, session))
            .expect("spawnable")
            .spec
            .args;
        assert!(!args.iter().any(|a| a == "--add-dir"), "{args:?}");
    }

    #[test]
    fn the_sandbox_caveat_is_said_only_where_the_sandbox_denies_something() {
        let none = crate::sandbox::SandboxGrant::default();
        let caveat = |mode, worktree, known, inject| {
            sandbox_caveat(mode, Unattended::Auto, worktree, known, inject, &none)
        };
        assert!(caveat(Some("auto"), true, true, true).is_some_and(|s| s.contains("X display")));
        assert!(
            caveat(Some("auto"), true, false, true).is_some_and(|s| s.contains("cannot commit"))
        );
        assert!(caveat(Some("plan"), true, true, true).is_some_and(|s| s.contains("read-only")));
        assert!(caveat(Some("auto"), true, true, false).is_some_and(|s| s.contains("wrapper")));
        assert_eq!(caveat(Some("bypassPermissions"), true, true, true), None);
        assert_eq!(
            caveat(None, true, true, true),
            None,
            "the project default is bypass"
        );
        assert_eq!(
            caveat(Some("auto"), false, true, true),
            None,
            "the root is not asked to commit"
        );
        assert_eq!(
            caveat(Some("manual"), true, true, true),
            None,
            "refused at dispatch instead"
        );
        assert!(
            sandbox_caveat(None, Unattended::Ask, true, true, true, &none).is_some(),
            "a project that asks is sandboxed"
        );
        // The way out it names is the grant, not the bypass (M119).
        assert!(caveat(Some("auto"), true, true, true).is_some_and(|s| s.contains("needs:")));
        let display = crate::sandbox::SandboxGrant {
            needs: vec![crate::sandbox::SandboxNeed::Display],
            ..Default::default()
        };
        assert!(
            sandbox_caveat(Some("auto"), Unattended::Auto, true, true, true, &display).is_some_and(
                |s| s.contains("network on")
                    && !s.contains("no X display")
                    && s.contains("llvmpipe")
            )
        );
        let gpu = crate::sandbox::SandboxGrant {
            needs: vec![crate::sandbox::SandboxNeed::Gpu],
            ..Default::default()
        };
        assert!(
            sandbox_caveat(Some("auto"), Unattended::Auto, true, true, true, &gpu)
                .is_some_and(|s| s.contains("real GPU") && !s.contains("llvmpipe"))
        );
        let allowed = crate::sandbox::SandboxGrant {
            allow_commands: vec!["blender -b".into()],
            ..Default::default()
        };
        assert!(
            sandbox_caveat(Some("auto"), Unattended::Auto, false, true, true, &allowed)
                .is_some_and(|s| s.contains("project root")),
            "a root run is told its allow-commands do not reach it"
        );
    }

    /// The grant on the command line (M119): `needs` is codex's network switch plus Mesa for a
    /// display, `writable-dirs` are `--add-dir`s, and all of it only under a writes sandbox;
    /// the trust `-c` whenever cide wrote the rules file.
    #[test]
    fn a_grant_reaches_codex_only_where_its_sandbox_is_on() {
        let session = SessionId::new();
        let mut agent = role();
        agent.permission_mode = Some("auto".into());
        agent.sandbox = crate::sandbox::SandboxGrant {
            needs: vec![crate::sandbox::SandboxNeed::Display],
            writable_dirs: vec!["/srv/assets".into()],
            allow_commands: vec!["blender -b".into()],
        };
        let mut plan = plan_for(&agent, session);
        plan.codex_trust_root = Some(PathBuf::from("/p"));
        plan.sandbox_brief = Some("BRIEF-LINE".into());
        let spawn = CodexHarness.spawn_spec(&plan).expect("spawnable");
        let args = &spawn.spec.args;
        assert!(
            args.iter()
                .any(|a| a == "sandbox_workspace_write.network_access=true"),
            "{args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--add-dir" && w[1] == "/srv/assets"),
            "{args:?}"
        );
        assert!(
            args.iter()
                .any(|a| a == "projects.\"/p\".trust_level=\"trusted\""),
            "{args:?}"
        );
        assert!(developer_brief(&plan).ends_with("BRIEF-LINE"));

        agent.permission_mode = Some("bypassPermissions".into());
        let plan = plan_for(&agent, session);
        let args = CodexHarness.spawn_spec(&plan).expect("spawnable").spec.args;
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("sandbox_workspace_write")),
            "no sandbox, nothing to widen: {args:?}"
        );
    }

    #[test]
    fn the_run_is_told_what_its_sandbox_denies_and_what_it_may_run() {
        let none = crate::sandbox::SandboxGrant::default();
        let brief = |grant: &crate::sandbox::SandboxGrant, rules| {
            sandbox_brief(
                Some("auto"),
                Unattended::Auto,
                true,
                true,
                true,
                grant,
                rules,
                false,
            )
        };
        assert!(brief(&none, false).is_some_and(|b| b.contains("do not start Xvfb")));
        let grant = crate::sandbox::SandboxGrant {
            needs: vec![crate::sandbox::SandboxNeed::Audio],
            allow_commands: vec!["blender -b".into()],
            ..Default::default()
        };
        let told = brief(&grant, true).expect("said");
        assert!(
            told.contains("`blender -b`") && told.contains("outside the sandbox"),
            "{told}"
        );
        assert!(!told.contains("do not start Xvfb"), "{told}");
        assert_eq!(
            sandbox_brief(
                Some("bypassPermissions"),
                Unattended::Auto,
                true,
                true,
                true,
                &none,
                false,
                false
            ),
            None
        );
        // `needs: [gpu]` (M125): promised the real GPU only when the shim was written; told
        // there is none — and how to get one — otherwise.
        let gpu = crate::sandbox::SandboxGrant {
            needs: vec![crate::sandbox::SandboxNeed::Gpu],
            ..Default::default()
        };
        let bound = sandbox_brief(
            Some("auto"),
            Unattended::Auto,
            true,
            true,
            true,
            &gpu,
            false,
            true,
        )
        .expect("said");
        assert!(
            bound.contains("real GPU") && !bound.contains("no GPU"),
            "{bound}"
        );
        let unbound = brief(&gpu, false).expect("said");
        assert!(
            unbound.contains("no GPU") && unbound.contains("needs: [gpu]"),
            "{unbound}"
        );
    }

    /// The shim's directory goes first on codex's `PATH` exactly where the grant reaches codex:
    /// a writes sandbox cide decides, a role that needs the GPU, and a shim that was written.
    /// (M125)
    #[test]
    fn a_gpu_role_gets_the_bwrap_shim_first_on_its_path() {
        let session = SessionId::new();
        let shim = PathBuf::from("/run/user/1000/cide-run-7.bin");
        let path_of = |mode: Option<&str>,
                       needs: Vec<crate::sandbox::SandboxNeed>,
                       inject,
                       dir: Option<&PathBuf>| {
            let mut agent = role();
            agent.permission_mode = mode.map(str::to_string);
            agent.sandbox.needs = needs;
            let mut plan = plan_for(&agent, session);
            plan.codex.cli.inject.permissions = inject;
            plan.codex_path_prepend = dir.cloned();
            let spec = CodexHarness.spawn_spec(&plan).expect("spawnable").spec;
            env_value(&spec, "PATH").map(str::to_string)
        };
        let gpu = || vec![crate::sandbox::SandboxNeed::Gpu];
        let first = |path: Option<String>| {
            path.is_some_and(|p| p.starts_with("/run/user/1000/cide-run-7.bin:"))
        };
        for mode in ["auto", "default", "dontAsk"] {
            assert!(
                first(path_of(Some(mode), gpu(), true, Some(&shim))),
                "{mode}"
            );
        }
        assert!(!first(path_of(
            Some("bypassPermissions"),
            gpu(),
            true,
            Some(&shim)
        )));
        assert!(!first(path_of(Some("plan"), gpu(), true, Some(&shim))));
        assert!(
            !first(path_of(Some("auto"), gpu(), false, Some(&shim))),
            "a wrapper decides"
        );
        assert!(!first(path_of(
            Some("auto"),
            vec![crate::sandbox::SandboxNeed::Display],
            true,
            Some(&shim)
        )));
        assert!(
            !first(path_of(Some("auto"), gpu(), true, None)),
            "no shim written"
        );
        // The shim is one more directory in front, not a replacement: the rest stays.
        let path = path_of(Some("auto"), gpu(), true, Some(&shim)).expect("a PATH");
        assert!(
            path.len() > "/run/user/1000/cide-run-7.bin:".len(),
            "{path}"
        );
    }

    #[test]
    fn the_definitions_vocabulary_maps_to_a_sandbox_and_an_approval_policy() {
        let policy = |mode: Option<&str>, unattended| {
            permission_policy(mode, unattended)
                .expect("mapped")
                .to_vec()
        };
        assert_eq!(
            policy(Some("bypassPermissions"), Unattended::Ask),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert_eq!(
            policy(Some("default"), Unattended::Bypass),
            ["-s", "workspace-write", "-a", "on-request"],
            "the TUI can ask, so `default` is finally expressible"
        );
        assert_eq!(
            policy(Some("acceptEdits"), Unattended::Ask),
            ["-s", "workspace-write", "-a", "on-request"]
        );
        assert_eq!(
            policy(Some("dontAsk"), Unattended::Ask),
            ["-s", "workspace-write", "-a", "never"]
        );
        assert_eq!(
            policy(Some("plan"), Unattended::Bypass),
            ["-s", "read-only", "-a", "on-request"]
        );
        assert_eq!(policy(Some("auto"), Unattended::Ask), ["--approve-for-me"]);
        assert_eq!(
            policy(None, Unattended::Auto),
            ["--dangerously-bypass-approvals-and-sandbox"],
            "an unattended run keeps being able to commit"
        );
        assert_eq!(
            policy(None, Unattended::Ask),
            ["-s", "workspace-write", "-a", "on-request"],
            "never codex's own default, which may open a trust chooser first"
        );
        assert!(matches!(
            permission_policy(Some("manual"), Unattended::Ask),
            Err(HarnessError::NoEquivalent { .. })
        ));

        let mut tuned = role();
        tuned.def.model = Some("gpt-5.5".into());
        tuned.effort = Some("high".into());
        tuned.tools = vec!["Read".into()];
        let session = SessionId::new();
        let args = CodexHarness
            .spawn_spec(&plan_for(&tuned, session))
            .expect("spawnable")
            .spec
            .args;
        assert_eq!(args[at(&args, "-m") + 1], "gpt-5.5");
        assert_eq!(config_value(&args, "model_reasoning_effort"), "\"high\"");
        assert!(!args.iter().any(|a| a == "Read"), "tools: is dropped");

        let mut claude = role();
        claude.def.harness = cide_ipc::Harness::Claude;
        assert!(matches!(
            CodexHarness.spawn_spec(&plan_for(&claude, session)),
            Err(HarnessError::WrongHarness { .. })
        ));
        let mut empty = plan_for(&tuned, session);
        empty.prompt = "  \n".into();
        assert!(matches!(
            CodexHarness.spawn_spec(&empty),
            Err(HarnessError::NoPrompt)
        ));
    }

    /// The configured binary and the user's arguments reach a run, the user's first and a
    /// refused one nowhere.
    #[test]
    fn a_run_launches_the_codex_settings_configure() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        plan.codex.cli.binary = "/opt/wrap/codex-wrapper".into();
        plan.codex.cli.args = vec!["--search".into(), "-c".into(), "hooks.Stop=[]".into()];
        let spec = CodexHarness.spawn_spec(&plan).expect("spawnable").spec;
        assert_eq!(spec.program, "/opt/wrap/codex-wrapper");
        assert_eq!(spec.args[0], "--search", "the user's arguments come first");
        assert!(
            !spec.args.iter().any(|a| a == "hooks.Stop=[]"),
            "{:?}",
            spec.args
        );
    }

    /// Back on the thread: `resume` first, the thread and then the follow-up last, and the
    /// routing id unchanged.
    #[test]
    fn a_respawn_resumes_the_thread_the_first_child_announced() {
        let agent = role();
        let session = SessionId::new();
        let mut plan = plan_for(&agent, session);
        plan.prompt = "Now add a test.".into();
        let spec = CodexHarness
            .respawn_spec(&plan, &format!(" {THREAD} "))
            .expect("resumable")
            .spec;
        let args = &spec.args;
        assert_eq!(args[0], "resume");
        let n = args.len();
        assert_eq!(args[n - 2], THREAD);
        assert_eq!(args[n - 1], "Now add a test.");
        assert_eq!(
            env_value(&spec, "CIDE_SESSION"),
            Some(session.to_string().as_str())
        );

        // Nothing new to say is legal on a resume, and then the thread is last.
        plan.prompt = String::new();
        let args = CodexHarness
            .respawn_spec(&plan, THREAD)
            .expect("resumable")
            .spec
            .args;
        assert_eq!(args.last().map(String::as_str), Some(THREAD));

        assert!(matches!(
            CodexHarness.respawn_spec(&plan, "thread_abc"),
            Err(HarnessError::NotAConversation { .. })
        ));
    }

    /// The thread comes off `SessionStart` and nothing else; the frames move the run the way a
    /// claude run's do, plus the approval prompt; only the exit ends it.
    #[test]
    fn hooks_name_the_thread_and_move_the_run() {
        let frame = |event: &str| {
            HookFrame::new(
                event,
                serde_json::json!({"session_id": THREAD, "hook_event_name": event}),
            )
        };
        assert_eq!(
            CodexHarness.capture_hook(&frame("SessionStart")).as_deref(),
            Some(THREAD)
        );
        assert_eq!(CodexHarness.capture_hook(&frame("Stop")), None);

        let observe = |current: RunState, event: &str| {
            CodexHarness.observe(current, Observation::Hook(&frame(event)))
        };
        assert_eq!(
            observe(RunState::Starting, "SessionStart"),
            Some(RunState::Idle)
        );
        assert_eq!(
            observe(RunState::Idle, "UserPromptSubmit"),
            Some(RunState::Running)
        );
        assert_eq!(
            observe(RunState::Running, "PermissionRequest"),
            Some(RunState::AwaitingPermission)
        );
        assert_eq!(
            observe(RunState::AwaitingPermission, "PostToolUse"),
            Some(RunState::Running)
        );
        assert_eq!(observe(RunState::Running, "Stop"), Some(RunState::Idle));
        assert_eq!(
            CodexHarness.observe(
                RunState::Running,
                Observation::Line("{\"type\":\"turn.completed\"}")
            ),
            None,
            "nothing is read off the screen"
        );
        assert_eq!(
            CodexHarness.observe(RunState::Idle, Observation::Exit(0)),
            Some(RunState::Finished { code: 0 })
        );
    }

    /// Typed as a paste then Enter, and stopped with one Esc.
    #[test]
    fn a_follow_up_is_a_paste_and_an_interrupt_is_esc() {
        assert_eq!(
            CodexHarness.deliver("two\nlines"),
            Delivery::Stdin(b"\x1b[200~two\nlines\x1b[201~\r".to_vec())
        );
        assert_eq!(CodexHarness.interrupt(), Some(vec![ESC]));
    }

    #[test]
    fn continuing_a_conversation_opens_the_tui_on_its_thread() {
        let spec = CodexHarness
            .continue_spec(&HarnessSession {
                harness: cide_ipc::Harness::Codex,
                id: format!(" {THREAD} "),
                cwd: PathBuf::from("/p/.cide/worktrees/developer-t-14"),
            })
            .expect("a uuid is a conversation");
        assert_eq!(spec.program, "codex");
        assert!(spec.args.is_empty());
        assert_eq!(
            spec.resume.map(|id| id.to_string()).as_deref(),
            Some(THREAD)
        );

        let refused = CodexHarness
            .continue_spec(&HarnessSession {
                harness: cide_ipc::Harness::Codex,
                id: "thread_abc".into(),
                cwd: PathBuf::from("/p"),
            })
            .expect_err("not a uuid");
        assert!(matches!(refused, HarnessError::NotAConversation { .. }));
        assert_eq!(
            CodexHarness.tool_name("cide_task_get"),
            "mcp__cide__cide_task_get"
        );
    }

    /// The catalog's listed slugs, in its order, once each; a warning above the document is
    /// absorbed; anything else is a sentence.
    #[test]
    fn the_model_catalog_keeps_listed_slugs_in_the_clis_order() {
        const CATALOG: &str = r#"{"fetched_at":"2026-09-07T07:36:48Z","etag":"x","client_version":"0.153.2","models":[{"slug":"gpt-5.6-terra","display_name":"GPT-5.6-Terra","visibility":"list"},{"slug":"gpt-reserve","visibility":"hide"},{"slug":"o3"},{"slug":"gpt-5.5","visibility":"list"},{"slug":"gpt-5.6-terra","visibility":"list"},{"slug":"gpt-5.4-mini","visibility":"list"}]}"#;
        assert_eq!(
            model_slugs(CATALOG).expect("a catalog"),
            ["gpt-5.6-terra", "gpt-5.5", "gpt-5.4-mini"]
        );
        assert_eq!(
            model_slugs(&format!("codex 0.153.4 is available\n{CATALOG}\n")).expect("absorbed"),
            ["gpt-5.6-terra", "gpt-5.5", "gpt-5.4-mini"]
        );
        assert!(model_slugs("").is_err());
        assert!(model_slugs("{not json").is_err());
        assert_eq!(
            model_slugs(r#"{"models":[]}"#).expect("empty is honest"),
            Vec::<String>::new()
        );
    }

    /// The real catalog, from the machine's own `codex`: non-empty, and every slug a single word.
    #[test]
    #[ignore = "runs the real codex; needs the binary on PATH"]
    fn the_real_codex_lists_models() {
        let models = CodexHarness
            .models(None, &cide_ipc::LlmSettings::default())
            .expect("a catalog");
        assert!(!models.is_empty(), "{models:?}");
        assert!(
            models.iter().all(|m| !m.chars().any(char::is_whitespace)),
            "{models:?}"
        );
    }
}
