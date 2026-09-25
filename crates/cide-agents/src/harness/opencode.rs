//! Running a role on the opencode CLI. (M18)
//!
//! Measured against opencode **1.18** on a Linux machine, not remembered. Where a paragraph below
//! says *measured*, it means a command was run or the shipped binary was read; the two facts that
//! decided the whole shape of this file — that a whole configuration can be handed over in one
//! environment variable, and that the inline agent's `prompt` actually reaches the model — were
//! each probed twice, because the first probe of the second one produced a confident false
//! negative.
//!
//! # A run is one process per turn, and that is what [`Delivery::Respawn`] is
//!
//! `opencode run` reads one message, answers it, and exits: its event loop **breaks the moment
//! the session reports idle**, which is visible in the shipped binary and is why there is no
//! second prompt to write. `ClaudeHarness` hosts an interactive TUI and types a follow-up into
//! it; there is nothing here to type into. So a follow-up is a *new child* carrying
//! `--session <id>`, which continues the same conversation server-side — the whole reason
//! [`Delivery`] is an enum rather than a `write(bytes)` method the second implementation would
//! have had to lie to.
//!
//! [`Harness::respawn_spec`] is the other half of that answer: it builds the continuing child,
//! and it is defaulted-and-refusing on the trait precisely so a `Stdin` harness never has to
//! implement a method it would have nothing truthful to put in.
//!
//! # The configuration travels in the environment, and nothing is written into the project
//!
//! **`OPENCODE_CONFIG_CONTENT` carries a whole configuration document.** Measured: an inline
//! `agent` definition supplied only through that variable is registered — `opencode agent list`
//! run with it prints the role — and its `prompt` reaches the model, verified by putting facts in
//! it the model could not otherwise know and asking a neutral question about them. That is the
//! seam this file is built on, and it buys three things at once:
//!
//! * **No file in the user's project.** A run's cwd is a git worktree cide created and will
//!   eventually prune; writing an `opencode.json` into it would be cide editing a checkout on the
//!   user's behalf, and killing cide would leave it there.
//! * **A per-role system prompt**, which opencode has no flag for. There is no
//!   `--append-system-prompt` here, so the role's prompt is *configuration* rather than an
//!   argument — and emphatically not the first line of the message, where a model reads it as the
//!   user's own words rather than as its brief. [`super::TRACKER_PREAMBLE`] is appended to it,
//!   after the role's own words and only when the `mcp` block below is written, so a role reads
//!   the same two paragraphs here that `claude.rs` folds into `--append-system-prompt`.
//! * **cide's MCP server**, in the same document, so an opencode run reaches the task tracker
//!   exactly as a claude run does.
//!
//! Read last of all the config sources — measured in the binary, which merges the global file,
//! then the project's, then this — so it wins where it says anything and leaves everything else
//! alone.
//!
//! ## `OPENCODE_DISABLE_PROJECT_CONFIG` is deliberately **not** set
//!
//! It is the tempting wrong move, so it is named here to stop somebody "fixing" the merge with
//! it. A project's own `opencode.json` is **the user's configuration** — their models, their MCP
//! servers, their instructions — and cide switching it off would mean a run behaving unlike every
//! other opencode invocation in that repository, for reasons no file anywhere records. cide adds
//! a role and a server; it does not take the project's own configuration away.
//!
//! ## What the document does *not* carry, and why each absence is deliberate
//!
//! * **`tools`** — deprecated in the schema in favour of `permission`, so emitting it writes a
//!   key the next release may stop reading. A definition's `tools:` line is `--allowedTools`
//!   vocabulary, which is the *claude* CLI's; there is no mapping from it to opencode's
//!   permission rules that cide could write without inventing one, and a permission rule the
//!   role's author never wrote is the one kind of restriction that must not be invented.
//! * **`permission`** — the same argument. `LoadedAgent::permission_mode` is validated at load
//!   against the literal set in `claude --help`; translating `acceptEdits` into a permission
//!   table here would be cide making up a policy and attributing it to a file.
//! * **`variant`** — carried as the `--variant` flag instead, beside `--model`, because the
//!   schema says the config field applies *only when the agent's configured model is the one in
//!   use* and this run may name another with `--model`.
//!
//! `mode: "primary"` **is** emitted, and it is load-bearing rather than decorative: the shipped
//! binary refuses to select a `subagent`-mode agent from `--agent` and falls back to the default
//! agent with a warning, which would silently run somebody else's prompt.
//!
//! # The argv, and the wager this file does not have to take
//!
//! `opencode run --agent <role> [--model <provider/model>] [--variant <effort>] --format json
//! --dir <cwd> [--session <id> | --title <label>] <message>`
//!
//! `ClaudeHarness`'s argv order is load-bearing because a *user's* variadic flag can swallow the
//! tokens cide adds. **There is no user argument list on this side**: `ClaudeSettings::cli` is the
//! Claude pane's launch configuration and has nothing to do with this binary, so cide owns the
//! whole vector. The day an opencode launch-args setting exists, the user's tokens go first, for
//! the reason `claude.rs` gives at length.
//!
//! Two things about it are still worth knowing:
//!
//! * **The message is a positional argument**, unlike a claude run's, whose prompt is typed into
//!   the terminal. So [`HarnessSpawn::opening`] is `None` here, and the PTY-write rule that a
//!   prompt may not contain a newline — an embedded `\n` is another Enter — does not apply: an
//!   argv token may contain anything. It is still written as one line by the dispatch site, and
//!   that is worth keeping, because the *parser* has its own hazard: the message goes last, and a
//!   message beginning with `-` would be read as a flag by the CLI's option parser.
//! * **`--dir` and the spawn's cwd are both set, to the same directory.** The cwd is what the
//!   child and everything it starts inherit; `--dir` is what opencode resolves the project and
//!   the session store from. Setting only one of them gives a run whose tools and whose conversation
//!   disagree about where the work is.
//!
//! # The environment, and the variable that is deliberately absent
//!
//! | name | value | why |
//! | --- | --- | --- |
//! | `TERM`, `COLORTERM`, `TERM_PROGRAM`… | [`cide_core::child_env::terminal_child_env`] | the identical list a pane and a claude run get; one composition, not three |
//! | proxy variables | [`RunPlan::proxy`], resolved by the caller | one resolved answer per child, `cide-core`'s rule |
//! | `OPENCODE_CONFIG_CONTENT` | the role and cide's MCP server | see above |
//! | `CIDE_RUN` | the run id | what `cide-hook mcp`'s header line names, and what scopes the connection to this run's task tools |
//! | `CIDE_AGENT_SOCK` | the agent-RPC socket | what that bridge connects to |
//! | `CIDE_SESSION` | **not set** | see below |
//! | `CIDE_HOOK_SOCK` | **not set** | nothing here writes a hook frame |
//!
//! The `CLAUDE_CODE_*` switches in that list ride along and are **inert** here, exactly as they
//! are in a shell pane — `terminal_child_env`'s own header argues that case at length, and the
//! alternative (a second, harness-aware composition of the terminal environment) would be two
//! lists to keep in step so that one child could avoid four names it ignores.
//!
//! `terminal_child_env` is handed an **empty** `extra` list. That list is
//! `claude_cli::user_env` — the variables from the user's *Claude sessions* launch configuration
//! — and that function's own header records the rule: the four inert `CLAUDE_CODE_*` names may
//! reach any child, but an arbitrary `NODE_OPTIONS` or `GIT_SSH_COMMAND` from a field labelled
//! *the environment claude panes are spawned with* may not quietly become the environment of a
//! program that is not claude.
//!
//! **`CIDE_SESSION` buys nothing here, so it is not set.** For a claude run it is the routing key
//! that makes liveness free: `cide-hook` reads it out of the child's environment and echoes it
//! back on every one of ten hook points, so the state machine, the phase dot and the live cost
//! figure all arrive without cide watching anything. opencode has no hooks, no `--settings`, and
//! nothing that would read the variable — setting it would be a name in an environment that
//! nobody ever looks at, which is worse than nothing because the next reader would take it for a
//! working channel.
//!
//! ## So liveness comes from the child's own output, and from nowhere else
//!
//! `--format json` makes stdout **newline-delimited JSON, one object per line**, and
//! [`Harness::observe`] over [`Observation::Line`] is the *only* thing that moves an opencode
//! run's row. Two consequences worth stating rather than discovering:
//!
//! * A caller that does not feed the stream in leaves every opencode run sitting at
//!   [`RunState::Starting`] until its child exits. For a claude run the equivalent lapse is
//!   invisible, because hooks arrive anyway.
//! * `sessionID` is on **every** line including the first (measured), which is what makes
//!   [`SessionBinding::Harness`] a one-line scan rather than the three-step ladder the design
//!   sketched — scan for an id, else reconcile through `--title` and `opencode session list`,
//!   else give up and call a run fire-and-forget. None of that is needed and none of it is here.
//!
//! # Permissions, and why a run never sits waiting on one
//!
//! Measured in the shipped binary: `run` answers a permission request by **rejecting it** and
//! printing one line, unless `--auto` was passed. So there is no `AwaitingPermission` for this
//! harness and no wedged turn either.
//!
//! `--auto` was deliberately not passed at first — its own `--help` calls it dangerous, and the
//! theory was that opencode's default rules allow ordinary edits and commands, so the flag would
//! buy nothing except the ability to say yes to questions worth asking. **Live runs disproved
//! the theory's second half**: the first command that strayed past the project directory
//! (`cat ~/.cargo/config.toml`, on the way to a `cargo check`) was auto-rejected and the turn
//! **ended at the rejection** — two real runs died mid-investigation, exit 0, task never
//! reported, which reads on the board as an agent that did nothing. A refusal that ends the
//! turn is not a safety property in a headless child; it is the autonomy failing silently. So
//! `--auto` now rides [`RunPlan::unattended`] — the project-level
//! `agents.permissionMode`, `auto` by default, `manual` in one config line — and the containment for an
//! unattended child is what it always actually was: worktree isolation.
//!
//! # Opening a run into a pane shows the events **rendered**, and the raw line is still the channel
//!
//! `--format json` is what makes the run legible to cide, and it used to be what made the pane
//! show raw ndjson to a person — a cost this header once accepted, and the first live run
//! reported as unreadable. [`render_event`] is the reconciliation, and *where* it runs is the
//! design: `cide_pty::LineRender` rewrites the stream once, upstream of the mirror and every
//! sink, while the app's observer reads each **raw** line inside the same hook before the
//! rendering is returned. One rendered stream downstream (a rehydrated pane, a choked sink's
//! catch-up and a live pane cannot disagree), one raw channel upstream for `capture` and
//! [`Harness::observe`]. `opencode serve` plus `attach` remains the named upgrade path to a
//! *live* TUI in the pane; this makes the transcript readable without it.

use crate::config::Unattended;
use cide_ipc::RunState;
use cide_pty::{Geometry as PtyGeometry, Rendered, SpawnSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    ADHOC_PREAMBLE, ContinueSpec, Delivery, FailoverReason, Harness, HarnessError, HarnessSpawn,
    Observation, RunPlan, SERVER, SessionBinding, tracker_preamble,
};
use cide_ipc::HarnessSession;

use super::RenderState;
use super::render::{
    BOLD, CYAN, DIM, RED, RESET, TITLE_BUDGET, absorbs, clip, compact_input, compose, measured_ms,
    one_line_of, prose, stamp, tail, thought_line, thousands,
};

// Named from here since M42 — `SpawnSpec::fixed_size`'s reason is in `render.rs` now, and
// the path stays so nothing that learned it moves.
pub use super::render::{RUN_COLS, RUN_ROWS};

/// Which opencode-shaped CLI a run is on. (M81)
///
/// # Two CLIs, one implementation, and every difference named here
///
/// MiMo Code (`mimo`) is an **opencode fork**, measured on 0.1.15 against this file rather than
/// assumed from a name: `mimo run` takes the same flags (`--agent`, `--model`, `--variant`,
/// `--format json`, `--thinking`, `--dir`, `--session`, `--title`); a real turn printed the same
/// events in the same envelope — `step_start`, `reasoning`, `tool_use`, `text`, `step_finish`,
/// `sessionID: "ses_…"` on every line, `tokens: {total, input, output, reasoning, cache}` on the
/// finish; an inline role handed over in `MIMOCODE_CONFIG_CONTENT` is registered with its prompt
/// verbatim (`mimo debug agent <id>`); and MCP tools are named `<server>_<tool>` — the model
/// listed `deepwiki_ask_wiki_question` for a server named `deepwiki`.
///
/// So the renderer, the failover classifier, the usage reader, the state machine and the
/// provider document are **one** each, and a copied `mimo.rs` would have been a second spelling
/// of every row format `check:json-log` pins and of the document `provider_members`' header
/// insists has one producer. What *does* differ is a field on this struct, and nothing below it
/// may say `if mimo`: a difference that is not a field is a difference nobody can find.
#[derive(Debug)]
pub struct Flavor {
    /// The wire variant, which is also the binary's name through `defs::harness_binary`.
    pub kind: cide_ipc::Harness,
    /// The prefix every environment variable the CLI reads carries: `OPENCODE_CONFIG_CONTENT`,
    /// `MIMOCODE_CONFIG_CONTENT`. Measured in the `mimo` binary: all sixty-odd are opencode's
    /// under this rename, and not one `OPENCODE_*` name is read — so an `OPENCODE_CONFIG_CONTENT`
    /// on a mimo child would be a whole configuration nobody reads, and the run would silently
    /// be mimo's *default* agent.
    env_prefix: &'static str,
    /// The document's own contract, so a person reading a dumped configuration can look it up.
    schema: &'static str,
    /// The flag [`RunPlan::unattended`] becomes, for anything but `Ask`. opencode's `run` spells it `--auto`;
    /// mimo's has no `--auto` and spells it `--dangerously-skip-permissions`.
    skip_permissions: &'static str,
    /// Extra arguments on the TUI a finished run is re-opened in ([`Flavor::continue_spec`]).
    ///
    /// mimo's TUI asks whether to trust a workspace the first time it opens one, and a cide
    /// worktree is a directory it has never seen. `--trust` answers it: the run this pane
    /// continues already worked in that directory headless (`run` asks nothing), so the trust
    /// was extended when the role was dispatched and a prompt here would be a question about a
    /// decision already made.
    continue_args: &'static [&'static str],
    /// Whether a fatal provider error exits 0 — see [`Harness::failure_exits_zero`]. Also the
    /// flag that says this CLI's own retry policy must be **bounded** under a pool
    /// ([`pool_retry`]): the two are one fact about mimo, which retries silently until it gives
    /// up and then says so with a clean exit.
    failure_exits_zero: bool,
    /// Whether `models` annotates each id. mimo prints `xiaomi/mimo-v2.5 — window 1.05M,
    /// compacts at 944K`; opencode prints the bare id. Only the exact ` — ` separator is cut,
    /// and only for the flavour that prints it, so opencode's filter goes on refusing every
    /// line with whitespace in it — see [`is_model_id`].
    annotated_models: bool,
}

/// opencode itself.
pub static OPENCODE_CLI: Flavor = Flavor {
    kind: cide_ipc::Harness::Opencode,
    env_prefix: "OPENCODE",
    schema: "https://opencode.ai/config.json",
    skip_permissions: "--auto",
    continue_args: &[],
    failure_exits_zero: false,
    annotated_models: false,
};

/// MiMo Code, the opencode fork. (M81)
pub static MIMO_CLI: Flavor = Flavor {
    kind: cide_ipc::Harness::Mimo,
    env_prefix: "MIMOCODE",
    schema: "https://mimo.xiaomi.com/mimocode/config.json",
    skip_permissions: "--dangerously-skip-permissions",
    continue_args: &["--trust"],
    failure_exits_zero: true,
    annotated_models: true,
};

impl Flavor {
    /// The flavour for a harness, or `None` for one that is not opencode-shaped.
    #[must_use]
    pub fn of(harness: cide_ipc::Harness) -> Option<&'static Flavor> {
        match harness {
            cide_ipc::Harness::Opencode => Some(&OPENCODE_CLI),
            cide_ipc::Harness::Mimo => Some(&MIMO_CLI),
            cide_ipc::Harness::Claude | cide_ipc::Harness::Qwen | cide_ipc::Harness::Codex => None,
        }
    }

    /// The flavour a probe that is not about one role should run under: opencode when it is
    /// installed, else mimo when *that* is, else opencode — so the not-found sentence names the
    /// original. (M81)
    ///
    /// For `llm_test_model`, which tests a *provider*, not a harness: the document is the same
    /// for both CLIs, and a machine with only mimo on it must still be able to press Test.
    #[must_use]
    pub fn first_installed() -> &'static Flavor {
        [&OPENCODE_CLI, &MIMO_CLI]
            .into_iter()
            .find(|flavor| cide_core::toolchain::which(flavor.program()).is_some())
            .unwrap_or(&OPENCODE_CLI)
    }

    /// The binary's bare name, which is also what every sentence below calls it.
    #[must_use]
    pub fn program(&self) -> &'static str {
        crate::defs::harness_binary(self.kind)
    }

    /// The variable the whole configuration document travels in.
    fn config_env(&self) -> String {
        format!("{}_CONFIG_CONTENT", self.env_prefix)
    }

    /// The `Harness` this flavour is, for the helpers that spell tool names through one.
    fn harness(&self) -> &'static dyn Harness {
        match self.kind {
            cide_ipc::Harness::Mimo => &MimoHarness,
            _ => &OpencodeHarness,
        }
    }

    /// The installed binary, by the *same* search `defs::installed` uses, and the same sentence
    /// when it misses: a role this build would grey as uninstalled must not also grow a second,
    /// differently worded complaint about the same absence.
    fn binary(&self) -> Result<std::path::PathBuf, String> {
        cide_core::toolchain::which(self.program()).ok_or_else(|| {
            crate::defs::installed(self.kind)
                .unwrap_or_else(|| format!("`{}` could not be found", self.program()))
        })
    }
}

/// Which of cide's optional flags this installed CLI actually has. (M108)
///
/// # Why this is asked of the binary rather than assumed
///
/// Reported from a Mac: an opencode MR review printed `opencode run [message..]` and its whole
/// usage, and did nothing else. The run's post-mortem log had the argv (`# cide forked: …`), and
/// the usage it printed had **no `--auto` and no `--password`** — that machine's opencode predated
/// both. yargs is strict and prints its usage, with no reason line, on any flag it does not know,
/// so one unattended run's `--auto` turned every opencode run on that machine into a help page.
///
/// So the two flags cide adds on its own account are asked of `--help` once per binary (keyed by
/// path and modification time, so an upgrade is noticed) and passed only where they exist:
///
/// * **the skip-permissions flag** (`--auto`, MiMo's `--dangerously-skip-permissions`) — absent on
///   an older CLI, which predates the permission prompts it answers and allowed by default;
/// * **`--password`** — absent means a `run --attach` client cannot authenticate, so the run's
///   server (M105) could only be unguarded. The run goes standalone instead, as every run did
///   before M105.
///
/// Unknown — never probed, or the probe failed — reads as the current CLI's answer, which is what
/// every measurement in this file was taken against: a probe that cannot run must not quietly
/// switch a working machine's runs to prompting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliFlags {
    /// `run` takes this flavour's skip-permissions flag.
    pub run_skip_permissions: bool,
    /// `run` takes `--password`, so a turn can attach to a guarded server.
    pub run_password: bool,
    /// The TUI (`<cli> [project]`) takes the skip-permissions flag — M104's session tab.
    pub tui_skip_permissions: bool,
}

impl Default for CliFlags {
    fn default() -> Self {
        Self {
            run_skip_permissions: true,
            run_password: true,
            tui_skip_permissions: true,
        }
    }
}

impl CliFlags {
    /// Read the flags off `run --help` and `--help`. Pure, so the usage a real old CLI printed is
    /// a test fixture.
    pub fn from_help(skip_permissions: &str, run_help: &str, tui_help: &str) -> Self {
        let has = |help: &str, flag: &str| {
            help.split(|c: char| c.is_whitespace() || c == ',')
                .any(|word| word == flag)
        };
        Self {
            run_skip_permissions: has(run_help, skip_permissions),
            run_password: has(run_help, "--password"),
            tui_skip_permissions: has(tui_help, skip_permissions),
        }
    }
}

/// One flavour's probed answer: which binary, at which modification time, said what.
type ProbedFlags = (
    cide_ipc::Harness,
    std::path::PathBuf,
    Option<std::time::SystemTime>,
    CliFlags,
);

/// What each flavour's binary answered, and which binary that was. See [`CliFlags`].
static CLI_FLAGS: std::sync::Mutex<Vec<ProbedFlags>> = std::sync::Mutex::new(Vec::new());

impl Flavor {
    /// Ask the installed binary which flags it has, once per binary version. **Forks** — call it
    /// from a thread allowed to block, before building a spec; [`Self::cli_flags`] then answers
    /// from the cache without forking, which keeps `spawn_spec` pure.
    pub fn probe_cli_flags(&self) -> CliFlags {
        let Ok(binary) = self.binary() else {
            return self.cli_flags();
        };
        let modified = std::fs::metadata(&binary).and_then(|m| m.modified()).ok();
        {
            let cache = CLI_FLAGS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((_, _, _, flags)) = cache.iter().find(|(kind, path, at, _)| {
                *kind == self.kind && *path == binary && *at == modified
            }) {
                return *flags;
            }
        }
        let help = |args: &[&str]| -> Option<String> {
            let mut command = std::process::Command::new(&binary);
            command.args(args).env("NO_COLOR", "1");
            let out = cide_core::child_env::run_filter_with(
                command,
                None,
                std::time::Duration::from_secs(20),
                &[],
            )
            .ok()?;
            // yargs prints usage to stdout for `--help`; some builds use stderr. Either will do.
            Some(format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                out.stderr
            ))
        };
        let flags = match (help(&["run", "--help"]), help(&["--help"])) {
            (Some(run), Some(tui)) if run.contains("--format") => {
                CliFlags::from_help(self.skip_permissions, &run, &tui)
            }
            // A probe that could not read a usage says nothing; see `CliFlags`' last paragraph.
            _ => CliFlags::default(),
        };
        let mut cache = CLI_FLAGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|(kind, _, _, _)| *kind != self.kind);
        cache.push((self.kind, binary, modified, flags));
        flags
    }

    /// The flags the last probe found for this flavour, or the current CLI's when none has run.
    pub fn cli_flags(&self) -> CliFlags {
        CLI_FLAGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(kind, _, _, _)| *kind == self.kind)
            .map_or_else(CliFlags::default, |(_, _, _, flags)| *flags)
    }
}

/// How to start this CLI's **interactive TUI** in a tab cide opens on one piece of work. (M104)
///
/// # Not a run, and not [`Flavor::continue_spec`]
///
/// A run is `opencode run --format json`, one process per turn, rendered as a log — the shape the
/// user asked `cide_session_open` specifically *not* to have ("a write session, not just a read
/// only log"). `continue_spec` is the TUI, but re-opened on a conversation with no configuration
/// at all, so a pane it starts has no task tools. This is the third shape: the TUI, fresh, with
/// the opening line in `--prompt` and a configuration document that carries **cide's MCP server
/// and the machine's providers, and no role** — a session is not a role, and an inline agent here
/// would be a system prompt nobody wrote.
///
/// The bridge the `mcp` entry names reads `CIDE_SESSION` and `CIDE_AGENT_SOCK` from the
/// environment it inherits from this TUI, which is how the tab's own connection is scoped by
/// `agent_rpc` without anything here knowing the id — `cmd::session` sets both on the child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Flavor {
    /// The TUI on one piece of work. `Err` is a sentence for the caller: the CLI is not installed.
    ///
    /// `unattended` passes the CLI's own skip-permissions flag (`--auto` for opencode), for the
    /// reason `claude_tab::TabMode::unattended` gives: a tab cide opens on its own is a tab nobody
    /// may be watching when the first permission prompt arrives.
    pub fn tab_launch(
        &self,
        prompt: &str,
        model: Option<&str>,
        unattended: bool,
        llm: &cide_ipc::LlmSettings,
        hook_bin: Option<&std::path::Path>,
    ) -> Result<TabLaunch, String> {
        let binary = self.binary()?;
        let mut args: Vec<String> = Vec::new();
        if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
            args.push("--model".into());
            args.push(model.to_string());
        }
        if unattended && self.probe_cli_flags().tui_skip_permissions {
            args.push(self.skip_permissions.to_string());
        }
        // Last and as one token: a value, so nothing after it can be mistaken for its tail.
        if !prompt.trim().is_empty() {
            args.push("--prompt".into());
            args.push(prompt.to_string());
        }
        let mut env = Vec::new();
        if let Some(document) = self.tab_config(llm, hook_bin) {
            env.push((self.config_env(), document));
        }
        Ok(TabLaunch {
            program: binary.to_string_lossy().into_owned(),
            args,
            env,
        })
    }

    /// The tab's configuration document: providers and cide's MCP server. `None` when there is
    /// nothing to say, so the variable is not set at all and the user's own configuration is all
    /// the CLI reads.
    fn tab_config(
        &self,
        llm: &cide_ipc::LlmSettings,
        hook_bin: Option<&std::path::Path>,
    ) -> Option<String> {
        let mut config = serde_json::Map::new();
        config.insert("$schema".into(), json!(self.schema));
        config.extend(provider_members(llm));
        if let Some(hook) = hook_bin {
            let mut servers = serde_json::Map::new();
            servers.insert(
                SERVER.into(),
                json!({
                    "type": "local",
                    "command": [hook.to_string_lossy(), "mcp"],
                    "enabled": true,
                }),
            );
            config.insert("mcp".into(), Value::Object(servers));
        }
        if config.len() == 1 {
            return None;
        }
        serde_json::to_string(&Value::Object(config)).ok()
    }
}

/// The opencode CLI as a harness. A unit struct: it holds nothing, and must not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpencodeHarness;

/// MiMo Code as a harness — [`OpencodeHarness`] over [`MIMO_CLI`]. (M81)
///
/// Every method is the opencode one, reached through the same shared functions with the other
/// [`Flavor`]; the argument for one implementation and not two is on that struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MimoHarness;

impl Harness for MimoHarness {
    fn kind(&self) -> cide_ipc::Harness {
        cide_ipc::Harness::Mimo
    }

    /// `<server>_<tool>`, opencode's spelling — measured, see [`Flavor`].
    fn tool_name(&self, tool: &str) -> String {
        OpencodeHarness.tool_name(tool)
    }

    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError> {
        child(&MIMO_CLI, plan, None)
    }

    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        child(&MIMO_CLI, plan, Some(session))
    }

    fn deliver(&self, text: &str) -> Delivery {
        OpencodeHarness.deliver(text)
    }

    fn continue_spec(&self, conversation: &HarnessSession) -> Result<ContinueSpec, HarnessError> {
        MIMO_CLI.continue_spec(conversation)
    }

    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState> {
        OpencodeHarness.observe(current, ob)
    }

    fn diagnose(&self, line: &str) -> Option<FailoverReason> {
        failover(line)
    }

    fn failure_exits_zero(&self) -> bool {
        MIMO_CLI.failure_exits_zero
    }

    fn usage(&self, line: &str) -> Option<cide_ipc::TokenUsage> {
        usage(line)
    }

    fn models(
        &self,
        cwd: Option<&std::path::Path>,
        llm: &cide_ipc::LlmSettings,
    ) -> Result<Vec<String>, String> {
        MIMO_CLI.models(cwd, llm)
    }
}

impl Harness for OpencodeHarness {
    fn kind(&self) -> cide_ipc::Harness {
        cide_ipc::Harness::Opencode
    }

    /// `<server>_<tool>`, so cide's vocabulary arrives as `cide_cide_task_get`.
    ///
    /// The doubled `cide` looks like a mistake and is not: the server is [`SERVER`] and the tool
    /// is already `cide_task_get`, and opencode joins the two with one underscore. Renaming the
    /// server to flatten it would break the rule [`SERVER`] exists for — one server name across
    /// harnesses, so a role's tool list need not know which CLI is carrying it.
    fn tool_name(&self, tool: &str) -> String {
        format!("{SERVER}_{tool}")
    }

    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError> {
        child(&OPENCODE_CLI, plan, None)
    }

    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        child(&OPENCODE_CLI, plan, Some(session))
    }

    /// Always [`Delivery::Respawn`]. See the module header: `run` is one turn per process, and
    /// the child that answered the last one has already exited.
    fn deliver(&self, _text: &str) -> Delivery {
        Delivery::Respawn
    }

    /// The opencode **TUI** on the conversation: `opencode --session <ses_…>`, in the run's
    /// directory. (M42)
    ///
    /// Not `run`. A run is `opencode run --format json`, one turn per process, rendered by
    /// [`render_event`] into the digest a pane shows while a turn is in flight — and that digest
    /// is exactly what the person who opened a finished run did *not* want to read. The TUI is
    /// the harness as its author ships it: the whole conversation, opencode's own rendering, a
    /// prompt to continue it. Measured on 1.18.27: `opencode [project]` takes `-s, --session
    /// <id>` ("session id to continue"), and the project directory is the cwd `session_spawn`
    /// starts it in.
    ///
    /// No `--agent`, no `OPENCODE_CONFIG_CONTENT`. The role's inline agent definition needs a
    /// [`RunPlan`] to build ([`config_json`]), and the pane has none; naming the agent without
    /// the document would make the CLI warn once and fall back to its default anyway. So the
    /// person continues under opencode's own configuration — recorded as a limitation in the
    /// journal rather than papered over with a config the run did not carry.
    fn continue_spec(&self, conversation: &HarnessSession) -> Result<ContinueSpec, HarnessError> {
        OPENCODE_CLI.continue_spec(conversation)
    }

    /// The whole state machine for this harness, because the output stream is the whole channel.
    ///
    /// | observation | answer | why |
    /// | --- | --- | --- |
    /// | `Exit(code)` | `Finished { code }` | the only ground truth about the child, so it answers from any state |
    /// | `Line` — `step_start`, `step_finish`, `text`, `tool_use`, `reasoning` | `Running` | the turn is producing something |
    /// | `Line` — anything else, or not JSON at all | `None` | says nothing about the run |
    /// | `Hook` | `None` | nothing installs a hook into opencode |
    ///
    /// # No line ends the turn — the exit does, and run 06202dd6 is why
    ///
    /// This mapping used to read `step_finish` in two halves: `reason: "tool-calls"` was an
    /// intermediate step (`Running` — a turn that calls tools is several model round trips, and
    /// the intermediate ones finish with that value, the AI SDK's own), any other reason was the
    /// turn handed back (`Idle`). The half-truth in it: a turn's *final* step really does finish
    /// with a non-`tool-calls` reason. What it missed is that a final-looking reason is not
    /// proof the *turn* is over — the stream carries `step_finish` events from **nested
    /// sessions** too (a role that reaches for opencode's own sub-agents sees their final steps
    /// on the same stdout, under their own `sessionID`s), and the SDK has finish reasons
    /// (`length`, `error`, `unknown`) that end a step with the child nowhere near done.
    ///
    /// Believing one was not a display bug. `Idle` releases the run's concurrency slot, and
    /// under worktree isolation the next queued run of the same role is then admitted — whose
    /// `bring_up` **winds down the idle child to reclaim the checkout**. Run 06202dd6 (a `qa`
    /// role, sixteen minutes into a live smoke test) died exactly this way: a queued dispatch
    /// for the same role had been waiting thirteen minutes, a mid-turn `step_finish` read as
    /// `Idle`, and the wind-down killed a working child — logged by the orchestrator as
    /// "dispatch-to-busy-role appears to preempt the active run", which is precisely what the
    /// queue exists never to do. (The observed exit was 143: the CLI traps the wind-down's
    /// signal, shuts its own process tree down — the cleanup was noted as flawless — and dies
    /// as if TERMed. Good manners, wrong death.)
    ///
    /// So no line answers `Idle` any more, and the asymmetry that already stood in this comment
    /// is the whole justification: being *late* to call the turn over costs almost nothing,
    /// because `run` is one turn per process and the child exits within milliseconds of its
    /// last step — `Observation::Exit` follows with the truth, releases the slot then, and the
    /// queued run starts into a checkout whose previous occupant is genuinely gone. Being
    /// *early* costs a working agent its life. An opencode run therefore never holds
    /// [`RunState::Idle`]: its turn ends as `Finished { code }`, which nudges the orchestrator
    /// like every other ending, and the idle wind-down is in practice a claude-only event —
    /// a parked interactive `claude` being the one idle child that never leaves by itself.
    ///
    /// # Why every event answers `Running` rather than `None`
    ///
    /// Because for this harness they are the only evidence that the child is doing anything at
    /// all. A run stays [`RunState::Starting`] from the fork until something says otherwise, and
    /// "otherwise" is a line. Answering `None` to `text` would mean a run whose first `step_start`
    /// was lost — a truncated line, a stream cide attached to a moment late — never leaves
    /// `Starting` again. Answering `Running` from any event costs nothing when the run is already
    /// running, because [`Harness::observe`]'s contract is that a transition to the state a run
    /// already holds is `None`.
    ///
    /// `error` events deliberately do **not** map to [`RunState::Failed`]. The process exits
    /// immediately after one, so `Observation::Exit` would overwrite the sentence within
    /// milliseconds with `Finished { code }` — the truthful record, since there *is* an exit
    /// status — and the error's own text is in the run's transcript, where opening the run into a
    /// pane shows it.
    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState> {
        match ob {
            // The only observation carrying ground truth about the child, so it answers from any
            // state — including `Paused`, whose frozen child may have been killed under the
            // freeze, and `Idle`, which is alive by definition and which nothing else ends.
            Observation::Exit(code) => {
                let next = RunState::Finished { code };
                (next != current).then_some(next)
            }
            // No hook cide can install reaches this CLI. Answered explicitly rather than by a
            // catch-all so that a frame routed here by mistake is a silent no-op rather than a
            // claude state machine applied to an opencode run.
            Observation::Hook(_) => None,
            Observation::Line(line) => {
                if absorbs(&current) {
                    return None;
                }
                let event = Event::parse(line)?;
                let next = match event.kind.as_str() {
                    "step_start" | "step_finish" | "text" | "tool_use" | "reasoning" => {
                        RunState::Running
                    }
                    _ => return None,
                };
                (next != current).then_some(next)
            }
        }
    }

    fn diagnose(&self, line: &str) -> Option<FailoverReason> {
        failover(line)
    }

    fn usage(&self, line: &str) -> Option<cide_ipc::TokenUsage> {
        usage(line)
    }

    /// `opencode models`, run for real — see [`Flavor::models`].
    fn models(
        &self,
        cwd: Option<&std::path::Path>,
        llm: &cide_ipc::LlmSettings,
    ) -> Result<Vec<String>, String> {
        OPENCODE_CLI.models(cwd, llm)
    }
}

impl Flavor {
    pub fn continue_spec(
        &self,
        conversation: &HarnessSession,
    ) -> Result<ContinueSpec, HarnessError> {
        if conversation.harness != self.kind {
            return Err(HarnessError::WrongHarness {
                plan: conversation.harness,
                harness: self.kind,
            });
        }
        let id = conversation.id.trim();
        if id.is_empty() {
            return Err(HarnessError::NotAConversation {
                harness: self.kind,
                id: conversation.id.clone(),
            });
        }
        Ok(ContinueSpec {
            program: self.program().to_string(),
            args: self
                .continue_args
                .iter()
                .map(|arg| (*arg).to_string())
                .chain(["--session".to_string(), id.to_string()])
                .collect(),
            resume: None,
        })
    }

    /// `opencode models`, run for real.
    ///
    /// A probe and not a table, because the answer is a property of *this machine*: the list is
    /// whatever providers the user has configured and authenticated, and on the box this was
    /// written on it holds `lmstudio/…` and `unsloth/…` entries no compiled-in list could have
    /// guessed. A menu of ids cide invented would be wrong in both directions at once — offering
    /// models the user cannot reach, and omitting every one they can.
    ///
    /// The spelling matters and is the reason this exists: opencode wants `provider/model`, and
    /// the form's box was placeholdered `sonnet` for Claude. Nobody types
    /// `unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M` from memory, so before this the field was
    /// unfillable in practice and every opencode role ran the provider default.
    pub fn models(
        &self,
        cwd: Option<&std::path::Path>,
        llm: &cide_ipc::LlmSettings,
    ) -> Result<Vec<String>, String> {
        let binary = self.binary()?;

        let mut command = std::process::Command::new(&binary);
        command.arg("models");
        // Colour would put escape sequences inside the ids themselves, which would then be
        // written into a role file and handed to `--model`. `cide_spec::cli::run` sets it for
        // the same reason one layer over.
        command.env("NO_COLOR", "1");
        // The same document a run gets, so the menu describes the runs that can happen. Without
        // it the probe sees only what the user configured by hand, and cide's own providers are
        // invisible in the very dialog where they are chosen.
        if let Some(document) = provider_config_content(self, llm) {
            command.env(self.config_env(), document);
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        // `run_filter_with` and not `Command::output`: it is the workspace's spawn chokepoint,
        // and `prepare_command` and `arm` are applied *inside* it. `blame`'s call site had one
        // and not the other for two milestones, which is the miss a chokepoint makes
        // unrepresentable. The extra `PATH` entry is the binary's own directory, for
        // `prepare_command_with`'s shebang reason — opencode ships as a compiled binary today,
        // but a version-manager shim in front of it would not.
        let bin_dir: Vec<std::path::PathBuf> = binary
            .parent()
            .map(|d| vec![d.to_path_buf()])
            .unwrap_or_default();
        let filtered =
            cide_core::child_env::run_filter_with(command, None, MODELS_DEADLINE, &bin_dir)
                .map_err(|error| describe(self, &error))?;

        if !filtered.ok {
            // The first non-empty line of stderr, which is where a CLI puts the sentence worth
            // showing. The rest is a stack trace nobody reading a settings dialog can act on.
            let detail = filtered
                .stderr
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty());
            return Err(match detail {
                Some(line) => format!("`{} models` failed: {line}", self.program()),
                None => format!("`{} models` failed and said nothing", self.program()),
            });
        }

        Ok(model_ids(self, &String::from_utf8_lossy(&filtered.stdout)))
    }
}

/// The `provider` member cide contributes to an opencode configuration. (M45)
///
/// # One producer, two callers, and that is the entire point of this function existing
///
/// [`config_json`] puts it in the **run's** document; [`OpencodeHarness::models`] puts it in the
/// **probe's**. Two documents built independently would drift, and the drift's shape is nasty and
/// specific: a Model menu listing ids no run can use, or a run reaching a provider the menu never
/// offered. `cide_git::push`'s preview states the rule — the preview must describe the push that
/// happens — and `a_probe_and_a_run_carry_the_same_provider_block` keeps it true here.
///
/// # No `plugin` key, ever
///
/// opencode installs a listed plugin from npm: `~/.config/opencode/package.json` is
/// opencode-managed and carries `@opencode-ai/plugin` with a populated `node_modules` beside it.
/// A `plugin` array cide wrote would therefore turn pressing Dispatch into a package fetch —
/// stalling a run's first turn, and failing outright on a machine with no network. cide does not
/// install third-party packages as a side effect of starting a subagent. A plugin-backed provider
/// is [`cide_ipc::LlmProvider::External`]: cide explains it and checks whether it worked, the way
/// `cide_spec::discover` treats the `openspec` CLI, and writes nothing.
///
/// # Blank is omitted, never written
///
/// The document is deep-merged **over** the user's own `opencode.json`, key by key, with cide's
/// last — measured. So a key cide writes wins outright: an `options.apiKey` of `""` for a provider
/// the user also declares would blank their working key, silently, in a file cide never touched.
/// Every field below is emitted only when it says something, which is also why a `Catalog`
/// provider with no key emits **no block at all** rather than an empty one.
///
/// # Only the keys the schema declares
///
/// `$defs.ProviderConfig` is `additionalProperties: false` over `api, name, env, id, npm,
/// whitelist, blacklist, options, models`. A key outside it does not degrade — it can invalidate
/// the document, and the module header records what a rejected document costs: the run silently
/// becomes opencode's default agent. Hence a hand-transcribed map rather than a
/// `serde_json::to_value` of cide's own type, whose field names would follow a Rust rename
/// straight into an invalid document.
fn provider_members(llm: &cide_ipc::LlmSettings) -> serde_json::Map<String, Value> {
    let mut providers = serde_json::Map::new();

    for provider in &llm.providers {
        if !provider.enabled() || provider.id().is_empty() {
            continue;
        }
        // Exhaustive on purpose: a fourth kind must be a compile error here rather than a
        // provider that silently emits nothing. See `cide_ipc::LlmProvider`'s header.
        let described = match provider {
            // The catalog supplies npm, baseURL and every model; cide supplies the credential —
            // and, when it has none, nothing at all. A blank key is the *ordinary* state for a
            // user whose shell already exports `OPENROUTER_API_KEY`, since a child inherits this
            // process's environment wholesale.
            cide_ipc::LlmProvider::Catalog { api_key, .. } if api_key.is_empty() => continue,
            cide_ipc::LlmProvider::Catalog { api_key, .. } => {
                json!({ "options": { "apiKey": api_key } })
            }

            cide_ipc::LlmProvider::Custom {
                label,
                npm,
                base_url,
                api_key,
                models,
                ..
            } => {
                let mut entry = serde_json::Map::new();
                if !label.is_empty() {
                    entry.insert("name".into(), json!(label));
                }
                if !npm.is_empty() {
                    entry.insert("npm".into(), json!(npm));
                }
                let mut options = serde_json::Map::new();
                // opencode's own capitalisation, applied here and nowhere else. cide's wire spells
                // the field `baseUrl`; the schema declares `baseURL` under
                // `additionalProperties: false`, so the two must not be confused.
                if !base_url.is_empty() {
                    options.insert("baseURL".into(), json!(base_url));
                }
                if !api_key.is_empty() {
                    options.insert("apiKey".into(), json!(api_key));
                }
                if !options.is_empty() {
                    entry.insert("options".into(), Value::Object(options));
                }
                let mut declared = serde_json::Map::new();
                for model in models {
                    if model.id.is_empty() {
                        continue;
                    }
                    let mut described = serde_json::Map::new();
                    if !model.label.is_empty() {
                        described.insert("name".into(), json!(model.label));
                    }
                    // `limit` is **all or nothing**, and that is measured rather than assumed:
                    // opencode 1.18.29 refuses a document carrying `limit.context` without
                    // `limit.output` — "Configuration is invalid at OPENCODE_CONFIG_CONTENT ↳
                    // Missing key provider.ollama.models.qwen3:8b.limit.output" — and the module
                    // header records what a refused document costs, which is the run silently
                    // becoming opencode's *default agent*. So a half-filled pair writes no
                    // `limit` at all rather than an invalid one.
                    //
                    // Omitting the block entirely is fine: the same probe with no `limit` lists
                    // the model. A `0` on either side means "the user gave no number", and
                    // guessing the other half would be cide inventing a context window that
                    // decides when opencode compacts.
                    if model.context > 0 && model.output > 0 {
                        described.insert(
                            "limit".into(),
                            json!({ "context": model.context, "output": model.output }),
                        );
                    }
                    declared.insert(model.id.clone(), Value::Object(described));
                }
                if !declared.is_empty() {
                    entry.insert("models".into(), Value::Object(declared));
                }
                Value::Object(entry)
            }

            // Nothing: not a block, not a plugin line, not a credential. See the header — and note
            // this arm is why `LlmProvider` is an enum, so the promise is a `match` the compiler
            // checks rather than a habit somebody has to remember.
            cide_ipc::LlmProvider::External { .. } => continue,
        };
        providers.insert(provider.id().to_string(), described);
    }

    let mut out = serde_json::Map::new();
    if !providers.is_empty() {
        out.insert("provider".into(), Value::Object(providers));
    }
    out
}

/// Those members as a whole document, for a caller with no [`RunPlan`] — the model probe.
///
/// `None` when cide has nothing to say, which is the ordinary state of an installation that has
/// never opened the Models screen: the probe then runs exactly as it did before this existed.
fn provider_config_content(flavor: &Flavor, llm: &cide_ipc::LlmSettings) -> Option<String> {
    let members = provider_members(llm);
    if members.is_empty() {
        return None;
    }
    let mut doc = serde_json::Map::new();
    doc.insert("$schema".into(), json!(flavor.schema));
    doc.extend(members);
    serde_json::to_string(&Value::Object(doc)).ok()
}

/// [`provider_config_content`] plus [`pool_retry`], for [`test_model`]. The provider members are
/// the same producer's, so the test still describes the document a run gets.
fn test_config_content(flavor: &Flavor, llm: &cide_ipc::LlmSettings) -> Option<String> {
    let Some(retry) = pool_retry(flavor) else {
        return provider_config_content(flavor, llm);
    };
    let mut doc = serde_json::Map::new();
    doc.insert("$schema".into(), json!(flavor.schema));
    doc.extend(provider_members(llm));
    doc.insert("retry".into(), retry);
    serde_json::to_string(&Value::Object(doc)).ok()
}

/// Whether this line is opencode reporting a **provider** failure, and which kind. (M45)
///
/// # Structural, not textual, and that is the finding this function exists to record
///
/// The first design read the provider's prose out of `data.message`. It was wrong, and the
/// measurement that corrected it is worth keeping. opencode's `APIError` carries `statusCode` —
/// 429, 401, 403 — and `isRetryable`, **its own verdict**, on every instance (`@opencode-ai/sdk`'s
/// `ApiError`, and probed against 1.18.29). So the status code answers the rate-limit and auth
/// questions outright, and `isRetryable` answers the unreachable one for the case that carries no
/// status code at all: a dead local endpoint, probed as
/// `{"name":"APIError","data":{"message":"Cannot connect to API: …","isRetryable":true,
/// "metadata":{"url":"http://127.0.0.1:1234/v1/chat/completions"}}}`.
///
/// # The negative is the specification, exactly as it is for `cide_core::jsonlog`
///
/// A bogus model id — a **typo**, which no other candidate will fix — was probed as
/// `{"name":"UnknownError","data":{"message":"Unexpected server error. Check server logs for
/// details.","ref":"err_c19594b9"}}`, with no `isRetryable` and no status code. It must answer
/// `None`. A classifier that failed over on it would burn every candidate in the pool, one turn
/// each, and report the last one's failure as the run's cause of death. `UnknownError` is
/// therefore refused **by name**, and so is every 4xx that is not one of the three below: a 404 or
/// a 422 is a statement about the *request*, not about the endpoint's availability.
///
/// # The two errors that are about the turn and not the provider
///
/// `MessageOutputLengthError` is a context overflow and `MessageAbortedError` is somebody stopping
/// the turn — cide's own wind-down among them. Neither is a candidate's fault and neither is fixed
/// by trying another one; both are refused by name, explicitly rather than by falling off the end,
/// so the refusal is a decision a reader can find.
///
/// # Parsed defensively, and never into a closed struct
///
/// The wire object carries fields the published SDK type does not declare — `metadata` was on the
/// probed `APIError` and is in no version of `ApiError` — so this reads through `serde_json::Value`
/// and **must never** grow `deny_unknown_fields`. A release that adds a field must widen nothing
/// here; one that adds an error *name* falls through to `None`, which is the safe direction.
pub fn failover(line: &str) -> Option<FailoverReason> {
    let line = line.trim();
    // Cheap first: this runs on the coalescer thread, the thread every byte of every session
    // flows through, so the overwhelmingly common line must cost a substring test and not a parse.
    if !line.starts_with('{') || !line.contains("\"type\":\"error\"") {
        return None;
    }
    let event: Value = serde_json::from_str(line).ok()?;
    // Only a top-level `error` event. A `tool_use` whose `state.status` is `error` is a *tool*
    // failing — a rejected permission, a command that returned non-zero — and `render_event`
    // already shows it in red. Failing a run's model over one would swap the model because a
    // `bash` call was refused.
    if event.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let error = event.get("error")?;
    let name = error
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let null = Value::Null;
    let data = error.get("data").unwrap_or(&null);

    match name {
        // opencode's own, and unambiguous: the provider rejected the credentials.
        "ProviderAuthError" => Some(FailoverReason::Auth),
        "APIError" => match data.get("statusCode").and_then(Value::as_u64) {
            Some(429) => Some(FailoverReason::RateLimited),
            // 402 is a guess and is here on purpose: a provider answering "payment required" is
            // out of quota in every sense a pool cares about. If no provider turns out to use it,
            // deleting this arm costs nothing.
            Some(402) => Some(FailoverReason::RateLimited),
            Some(401 | 403) => Some(FailoverReason::Auth),
            // Every other explicit 4xx is a statement about the *request* — a model id that does
            // not exist, a body the endpoint would not take — and no other candidate repairs it.
            // Refused ahead of `isRetryable`, deliberately: the status code is the more specific
            // fact and must win.
            Some(400..=499) => None,
            // A 5xx, or no code at all. `isRetryable` is opencode's own verdict and is the primary
            // signal here — the probed connect-refused case carries it with no code.
            _ => data
                .get("isRetryable")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                .then_some(FailoverReason::Unreachable),
        },
        // Named rather than defaulted — see the header. A typo, a context overflow and an abort
        // are each refused because they are each unfixable by another candidate.
        "UnknownError" | "MessageOutputLengthError" | "MessageAbortedError" => None,
        // A name this build has never met. `None` is the safe direction: the run ends as it always
        // did, and its error is in the pane and in `run-logs/<run>.log`.
        _ => None,
    }
}

/// What a `step_finish` line says was spent on the step it ends. (M80)
///
/// The part carries `tokens: {total, input, output, reasoning, cache: {read, write}}`, and the
/// shape is worth reading carefully because its own `total` is the proof: `5725 = 5696 + 4 + 25`
/// on the measured line below — input plus output plus reasoning, with **cache reads counted
/// nowhere in it**. So opencode's `input` already excludes what came from the cache, which is
/// the normalisation [`cide_ipc::TokenUsage`] asks for and means nothing has to be subtracted
/// here. codex is the harness that needs the arithmetic.
///
/// `total` itself is deliberately not carried: it is one CLI's definition of a sum, and a field
/// holding somebody else's arithmetic is a field that disagrees with [`cide_ipc::TokenUsage::context`]
/// the day either changes.
///
/// A line with a `tokens` object full of zeroes still answers `Some`: a step that spent nothing
/// measurable is a different claim from a run that has never reported, and the card draws them
/// differently.
pub fn usage(line: &str) -> Option<cide_ipc::TokenUsage> {
    let line = line.trim();
    // Cheap first — `failover`'s rule, and its reason: this runs on the coalescer thread, the
    // thread every byte of every session flows through.
    if !line.starts_with('{') || !line.contains("\"type\":\"step_finish\"") {
        return None;
    }
    let event: Value = serde_json::from_str(line).ok()?;
    if event.get("type").and_then(Value::as_str) != Some("step_finish") {
        return None;
    }
    let tokens = event.get("part")?.get("tokens")?;
    let at = |pointer: &str| tokens.pointer(pointer).and_then(Value::as_u64).unwrap_or(0);
    Some(cide_ipc::TokenUsage {
        input: at("/input"),
        output: at("/output"),
        reasoning: at("/reasoning"),
        cache_read: at("/cache/read"),
        cache_write: at("/cache/write"),
    })
}

/// How long a [`test_model`] turn is given before it is killed.
///
/// Generously longer than [`MODELS_DEADLINE`], because this one waits on a *model*: a cold local
/// llama-server loading weights, or a hosted provider queueing behind a rate limit, is slow in a
/// way listing ids never is. Short enough that a wedged endpoint does not hold the dialog open
/// for ever.
const TEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(90);

/// The smallest prompt that still proves the round trip happened.
///
/// A real turn and not a handshake, because that is what the button claims. Listing a model id
/// proves only that opencode *resolved* it — a wrong key, an endpoint that accepts connections
/// and refuses completions, a model the provider has retired, and a plugin whose OAuth expired
/// all list perfectly and fail on the first token.
const TEST_PROMPT: &str = "Reply with the single word: ok";

/// Whether this provider/model actually answers, run for real. (M45)
///
/// Costs one very small turn of the user's own quota, which is the honest price of the claim the
/// button makes; the caller says so before spending it. `Ok(detail)` is a working model with one
/// line worth showing, `Err(sentence)` is a whole sentence for the person looking at the dialog.
///
/// The provider document is the **same** one [`config_json`] gives a run, through the same
/// [`provider_members`] — so a model that passes here is a model a run can use, and a failure here
/// is a failure a run would have had. Testing against an independently built document would be a
/// button that reports on a configuration nothing else uses.
pub fn test_model(
    flavor: &Flavor,
    cwd: Option<&std::path::Path>,
    llm: &cide_ipc::LlmSettings,
    model: &str,
) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Name a model first — there is nothing to test.".to_string());
    }

    let binary = flavor.binary()?;

    let mut command = std::process::Command::new(&binary);
    command.arg("run");
    command.arg("--model");
    command.arg(model);
    command.arg("--format");
    command.arg("json");
    // Last, and after every flag, for `child`'s stated reason: the message is positional and a
    // parser reads a leading `-` as a flag.
    command.arg(TEST_PROMPT);
    command.env("NO_COLOR", "1");
    // Bounded retries on a flavour that would otherwise retry a dead endpoint for fifteen minutes
    // in silence: a test asks whether the model answers *now*. See `pool_retry`.
    if let Some(document) = test_config_content(flavor, llm) {
        command.env(flavor.config_env(), document);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let bin_dir: Vec<std::path::PathBuf> = binary
        .parent()
        .map(|d| vec![d.to_path_buf()])
        .unwrap_or_default();
    let filtered = cide_core::child_env::run_filter_with(command, None, TEST_DEADLINE, &bin_dir)
        .map_err(|error| describe_test(flavor, &error))?;

    let stdout = String::from_utf8_lossy(&filtered.stdout);
    verdict(flavor, &stdout, filtered.ok, &filtered.stderr)
}

/// Read a finished `opencode run --format json` stream as a pass or a sentence.
///
/// Split out from [`test_model`] so the interesting half is a pure function a test can drive over
/// captured output — `plan_respawn`'s "the half that happens under the lock is the half a test can
/// drive", applied to a child instead of a lock.
///
/// **An `error` event outranks the exit status.** Measured on 1.18.29: a provider failure prints
/// one `{"type":"error",…}` line and exits 1, but a turn that opencode retried internally can also
/// carry an error line and still finish — so the *last* error seen is only fatal when nothing
/// afterwards proved the model answered. Reading the exit code alone would call the first case a
/// success on any CLI that exits 0 after recovering, and reading the first error alone would call
/// the second a failure.
fn verdict(flavor: &Flavor, stdout: &str, ok: bool, stderr: &str) -> Result<String, String> {
    let mut answered = false;
    let mut error: Option<String> = None;
    for line in stdout.lines() {
        let Some(event) = Event::parse(line) else {
            continue;
        };
        match event.kind.as_str() {
            // The model produced something. `step_finish` alone is not enough: a turn that died
            // mid-step still finishes its step.
            "text" | "tool_use" | "reasoning" => answered = true,
            "error" => {
                error = Some(error_sentence(flavor, line));
                answered = false;
            }
            _ => {}
        }
    }

    if answered {
        return Ok("The model answered.".to_string());
    }
    if let Some(error) = error {
        return Err(error);
    }
    if ok {
        // Exit 0 and not one usable event. Rare, and worth its own sentence rather than a
        // cheerful one: something ran and said nothing, which is not a working model.
        return Err(format!(
            "{} exited cleanly but the model produced nothing.",
            flavor.program()
        ));
    }
    let detail = stderr.lines().map(str::trim).find(|line| !line.is_empty());
    Err(match detail {
        Some(line) => format!("{} failed: {line}", flavor.program()),
        None => format!("{} failed and said nothing.", flavor.program()),
    })
}

/// The sentence inside one `{"type":"error",…}` line.
///
/// Parsed defensively through `Value` and never into a closed struct: the live object carries
/// fields the published SDK type does not declare (`metadata` was on a probed `APIError`), and a
/// release that adds one must not turn this into "an unreadable error".
fn error_sentence(flavor: &Flavor, line: &str) -> String {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return format!(
            "{} reported an error it did not describe.",
            flavor.program()
        );
    };
    let error = event.get("error").unwrap_or(&Value::Null);
    let name = error.get("name").and_then(Value::as_str).unwrap_or("error");
    let message = error
        .get("data")
        .and_then(|data| data.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match message.is_empty() {
        true => format!("{name}."),
        false => format!("{name}: {message}"),
    }
}

/// One sentence for a [`cide_core::child_env::FilterError`] from [`test_model`].
///
/// Its own wording rather than [`describe`]'s, because that one names `opencode models` in every
/// arm and this is a different command with a different remedy — a timeout here means the *model*
/// did not answer, not that a provider is unreachable.
fn describe_test(flavor: &Flavor, error: &cide_core::child_env::FilterError) -> String {
    use cide_core::child_env::FilterError;
    let program = flavor.program();
    match error {
        FilterError::Spawn(io) => format!("`{program} run` could not be started: {io}"),
        FilterError::Timeout => format!(
            "The model did not answer within {}s and the attempt was stopped. A local endpoint \
             may still be loading its weights.",
            TEST_DEADLINE.as_secs()
        ),
        FilterError::Unreadable => format!("`{program} run` ran and its output could not be read"),
        FilterError::Wait(io) => format!("`{program} run` ran and could not be reaped: {io}"),
    }
}

/// One sentence for a [`cide_core::child_env::FilterError`].
///
/// The error is tagged and not prose deliberately — its own doc says every caller phrases its
/// own sentence — and this one is read in a settings dialog by somebody choosing a model, so it
/// names the command rather than the machinery.
fn describe(flavor: &Flavor, error: &cide_core::child_env::FilterError) -> String {
    use cide_core::child_env::FilterError;
    let program = flavor.program();
    match error {
        FilterError::Spawn(io) => format!("`{program} models` could not be started: {io}"),
        // One sentence: this literal used to run fourteen spaces of indentation into the middle
        // of it, a line continuation that had lost its backslash.
        FilterError::Timeout => format!(
            "`{program} models` did not answer in time and was stopped. A configured provider \
             may be unreachable — the model can still be typed in below."
        ),
        FilterError::Unreadable => {
            format!("`{program} models` ran and its output could not be read")
        }
        FilterError::Wait(io) => format!("`{program} models` ran and could not be reaped: {io}"),
    }
}

/// How long `opencode models` is given before it is killed.
///
/// Measured at well under a second, and generous by an order of magnitude on purpose: the list
/// is read from configured providers, one of which may be a local server that is not running.
/// The cost of the ceiling being high is a spinner in a dialog — this never runs on the IPC
/// thread — and the cost of it being low is a correct list thrown away on a slow machine.
const MODELS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// What opencode itself resolves as the user's configuration for a project — the parts of it
/// a run's behaviour turns on. Asked of the binary, never parsed from its files.
///
/// # Why cide reads this at all
///
/// **A continued session keeps the model it last used.** Measured on 1.18.31, in a log: a run
/// forked with `--session <id>` and no `--model` streamed to `providerID=vllm` — the provider the
/// session had been on — forty minutes after the user's `opencode.jsonc` had switched `model` to
/// `deepseek/deepseek-flash`, while a *fresh* session forked in between went to deepseek. Only an
/// explicit `--model` moves a continued session. So a role that names no model, under no pool and
/// no override, was a role whose model cide could not change on any continuation: not on a
/// follow-up, not on a resume after a cide restart, and not on a restart for new settings — the
/// last of which exists for nothing else. The fix is that every opencode child gets `--model`:
/// the pool's candidate, the role's or the override's, and failing all three **opencode's own
/// default**, which is what a fresh child would have picked anyway. [`UserConfig::model`] is that
/// default; `overrides::Resolved::with_default_model` folds it in.
///
/// # Why the binary and not the files
///
/// The default is the end of a merge — `~/.config/opencode/opencode.json` or `.jsonc` (JSONC:
/// comments and trailing commas), then `opencode.json`/`opencode.jsonc`/`.opencode/opencode.json`
/// walked up from the cwd, then `OPENCODE_CONFIG`, then whatever the org's remote config adds —
/// and a copy of that merge in Rust would be right until the next release moved it. `opencode
/// debug config` prints the merge as the binary performed it, in half a second, with the same
/// environment a child gets. It is asked **without** `OPENCODE_CONFIG_CONTENT`: cide's own
/// document is merged at the fork and never names a model, so the user's configuration alone is
/// the honest reading of "what would opencode pick".
///
/// # What is kept
///
/// `model`, `small_model` and the `provider` block — the keys a person edits to point runs
/// somewhere else, and the ones `AgentRegistry::plan_restarts` compares at Resume so that such an
/// edit restarts a paused run. The rest of the document (keybinds, theme, `permission`) is
/// deliberately not here: a restart costs the in-flight turn, and a keybind is not worth one.
/// `provider` is kept as the JSON it was printed as, for equality only; nothing reads a field
/// of it.
#[derive(Clone, PartialEq, Eq)]
pub struct UserConfig {
    /// `model`, as `provider/model`. `None` when the configuration names none.
    pub model: Option<String>,
    /// `small_model`, the same shape.
    pub small_model: Option<String>,
    /// The `provider` object, serialised. Empty when there is none.
    ///
    /// **Carries the user's API keys** — `provider.<id>.options.apiKey` is where opencode keeps
    /// them — which is why this struct's `Debug` is written by hand below and never derived: a
    /// `LiveRun` is one `{:?}` away from a log line, and the log is the file users are asked to
    /// send back (`log_plugin`'s argument about `bollard`, in the Docker row of `CLAUDE.md`).
    pub provider: String,
}

impl std::fmt::Debug for UserConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserConfig")
            .field("model", &self.model)
            .field("small_model", &self.small_model)
            .field(
                "provider",
                &format_args!("<{} bytes, redacted>", self.provider.len()),
            )
            .finish()
    }
}

/// Ask the installed `opencode` for the configuration it resolves in `cwd`. See [`UserConfig`].
///
/// `cwd` is the project root: opencode walks up from the cwd to the git worktree root for the
/// project's file, and a cide worktree is its own root, so from inside one only the checkout's
/// committed copy would be seen — the project root sees the same file plus any uncommitted edit
/// to it, which is the reading a person editing that file expects.
pub fn user_config(flavor: &Flavor, cwd: &std::path::Path) -> Result<UserConfig, String> {
    let binary = flavor.binary()?;
    let mut command = std::process::Command::new(&binary);
    command.args(["debug", "config"]);
    command.env("NO_COLOR", "1");
    command.current_dir(cwd);
    // The chokepoint, for `models`' reason one screen up. The extra `PATH` entry is the
    // binary's own directory, for a version-manager shim's shebang.
    let bin_dir: Vec<std::path::PathBuf> = binary
        .parent()
        .map(|d| vec![d.to_path_buf()])
        .unwrap_or_default();
    let filtered = cide_core::child_env::run_filter_with(command, None, CONFIG_DEADLINE, &bin_dir)
        .map_err(|error| describe(flavor, &error))?;
    if !filtered.ok {
        let detail = filtered
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty());
        return Err(match detail {
            Some(line) => format!("`{} debug config` failed: {line}", flavor.program()),
            None => format!(
                "`{} debug config` failed and said nothing",
                flavor.program()
            ),
        });
    }
    parse_user_config(&String::from_utf8_lossy(&filtered.stdout))
}

/// The pure half of [`user_config`]: the printed document to the three kept fields.
///
/// A missing key is `None`/empty and never a failure — a configuration with no `model` is a
/// legitimate one that lets the provider's default stand. A value of the wrong type is ignored
/// the same way, because the alternative is refusing to fork a run over a key cide does not
/// even read. Only an unparseable document is an error, and that one names itself.
pub fn parse_user_config(printed: &str) -> Result<UserConfig, String> {
    let document: Value = serde_json::from_str(printed.trim()).map_err(|error| {
        format!("`opencode debug config` printed something that is not JSON: {error}")
    })?;
    let string = |key: &str| {
        document
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Ok(UserConfig {
        model: string("model"),
        small_model: string("small_model"),
        provider: document
            .get("provider")
            .filter(|provider| provider.is_object())
            .map(Value::to_string)
            .unwrap_or_default(),
    })
}

/// `opencode debug config` reads files and prints; half a second measured. Generous, because a
/// miss here degrades a fork to "let opencode pick" rather than failing it.
const CONFIG_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// The `provider/model` ids in `opencode models` output, in order, without repeats.
///
/// # Why this filters rather than trusting the lines
///
/// `opencode --help` prints a five-line ASCII banner above its output. `models` does not today,
/// and the whole point of a probe is that "today" is not a guarantee: a release that grew one
/// would put `█▀▀█ █▀▀█ …` in the dropdown as a *selectable model*, which is then written into a
/// role file and handed to `--model`, and the failure surfaces three processes down as a
/// provider error about a model nobody typed. The same filter absorbs a deprecation warning, a
/// blank separator line and a trailing newline.
///
/// A `/` is required because that is opencode's whole spelling — an id without one is not a
/// model, it is prose.
fn model_ids(flavor: &Flavor, stdout: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    stdout
        .lines()
        .map(str::trim)
        // mimo's annotation, cut at its exact separator and only for the flavour that prints
        // one — see `Flavor::annotated_models`.
        .map(|line| match flavor.annotated_models {
            true => line.split_once(" — ").map_or(line, |(id, _)| id.trim_end()),
            false => line,
        })
        .filter(|line| is_model_id(line))
        // Order is the CLI's, which groups by provider. Sorting would scatter a provider's
        // models through the menu and put whichever id happens to start with `a` on the row a
        // person picks without reading.
        .filter(|line| seen.insert(line.to_string()))
        .map(str::to_string)
        .collect()
}

/// `provider/model`: two non-empty segments, the first a plain identifier.
///
/// The second half is deliberately permissive — `unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M` and
/// `lmstudio/openai/gpt-oss-20b` are both real ids on the machine this was written on, so it
/// carries colons and further slashes. What it may not carry is whitespace, which is what rules
/// out every line of prose.
fn is_model_id(line: &str) -> bool {
    let Some((provider, rest)) = line.split_once('/') else {
        return false;
    };
    !provider.is_empty()
        && !rest.is_empty()
        && provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        && !rest.chars().any(char::is_whitespace)
}

/// One event line, in the two fields anything here reads.
///
/// Measured shape, from the shipped binary's own emitter:
/// `{"type":<name>,"timestamp":<ms>,"sessionID":<id>, ...}` — the id is spread onto **every**
/// line, including the first, which is the whole of [`capture`]. A `step_finish`'s finish
/// `reason` is deliberately **not** here any more: it used to route the event to
/// [`RunState::Idle`], which is the misreading that killed run 06202dd6 — see
/// [`OpencodeHarness::observe`]. [`render_event`] still shows the reason to a person, from the
/// raw JSON it reads for display.
#[derive(Debug, Deserialize)]
struct Event {
    /// `step_start`, `step_finish`, `text`, `tool_use`, `reasoning`, `error`.
    #[serde(rename = "type")]
    kind: String,
    /// opencode's own session id, `ses_…`. Not cide's [`cide_ipc::SessionId`].
    #[serde(rename = "sessionID")]
    session: Option<String>,
}

impl Event {
    /// One line, or `None` when it is not one of ours.
    ///
    /// A run's stdout is **not** all JSON: the CLI prints a plain warning line when it rejects a
    /// permission, and another if an agent name does not resolve — with ANSI styling, because it
    /// is talking to a terminal. Those must fall through rather than raising anything.
    ///
    /// `trim` because this is a PTY: the line discipline turns every `\n` into `\r\n`, so a line
    /// split on `\n` still ends in a carriage return.
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        serde_json::from_str(line).ok()
    }
}

/// The session id this line names, for [`SessionBinding::Harness`].
///
/// Answers from the **first** line a run prints, and from every line after it, so a run is
/// resumable from its first event onward rather than only once it has said something.
///
/// Both refusals are ordinary rather than exceptional: a line that is not JSON is a warning the
/// CLI printed, and a JSON line with no `sessionID` would be a shape this release does not have —
/// neither is worth an error, and either would be worth one if this returned a `Result`.
fn capture(line: &str) -> Option<String> {
    Event::parse(line)?.session.filter(|id| !id.is_empty())
}

// ==========================================================================================
// Rendering the event stream for a person.
// ==========================================================================================

/// Whether a person may later ask for this line's whole event — [`SessionBinding::Harness`]'s
/// `keep`. A tool call (its input and its whole output), the model's own words, a block of the
/// model's thinking, and an error are worth a ring slot; a step marker is not.
///
/// Reasoning joined the list in M62 and the reason is the inverse of why it was excluded: the
/// rendering used to *show* a clipped 240 characters of it, so the ring was a second copy of
/// something already on screen. Now the row shows a duration and nothing else, which makes this
/// ring the only place the thinking survives at all — see [`render_event`]'s reasoning arm.
///
/// Paired with the handle the row carries: keep a line the row does not name, and the slot is
/// spent on something no click can reach; name a handle for a line this refuses, and the row
/// shows a `#7` that resolves to nothing. The test asserts both halves over one fixture.
pub fn keep_event(line: &str) -> bool {
    Event::parse(line).is_some_and(|event| {
        matches!(
            event.kind.as_str(),
            "tool_use" | "text" | "reasoning" | "error"
        )
    })
}

/// One event line, as a person should read it — [`SessionBinding::Harness`]'s `render`.
///
/// The stream this rewrites is `--format json`, which is the *channel* (see the module header)
/// and which used to reach the pane raw: opening a working run showed ndjson, one event per
/// line, with a whole file read arriving as a single JSON document. Reported, verbatim:
/// *"it opens strange console with json output that isn't understandable"*. The first rendering
/// answered that with a clipped output block under every tool call, and the second report was
/// the other half of the same problem — *"hard to understand what it is doing right now"*, a
/// pane full of six-line previews with no sign of which line was the present. So (M42):
///
/// * a tool call is **one line** — `● name  title  duration #handle` — and its input and whole
///   output are behind the handle, which the pane turns into a click that opens them in a card.
///   A rejected or failed call is red, with the error verbatim on the next line: that line *is*
///   the answer to "why did this run die", and it stays on screen;
/// * **a live marker** — `▸ working…` — sits under the last line from a `step_start` to its
///   `step_finish`, and every line rendered in between erases it and draws it again below
///   itself. The last row of the pane therefore answers the question the report asked: a marker
///   means the model is thinking or a tool is running, no marker means the turn is over;
/// * the model's own text passes whole; a block of reasoning collapses to one dim row naming how
///   long it took — `∴ thought  4.1s #8`, the whole of the thinking behind the same kind of
///   handle a tool call carries (M62, which replaced a clipped 240 characters of it per block);
///   `step_finish` is one dim token count;
/// * an event type this build has never heard of becomes a dim one-word marker rather than a
///   screenful of JSON, and **a line that is not JSON is kept verbatim** — that is the CLI's own
///   prose (a warning, a rejected permission) and hiding it would hide the failure. Under a live
///   marker it is re-emitted beneath the erase instead, so the marker's row accounting holds.
///
/// Every row an event draws opens with a dim `[YYYY-MM-DD HH:MM:SS]` — when the call or the
/// thought began, in local time (see [`stamp_of`]). The live marker and the CLI's own prose get
/// none: the marker is not an event, and the prose is kept byte for byte.
///
/// Styling is bare SGR (dim/bold/cyan/red), which every theme already maps; no colour is load-
/// bearing. The handle token is plain text on purpose: an OSC 8 hyperlink is dropped by the
/// mirror's replay, and a run is mostly read *after* the fact.
pub fn render_event(state: &mut RenderState, line: &str, handle: Option<u64>) -> Rendered {
    let Ok(Value::Object(event)) = serde_json::from_str::<Value>(line.trim()) else {
        return prose(state, line);
    };
    let Some(kind) = event.get("type").and_then(Value::as_str) else {
        return prose(state, line);
    };
    let part = event.get("part").unwrap_or(&Value::Null);

    let text = match kind {
        "step_start" => {
            state.in_step = true;
            None
        }
        "step_finish" => {
            state.in_step = false;
            let reason = part.get("reason").and_then(Value::as_str).unwrap_or("done");
            let total = part
                .pointer("/tokens/total")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Some(format!("{DIM}· {reason} · {} tok{RESET}", thousands(total)))
        }
        "text" => {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
            let text = text.trim_end();
            if text.is_empty() {
                return Rendered::Drop;
            }
            Some(text.to_string())
        }
        // One row, no prose: `∴ thought  4.1s #8`. The whole block is behind the handle.
        //
        // The duration is the part's own `time` where it has one — opencode stamps a completed
        // part with `{"start":…,"end":…}`, which is the harness's own measurement and outranks
        // anything cide could time from outside. `end` is optional on a part, though, so the
        // fallback is the gap since the last line that drew anything; a row with no duration at
        // all is what both refusing looks like, and it is a better answer than `0ms`.
        "reasoning" => {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
            if one_line_of(text).is_empty() {
                return Rendered::Drop;
            }
            let ms = duration_ms(part).or_else(|| measured_ms(state));
            Some(thought_line(ms, handle))
        }
        "tool_use" => Some(render_tool(part, handle)),
        // The shape the CLI actually prints for a failure is top-level — `{"type":"error",
        // "error":{"name":"ProviderAuthError"}}` — with no `part` at all; the older reading
        // of `part` here rendered every one of them as `✗ null`.
        "error" => {
            let error = event.get("error").unwrap_or(part);
            let error = match error {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            Some(format!("{RED}✗ {}{RESET}", one_line_of(&error)))
        }
        other => Some(format!("{DIM}· {other}{RESET}")),
    };
    // Every row the event draws opens with when it happened (`render::stamp`) — asked for so a
    // reader can tell a run that thought for a minute from one that sat for an hour, which the
    // durations alone cannot say because they measure the call, not the gap before it.
    let text = text.map(|text| match stamp_of(state, &event, part) {
        Some(stamp) => format!("{stamp}{text}"),
        None => text,
    });
    compose(state, text)
}

/// The time a row is stamped with: when the call or the thought **began**, where opencode says.
///
/// A `tool_use` and a `reasoning` are emitted once, on completion, so the envelope's `timestamp`
/// is when they *ended* — a two-minute build would be stamped two minutes after it was started,
/// under a row that reads as its start. The part's own `time.start` is the moment a person means
/// by "when did it call that". Past that, the envelope's `timestamp` (every line the shipped
/// emitter prints carries one), and past that the moment the line reached cide, so a release
/// that stops stamping its events still gets a clock rather than none.
fn stamp_of(
    state: &RenderState,
    event: &serde_json::Map<String, Value>,
    part: &Value,
) -> Option<String> {
    part.pointer("/state/time/start")
        .or_else(|| part.pointer("/time/start"))
        .and_then(Value::as_u64)
        .or_else(|| event.get("timestamp").and_then(Value::as_u64))
        .or(state.now_unix_ms)
        .and_then(stamp)
}

/// A tool call on one line: the glyph, the tool, the CLI's own title, the duration, the handle.
fn render_tool(part: &Value, handle: Option<u64>) -> String {
    let tool = display_tool(part.get("tool").and_then(Value::as_str).unwrap_or("tool"));
    let state = part.get("state").unwrap_or(&Value::Null);
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");
    let input = state.get("input").unwrap_or(&Value::Null);
    let title = match state
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) => title.to_string(),
        None => compact_input(input),
    };
    let title = clip(&one_line_of(&title), TITLE_BUDGET);

    // The dim tail: how long it took, and the handle a click resolves. Shared with the thought
    // row since M62 — see `render::tail`, which is why the handle is last.
    let tail = tail(duration_ms(state), handle);

    if status == "error" {
        let error = state
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        return format!(
            "{RED}✗ {tool}{RESET}  {title}{tail}\n{RED}  {}{RESET}",
            one_line_of(error)
        );
    }
    // A shell command that returned non-zero is not an `error` to opencode, but it is the one
    // fact about a `bash` call a reader wants without opening it.
    let exit = state
        .pointer("/metadata/exit")
        .and_then(Value::as_i64)
        .filter(|code| *code != 0)
        .map(|code| format!("  {RED}exit {code}{RESET}"))
        .unwrap_or_default();
    format!("{CYAN}{BOLD}● {tool}{RESET}  {title}{exit}{tail}")
}

/// `cide_cide_task_get` reads as `cide_task_get`: opencode joins the server name and the tool
/// name with an underscore (see [`OpencodeHarness::tool_name`]), and cide's server is called
/// `cide`, so every tracker tool arrives doubled. The tool's own name is what a person knows.
fn display_tool(tool: &str) -> String {
    match tool.strip_prefix("cide_cide_") {
        Some(rest) => format!("cide_{rest}"),
        None => tool.to_string(),
    }
}

/// `time.end - time.start`, in milliseconds, when the CLI recorded both.
///
/// Read from whichever document carries the `time` object: a tool call's `part.state` and, since
/// M62, a reasoning `part` itself. `None` where either end is missing — `end` is absent on a part
/// opencode has not finished, and half an interval is not a duration.
fn duration_ms(value: &Value) -> Option<u64> {
    let start = value.pointer("/time/start").and_then(Value::as_u64)?;
    let end = value.pointer("/time/end").and_then(Value::as_u64)?;
    Some(end.saturating_sub(start))
}

/// The child, fresh (`resume: None`) or continuing a conversation the harness already minted.
///
/// One function for both, so the two children differ in exactly the tokens the difference is
/// about and cannot drift in the environment, the cwd or the configuration.
fn child(
    flavor: &Flavor,
    plan: &RunPlan<'_>,
    resume: Option<&str>,
) -> Result<HarnessSpawn, HarnessError> {
    // `plan.harness`, the *resolved* one, not the definition's: a role a local override moved onto
    // this CLI must not be refused by it. See `RunPlan::harness`. Against the *flavour's* kind, so
    // an opencode plan handed to mimo is refused as surely as a claude one. (M81)
    if plan.harness != flavor.kind {
        return Err(HarnessError::WrongHarness {
            plan: plan.harness,
            harness: flavor.kind,
        });
    }
    // Refused before anything is built. `run` with an empty message is a process that starts, has
    // nothing to do and holds a concurrency slot and a worktree while it works that out.
    if plan.prompt.trim().is_empty() {
        return Err(HarnessError::NoPrompt);
    }
    // Refused rather than degraded, and this is the interesting refusal on this side: without the
    // document there is no role — `--agent <id>` would name an agent that does not exist, the CLI
    // would warn once and fall back to its own default agent, and the run would look like it
    // worked while carrying somebody else's system prompt.
    let config = config_json(flavor, plan).ok_or(HarnessError::NoConfig)?;

    // A bare name, resolved by the OS at this spawn rather than pinned at load: opencode updates
    // itself underneath a running app. `defs::harness_binary` is the one place that name lives,
    // and it is the same one `defs::installed` probed for the roster.
    let program = flavor.program();

    let mut args: Vec<String> = vec![
        "run".into(),
        "--agent".into(),
        plan.agent.def.id.to_string(),
    ];

    // Each of these only when the definition carried it. The CLI's defaults are the safe end of
    // both ranges, and a value invented here is a behaviour the role's author never wrote and
    // cannot find in their file.
    // The pool's candidate outranks the definition's `model:` — for this run only, and only while
    // the run is on that candidate. The role's file is untouched and goes on saying what its
    // author wrote, which is what keeps "why is my agent doing that" answerable from a file.
    let model = plan
        .choice
        .as_ref()
        .map(|choice| choice.entry.model_flag())
        .or_else(|| plan.agent.def.model.clone());
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model);
    }
    // Same precedence one flag down, and it is load-bearing rather than tidy: each model declares
    // its own variant set (`gpt-5.2` offers none/low/medium/high/xhigh, `gpt-5.1-codex-mini` only
    // medium/high), so a role-level `effort` carried onto the candidate a rate limit fell over to
    // would be refused outright by the CLI — turning a recoverable failure into a hard one at the
    // worst possible moment. An entry that names a variant is naming *this* candidate's; the
    // role's `effort` stands for every entry that names none.
    //
    // Carried verbatim and unvalidated either way, for `LoadedAgent::effort`'s stated reason: the
    // set is per-harness and per-release, and a bad value is the CLI's refusal to make, loudly.
    let variant = plan
        .choice
        .as_ref()
        .map(|choice| choice.entry.variant.clone())
        .filter(|variant| !variant.is_empty())
        .or_else(|| plan.agent.effort.clone());
    if let Some(variant) = variant {
        args.push("--variant".into());
        args.push(variant);
    }

    // The project's default for unattended children (`agents.permissionMode`). For this harness
    // the alternative is not a prompt: `run` auto-rejects and, measured, the **turn ends at the
    // rejection**. So the module header's old argument for never passing `--auto` is rewritten
    // there, beside the measurement that disproved it. `auto` takes the flag as well, because this
    // CLI has one switch and no classifier, and the project chose "the run keeps working".
    //
    // Unlike the three other harnesses, this ignores the role's `permission-mode`. That was true
    // before M82 and is left alone: opencode has no counterpart to map a mode onto.
    // Only where this CLI has the flag (M108): an older opencode rejects it and prints its usage
    // instead of running — see `CliFlags`. It also predates the prompts the flag answers.
    let flags = flavor.cli_flags();
    if plan.unattended != Unattended::Ask && flags.run_skip_permissions {
        args.push(flavor.skip_permissions.into());
    }

    // Not a display preference. This is the channel — see the module header.
    args.push("--format".into());
    args.push("json".into());
    // And nor is this, which is why it is not behind a setting. Measured against 1.18.31: the
    // CLI gates its `reasoning` event on a flag, `m = j.mini ? (j.thinking ?? true) :
    // (j.thinking ?? false)`, and cide passes neither `--mini` nor this — so a run's stream
    // carried **no reasoning event at all**, whatever the model did. The collapsed thought row
    // shipped correct and drew nothing, over a stream of 16 `tool_use` and 2 `text` events with
    // the model plainly thinking between them (run c913aa51). `--format json` is the whole
    // reason: without a flag the CLI prints its own `Thinking: …` block only when it is talking
    // to a person, and cide is not a person.
    args.push("--thinking".into());
    args.push("--dir".into());
    args.push(plan.cwd.to_string_lossy().to_string());

    // The run's own server (M104 follow-up): this turn is a client of it, so the conversation
    // lives where a pane can attach the full TUI to it. See `RunPlan::server`.
    // Guarded by `run_password` too: the app starts no server for a CLI without it, and a plan
    // that names one anyway must not produce a client that cannot authenticate.
    let server = plan.server.as_ref().filter(|_| flags.run_password);
    if let Some(server) = server {
        args.push("--attach".into());
        args.push(server.url.clone());
    }

    match resume {
        // The harness's own id, captured off this run's output. Deliberately no `--title`: the
        // session already has one, and passing a second would rename a conversation mid-flight.
        Some(session) => {
            args.push("--session".into());
            args.push(session.to_string());
        }
        // What `opencode session list` and the terminal title show, built like a claude run's
        // `-n` from the title copied at dispatch — so it still reads correctly after the task has
        // been renamed.
        None => {
            args.push("--title".into());
            args.push(session_name(plan));
        }
    }

    // **Last, and nothing may be added after it.** The message is a positional argument; a token
    // written after it would be read as another word of the message.
    args.push(plan.prompt.clone());

    // Not `plan.geometry`, and never resized afterwards — see `RUN_COLS`. The plan's geometry
    // is the default a headless run gets, and for a child that prints lines the pane's later
    // measurement is exactly the resize that would damage the mirror.
    let mut spec = SpawnSpec::new(program, plan.cwd.clone())
        .geometry(PtyGeometry::new(
            RUN_COLS,
            RUN_ROWS,
            plan.geometry.cell_width,
            plan.geometry.cell_height,
        ))
        .fixed_size();
    for arg in args {
        spec = spec.arg(arg);
    }

    spec = run_env(flavor, plan, spec, config);
    // The client proves itself to the run's server with the password the server was started with.
    if let Some(server) = server {
        spec = spec.env(flavor.password_env(), server.password.clone());
    }

    Ok(HarnessSpawn {
        spec,
        // The prompt went in the argv, so there is nothing to type. See `HarnessSpawn::opening`.
        opening: None,
        binding: SessionBinding::Harness {
            capture,
            keep: keep_event,
            render: render_event,
        },
        // The stream *is* this harness's stdout; nothing is written beside it.
        events: None,
    })
}

/// The `--title`: the role, then the task it was dispatched for. `claude.rs`'s `-n`, verbatim.
/// Everything a run's process is given in its environment: the terminal, the proxy, the isolated
/// directories, the configuration document and cide's two identities.
///
/// One function for the turn and for the server (M104 follow-up), and the server is the one that
/// matters most: attached, a `run` child is a thin client and it is the **server** that loads the
/// role, starts cide's MCP bridge — which reads `CIDE_RUN` and `CIDE_AGENT_SOCK` from the process
/// that forks it — and runs every tool. A server built with a second, hand-copied environment
/// would be a run whose tools could not reach the tracker, or whose tests escaped `isolateEnv`.
fn run_env(flavor: &Flavor, plan: &RunPlan<'_>, spec: SpawnSpec, config: String) -> SpawnSpec {
    // The same list a pane and a claude run get, with an **empty** `extra` — see the module
    // header: the user's Claude launch variables are not this child's to inherit.
    let mut spec = spec.apply(cide_core::child_env::terminal_child_env(
        &plan.claude,
        env!("CARGO_PKG_VERSION"),
        Vec::new(),
    ));
    spec = spec.apply(plan.proxy.changes().to_vec());
    spec = spec.apply(plan.env.clone());
    spec = spec.env(flavor.config_env(), config);
    // What scopes this child's MCP connection to this run's task tools, and nothing else — the
    // app resolves the header's `run` against the registry rather than trusting anything the
    // child says about itself.
    spec = spec.env("CIDE_RUN", plan.run.to_string());
    if let Some(sock) = &plan.agent_sock {
        spec = spec.env("CIDE_AGENT_SOCK", sock.to_string_lossy().to_string());
    }

    spec
}

impl Flavor {
    /// The variable the CLI reads its server password from — both as a server and as a client
    /// (`run --attach`, `attach`). Measured in `--help` for both flavours.
    pub fn password_env(&self) -> String {
        format!("{}_SERVER_PASSWORD", self.env_prefix)
    }

    /// The basic-auth user the server expects when none is configured: the CLI's own name.
    /// Measured: `mimo serve` answers 200 to `mimocode:<pw>` and 401 to `opencode:<pw>`. Its
    /// clients default to the same name, so only a caller speaking HTTP itself needs this.
    pub fn server_user(&self) -> String {
        self.env_prefix.to_ascii_lowercase()
    }

    /// The run's headless server: `<cli> serve` on `port`, in the run's directory, with the run's
    /// whole environment. (M104 follow-up) See [`RunPlan::server`].
    ///
    /// # Why a server at all
    ///
    /// `opencode run --port` looks like it would do this and does not: measured on 1.18.32, a run
    /// given `--port` listens on nothing. `serve` does, and a `run --attach` turn against it is
    /// visible there live — `/session/status` answered `busy` for it — while `attach --session`
    /// draws that session in the real TUI. So the run's conversation lives in the server, each
    /// turn is a client of it, and a pane opened on the run is another client: the full,
    /// writable TUI on the very conversation the run is working, rather than a rendering of its
    /// JSON.
    ///
    /// The configuration document is the **first fork's**, and that is enough: a later turn on
    /// another pool entry names its model with `--model` on its own argv, and every provider
    /// that entry can name is already in the document (`provider_members` writes them all).
    pub fn serve_spec(
        &self,
        plan: &RunPlan<'_>,
        port: u16,
        password: &str,
    ) -> Result<SpawnSpec, HarnessError> {
        if plan.harness != self.kind {
            return Err(HarnessError::WrongHarness {
                plan: plan.harness,
                harness: self.kind,
            });
        }
        let config = config_json(self, plan).ok_or(HarnessError::NoConfig)?;
        let mut spec = SpawnSpec::new(self.program(), plan.cwd.clone())
            .geometry(PtyGeometry::new(
                RUN_COLS,
                RUN_ROWS,
                plan.geometry.cell_width,
                plan.geometry.cell_height,
            ))
            .fixed_size();
        for arg in [
            "serve".to_string(),
            "--port".to_string(),
            port.to_string(),
            "--hostname".to_string(),
            "127.0.0.1".to_string(),
        ] {
            spec = spec.arg(arg);
        }
        spec = run_env(self, plan, spec, config);
        Ok(spec.env(self.password_env(), password.to_string()))
    }

    /// A pane's child on a conversation a live run server holds: the full TUI, attached.
    ///
    /// The password is not here — it travels in the environment ([`Self::password_env`]), which
    /// the caller sets, because an argv is readable by every process on the machine.
    pub fn attach_spec(&self, conversation: &HarnessSession, url: &str) -> ContinueSpec {
        ContinueSpec {
            program: self.program().to_string(),
            args: vec![
                "attach".into(),
                url.to_string(),
                "--dir".into(),
                conversation.cwd.to_string_lossy().into_owned(),
                "--session".into(),
                conversation.id.trim().to_string(),
            ],
            resume: None,
        }
    }
}

fn session_name(plan: &RunPlan<'_>) -> String {
    match plan
        .task_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(title) => format!("{} · {}", plan.agent.def.label, title),
        None => plan.agent.def.label.clone(),
    }
}

/// mimo's `retry` block, bounding the classes it would otherwise retry for a very long time —
/// or `None` for a flavour that gives up by itself. (M81)
///
/// # Measured, because the failure is a run that hangs rather than one that fails
///
/// mimo 0.1.15 carries a retry policy per error class (`request`, `stream`, `network`, `server`,
/// `rateLimit`, `unknown`), read from a top-level `retry` key. `network`, `server` and `rateLimit`
/// are `persistent` — retried for ever — and `unknown` allows eight tries over fifteen minutes.
/// A provider on a closed port lands in `unknown`: probed, it printed **nothing** for two
/// minutes, and bounding `network` alone changed nothing, while bounding `unknown` ended it in
/// eighteen seconds with the same `APIError { isRetryable: true }` line opencode prints — which
/// [`failover`] already classifies as `Unreachable`.
///
/// So under a pool all four are bounded: a pool exists to *move on*, and a candidate that the
/// CLI retries silently for a quarter of an hour is a pool that never fails over. Without one,
/// nothing is written and mimo's own patience stands, since there is nowhere else to go. The
/// keys are spelled as mimo's `debug config` echoes them back: `maxElapsedMs` is dropped by its
/// schema, and `deadlineMs` is the key it keeps.
fn pool_retry(flavor: &Flavor) -> Option<Value> {
    if !flavor.failure_exits_zero {
        return None;
    }
    let bounded = json!({ "mode": "bounded", "maxRetries": 2, "deadlineMs": 30_000 });
    Some(json!({
        "network": bounded,
        "server": bounded,
        "rateLimit": bounded,
        "unknown": bounded,
    }))
}

/// The whole `OPENCODE_CONFIG_CONTENT` document: this role, and cide's MCP server.
///
/// # Built with serde, never `format!`
///
/// A system prompt is a multi-paragraph document a human wrote. It contains quotation marks, it
/// contains backslashes, and it contains newlines — and a `format!`-built document would turn any
/// of them into a configuration that parses as something *else*: a truncated prompt, a different
/// key, or a document the CLI rejects wholesale, in which case the run silently becomes opencode's
/// default agent. `claude.rs`'s `--mcp-config` builder makes the same argument for a worktree path
/// with an apostrophe in it; this one is worse, because the hostile input is the feature.
///
/// # The shape, checked against `https://opencode.ai/config.json`
///
/// `$defs.AgentConfig` accepts `model`, `variant`, `temperature`, `top_p`, `prompt`, `disable`,
/// `description`, `mode`, `hidden`, `steps`, `permission`, `options` and `color`; `tools` is
/// deprecated in favour of `permission`. Four are emitted and the module header argues each
/// absence.
///
/// `$defs.McpLocalConfig` requires `type: "local"` and a `command` **array** — program first, then
/// its arguments, unlike the `{command, args}` shape claude's `--mcp-config` takes. The path is
/// absolute for the reason `RunPlan::hook_bin` states: the child's cwd is a worktree and its
/// `PATH` is the user's, so a bare `cide-hook` resolves to nothing and the failure arrives as a
/// connection error from a process three levels below anything cide logs.
///
/// `None` is unreachable — `serde_json::to_string` of a `Value` whose keys are all strings cannot
/// fail — and is still an `Option` rather than an `unwrap`, because the caller turns it into a
/// refusal and a refused run is always better than a panicking one.
fn config_json(flavor: &Flavor, plan: &RunPlan<'_>) -> Option<String> {
    let def = &plan.agent.def;

    let mut role = serde_json::Map::new();
    role.insert("description".into(), json!(def.description));
    // Load-bearing: `--agent` refuses a `subagent`-mode agent and falls back to the default one.
    role.insert("mode".into(), json!("primary"));
    // The role's own brief **first and byte for byte**, then — only when the `mcp` block below is
    // actually written — cide's paragraph about the task tracker, separated by a blank line.
    //
    // The order is the claim: the role's file is what this run is for, and cide's housekeeping is
    // an addition to it rather than a preamble in front of it. The join is `\n\n`, which is the
    // join `cide_core::claude_cli::fold_append_system_prompt` makes on the claude side, so one
    // role pointed at either CLI reads the same two paragraphs in the same order.
    //
    // `plan.hook_bin` is the gate, and it is the same value the `mcp` block is written from a few
    // lines down — one `if let` there, one `is_some` here, and no way for a run to be told about
    // tools that were never attached. See `TRACKER_PREAMBLE`'s header.
    role.insert(
        "prompt".into(),
        // `&& tracker_paragraphs`: an MR review keeps the bridge and hears none of this (M85).
        json!(match plan.hook_bin.is_some() && plan.tracker_paragraphs {
            true => {
                // Three paragraphs at most, in the same order and with the same `\n\n` join the
                // claude side's fold produces — so one role pointed at either CLI reads the same
                // brief. (M28 added the third.)
                let mut prompt = format!(
                    "{}\n\n{}",
                    def.system_prompt,
                    tracker_preamble(flavor.harness())
                );
                if let Some(change) = plan.change.as_deref() {
                    prompt.push_str("\n\n");
                    prompt.push_str(&super::spec_preamble(
                        change,
                        plan.spec_cli.as_deref(),
                        plan.spec_apply.as_deref(),
                        flavor.harness(),
                    ));
                }
                // And, for a run with no task, the correction to the tracker paragraph — last,
                // exactly where the claude side folds it, so `harness.rs`'s parity test holds.
                // (M40)
                if plan.adhoc() {
                    prompt.push_str("\n\n");
                    prompt.push_str(ADHOC_PREAMBLE);
                }
                prompt
            }
            false => def.system_prompt.clone(),
        }),
    );
    // Also on the command line as `--model`, which is what *this run* uses. Here as well so the
    // registered agent is self-consistent — `opencode agent list` and any other opencode process
    // that reads this definition see the model the role names rather than a provider default.
    // The **pool's** candidate here too, or `opencode agent list` and every other process that
    // reads this document describe an agent on a model this run is not using.
    if let Some(model) = plan
        .choice
        .as_ref()
        .map(|choice| choice.entry.model_flag())
        .or_else(|| def.model.clone())
    {
        role.insert("model".into(), json!(model));
    }

    let mut agents = serde_json::Map::new();
    agents.insert(def.id.to_string(), Value::Object(role));

    let mut config = serde_json::Map::new();
    config.insert("$schema".into(), json!(flavor.schema));
    config.insert("agent".into(), Value::Object(agents));
    // The user's providers, from global settings. Merged into this document rather than built
    // beside it, so a run and the model probe cannot disagree — see `provider_members`.
    config.extend(provider_members(&plan.llm));
    // Only on a pool: see `pool_retry`.
    if plan.choice.is_some()
        && let Some(retry) = pool_retry(flavor)
    {
        config.insert("retry".into(), retry);
    }

    if let Some(hook) = &plan.hook_bin {
        let mut servers = serde_json::Map::new();
        servers.insert(
            SERVER.into(),
            json!({
                "type": "local",
                // `cide-hook mcp` is the bridge: stdio in, `$CIDE_AGENT_SOCK` out, and no
                // knowledge of the vocabulary at all. See its module doc.
                "command": [hook.to_string_lossy(), "mcp"],
                "enabled": true,
            }),
        );
        config.insert("mcp".into(), Value::Object(servers));
    }

    serde_json::to_string(&Value::Object(config)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::render::{ERASE_MARKER, MARKER};

    /// The tracker paragraph as *this* harness renders it — `cide_cide_task_get`, not
    /// `mcp__cide__cide_task_get`. Derived from the one definition, never quoted.
    fn tracker() -> String {
        tracker_preamble(&OpencodeHarness)
    }

    /// The run's server and the turns attached to it (M104 follow-up): one environment for both,
    /// the password never in an argv, and `--attach` before the positional message.
    #[test]
    fn a_served_run_attaches_each_turn_and_the_server_carries_the_runs_environment() {
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.server = Some(crate::RunServer {
            url: "http://127.0.0.1:47000".into(),
            password: "pw".into(),
        });

        let server = OPENCODE_CLI
            .serve_spec(&plan, 47000, "pw")
            .expect("a server");
        assert_eq!(
            server.args,
            ["serve", "--port", "47000", "--hostname", "127.0.0.1"]
        );
        assert_eq!(env_value(&server, "OPENCODE_SERVER_PASSWORD"), Some("pw"));
        assert_eq!(
            env_value(&server, "CIDE_RUN"),
            Some(plan.run.to_string().as_str())
        );
        // The role and cide's MCP server live in the server: it is what loads them.
        let config: Value =
            serde_json::from_str(env_value(&server, "OPENCODE_CONFIG_CONTENT").expect("a config"))
                .expect("json");
        assert!(config["agent"]["developer"].is_object(), "{config}");
        assert!(config["mcp"][SERVER].is_object(), "{config}");

        let turn = OpencodeHarness.spawn_spec(&plan).expect("a turn").spec;
        let at = turn
            .args
            .iter()
            .position(|a| a == "--attach")
            .expect("attached");
        assert_eq!(turn.args[at + 1], "http://127.0.0.1:47000");
        assert_eq!(turn.args.last(), Some(&plan.prompt));
        assert!(!turn.args.iter().any(|a| a == "pw"), "{:?}", turn.args);
        assert_eq!(env_value(&turn, "OPENCODE_SERVER_PASSWORD"), Some("pw"));

        // Standalone, as every run was before: no flag, no password.
        plan.server = None;
        let turn = OpencodeHarness.spawn_spec(&plan).expect("a turn").spec;
        assert!(!turn.args.iter().any(|a| a == "--attach"));
        assert_eq!(env_value(&turn, "OPENCODE_SERVER_PASSWORD"), None);

        let attach = MIMO_CLI.attach_spec(
            &HarnessSession {
                harness: cide_ipc::Harness::Mimo,
                id: SESSION.into(),
                cwd: PathBuf::from("/repo/.cide/worktrees/developer-t-14"),
            },
            "http://127.0.0.1:47001",
        );
        assert_eq!(
            attach.args,
            [
                "attach",
                "http://127.0.0.1:47001",
                "--dir",
                "/repo/.cide/worktrees/developer-t-14",
                "--session",
                SESSION
            ]
        );
        assert_eq!(MIMO_CLI.password_env(), "MIMOCODE_SERVER_PASSWORD");
        assert_eq!(MIMO_CLI.server_user(), "mimocode");
        assert_eq!(OPENCODE_CLI.server_user(), "opencode");
    }

    /// The usage a real older opencode printed on a Mac (M108), trimmed to its options, against
    /// the 1.18.32 one here: only the flags that exist are passed.
    #[test]
    fn an_older_cli_is_given_only_the_flags_it_has() {
        const OLD_RUN: &str = "opencode run [message..]\n\nOptions:\n  -h, --help  show help\n      \
            --model  model\n      --agent  agent\n      --format  format\n      --title  title\n      \
            --attach  attach\n      --dir  dir\n      --port  port\n      --variant  variant\n      \
            --thinking  show thinking blocks";
        let old = CliFlags::from_help("--auto", OLD_RUN, "opencode [project]\n  --model");
        assert_eq!(
            old,
            CliFlags {
                run_skip_permissions: false,
                run_password: false,
                tui_skip_permissions: false,
            }
        );
        const NEW_RUN: &str = "      --attach  attach\n  -p, --password  basic auth password\n      \
            --auto  auto-approve permissions";
        let new = CliFlags::from_help("--auto", NEW_RUN, "      --auto  auto-approve");
        assert_eq!(new, CliFlags::default());
        // A word that merely contains the flag is not the flag.
        assert!(!CliFlags::from_help("--auto", "--autocomplete", "").run_skip_permissions);
    }

    /// M104's tab: the TUI, `--prompt` last, and a document with cide's server and no role.
    #[test]
    fn a_tab_is_the_tui_with_the_task_tools_and_no_role() {
        let hook = PathBuf::from("/opt/cide/cide-hook");
        let document = OPENCODE_CLI
            .tab_config(&cide_ipc::LlmSettings::default(), Some(&hook))
            .expect("a document");
        let value: Value = serde_json::from_str(&document).expect("json");
        assert_eq!(
            value["mcp"][SERVER]["command"],
            json!(["/opt/cide/cide-hook", "mcp"])
        );
        assert!(value.get("agent").is_none(), "{document}");
        // Nothing to say, nothing set: the user's own configuration is all the CLI reads.
        assert_eq!(
            OPENCODE_CLI.tab_config(&cide_ipc::LlmSettings::default(), None),
            None
        );
    }

    use crate::defs::LoadedAgent;
    use cide_claude::HookFrame;
    use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, TaskId, Theme};
    use std::path::PathBuf;

    /// Three lines captured from a real `opencode run --format json`, filled out to the shape the
    /// shipped emitter writes: `{type, timestamp, sessionID, ...part}`.
    ///
    /// Note the two spellings of one event, which is not a transcription error: the **envelope's**
    /// `type` is `step_start`, the **part's** is `step-start`. Anything reading the envelope must
    /// read the underscore form, and a fixture that quietly normalised them would let a bug
    /// through in exactly the field this file dispatches on.
    const STEP_START: &str = r#"{"type":"step_start","timestamp":1755402000000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c30ffe8Ab","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"step-start"}}"#;
    const TEXT: &str = r#"{"type":"text","timestamp":1755402001000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c40ffe8Ac","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"text","text":"middle","time":{"start":1755402000500,"end":1755402001000}}}"#;
    const STEP_FINISH: &str = r#"{"type":"step_finish","timestamp":1755402002000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c50ffe8Ad","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"step-finish","reason":"stop","cost":0,"tokens":{"total":5725,"input":5696,"output":4,"reasoning":25,"cache":{"read":0,"write":0}}}}"#;
    /// The same event in the middle of a turn: one step ended and another is about to begin.
    const STEP_FINISH_TOOLS: &str = r#"{"type":"step_finish","timestamp":1755402003000,"sessionID":"ses_fe6da2c1effe8Yz","part":{"id":"prt_fe6da2c60ffe8Ae","sessionID":"ses_fe6da2c1effe8Yz","messageID":"msg_fe6da2c2affe8Aa","type":"step-finish","reason":"tool-calls","cost":0.0012,"tokens":{"total":812,"input":700,"output":112,"reasoning":0,"cache":{"read":0,"write":0}}}}"#;

    const SESSION: &str = "ses_fe6da2c1effe8Yz";

    /// The role every test starts from: enough to spawn, nothing optional set.
    fn role() -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope: cide_ipc::agents::AgentScope::Project,
                harness: cide_ipc::Harness::Opencode,
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
        }
    }

    fn plan_for(agent: &LoadedAgent) -> RunPlan<'_> {
        RunPlan {
            run: RunId::new(),
            session: SessionId::new(),
            agent,
            cwd: PathBuf::from("/repo/.cide/worktrees/developer"),
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
            events_path: None,
            theme: Theme::Dark,
            proxy: cide_core::proxy::ProxyEnv::default(),
            env: Vec::new(),
            geometry: Geometry::default(),
            claude: cide_ipc::ClaudeSettings::default(),
            codex: cide_ipc::CodexSettings::default(),
            // Empty in the fixture, so every existing assertion is about the document as it was
            // before providers existed. The provider tests build their own.
            llm: cide_ipc::LlmSettings::default(),
            choice: None,
            harness: agent.def.harness,
            // Off in the fixture, so every argv assertion below is about what the role
            // and the plan actually said; the skip default has tests of its own.
            unattended: Unattended::Ask,
            tracker_paragraphs: true,
            server: None,
        }
    }

    fn spawn(plan: &RunPlan<'_>) -> HarnessSpawn {
        OpencodeHarness.spawn_spec(plan).expect("a spawnable plan")
    }

    fn at(args: &[String], token: &str) -> usize {
        args.iter()
            .position(|a| a == token)
            .unwrap_or_else(|| panic!("no {token} in {args:?}"))
    }

    fn value_of<'a>(args: &'a [String], flag: &str) -> &'a str {
        &args[at(args, flag) + 1]
    }

    fn env_value<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        // Last wins, matching what the child actually gets.
        spec.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn config_of(spec: &SpawnSpec) -> Value {
        serde_json::from_str(env_value(spec, "OPENCODE_CONFIG_CONTENT").expect("a config"))
            .expect("the config is json")
    }

    /// The pane's rendering of the event stream, over the shapes a real run printed — including
    /// the two that motivated it: a whole-file `read` arriving as one JSON line, and a rejected
    /// permission, which is the line that answers "why did this run die". (M42: one line per
    /// tool call, the live marker between a step's start and its finish, the handle a click
    /// resolves.)
    #[test]
    fn the_rendering_reads_as_a_transcript_and_hides_no_failure() {
        let mut state = RenderState::default();

        // A step starting draws the live marker and nothing else; the step finishing erases it
        // and leaves one dim token count, with no marker under it.
        let started = render_event(&mut state, STEP_START, None)
            .text()
            .expect("the marker")
            .to_string();
        assert!(started.contains("working"), "{started:?}");
        assert!(state.in_step && state.marker);
        let finish = render_event(&mut state, STEP_FINISH, None)
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            finish.starts_with(ERASE_MARKER),
            "the marker row is erased before the next line: {finish:?}"
        );
        assert!(
            finish.contains("stop") && finish.contains("5.7k tok"),
            "{finish}"
        );
        assert!(!finish.contains("working") && !state.marker && !state.in_step);

        // A tool call is one line: the tool, the CLI's own title, the duration, the handle —
        // and none of its output, which is what the handle is for.
        let big_output = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8";
        let tool = format!(
            r#"{{"type":"tool_use","sessionID":"s","part":{{"type":"tool","tool":"bash","state":{{"status":"completed","input":{{"command":"ls -la"}},"output":{},"title":"ls -la","time":{{"start":1000,"end":2200}}}}}}}}"#,
            serde_json::to_string(big_output).unwrap()
        );
        let rendered = render_event(&mut state, &tool, Some(7))
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            rendered.contains("● bash") && rendered.contains("ls -la") && rendered.contains("1.2s"),
            "{rendered}"
        );
        assert!(
            rendered.ends_with(&format!("#7{RESET}")),
            "the handle is the last thing on the line, for `runLinks.ts`: {rendered:?}"
        );
        assert!(
            !rendered.contains('\n') && !rendered.contains("line1"),
            "a completed call is one line; its output is behind the handle: {rendered:?}"
        );
        assert!(
            !rendered.contains(r#""status""#),
            "JSON structure leaked into the display: {rendered}"
        );

        // Inside a step, every rendered line erases the marker and draws it again below itself.
        render_event(&mut state, STEP_START, None);
        let within = render_event(&mut state, &tool, Some(8))
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            within.starts_with(ERASE_MARKER) && within.ends_with(MARKER),
            "{within:?}"
        );
        assert!(state.marker);
        // The CLI's own prose under a marker goes beneath the erase too, so the row accounting
        // holds; with no marker on screen it is kept exactly as it arrived.
        let prose = render_event(&mut state, "! permission requested: bash (*)", None)
            .text()
            .expect("re-emitted under the marker")
            .to_string();
        assert!(
            prose.starts_with(ERASE_MARKER)
                && prose.contains("permission requested")
                && prose.ends_with(MARKER),
            "{prose:?}"
        );
        render_event(&mut state, STEP_FINISH, None);
        assert_eq!(
            render_event(&mut state, "! permission requested: bash (*)", None),
            Rendered::Keep,
            "the CLI's own prose is kept as it arrived — rendering it back as itself would \
             re-terminate it and lose the bytes the child actually wrote"
        );

        // The rejected permission — the failure that used to be findable only in opencode's own
        // database — is red and verbatim, on the line under the call.
        let rejected = r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"bash","state":{"status":"error","input":{"command":"cat ~/.cargo/config.toml"},"error":"The user rejected permission to use this specific tool call."}}}"#;
        let rendered = render_event(&mut state, rejected, Some(9))
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            rendered.contains("✗ bash") && rendered.contains("rejected permission"),
            "{rendered}"
        );
        let first = rendered.lines().next().expect("the call line");
        assert!(first.ends_with(&format!("#9{RESET}")), "{first:?}");

        // A shell command that returned non-zero says so on its line; a tracker tool reads under
        // its own name rather than the doubled server prefix.
        let failed = r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"false"},"output":"","title":"false","metadata":{"exit":1}}}}"#;
        let rendered = render_event(&mut state, failed, None)
            .text()
            .expect("rendered")
            .to_string();
        assert!(rendered.contains("exit 1"), "{rendered}");
        let tracker = r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"cide_cide_task_get","state":{"status":"completed","input":{"id":"t-14"},"output":"…","title":""}}}"#;
        let rendered = render_event(&mut state, tracker, None)
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            rendered.contains("● cide_task_get")
                && !rendered.contains("cide_cide")
                && rendered.contains("t-14"),
            "{rendered}"
        );

        // The model's words pass whole.
        let text = r#"{"type":"text","sessionID":"s","part":{"type":"text","text":"The task is already in doing.\n\nChecking the build."}}"#;
        let rendered = render_event(&mut state, text, None)
            .text()
            .expect("rendered")
            .to_string();
        assert!(rendered.contains("Checking the build."), "{rendered}");

        // A block of thinking is one row naming how long it took, and **none** of the thinking.
        // The whole of it is behind the handle — the collapse is the feature, so a rendering
        // that put the prose back would still pass every assertion about the duration and the
        // token below.
        let thinking = "x".repeat(500);
        let reasoning = format!(
            r#"{{"type":"reasoning","sessionID":"s","part":{{"type":"reasoning","text":{},"time":{{"start":1000,"end":5100}}}}}}"#,
            serde_json::to_string(&thinking).unwrap()
        );
        let rendered_reasoning = render_event(&mut state, &reasoning, Some(8))
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            rendered_reasoning.contains("∴ thought")
                && rendered_reasoning.contains("4.1s")
                && rendered_reasoning.contains("#8")
                && !rendered_reasoning.contains("xxx"),
            "{rendered_reasoning}"
        );

        // The part's own `time` outranks the clock, but `end` is optional on a part: without it
        // the row falls back to the silence it ended. Neither available — an unstamped caller,
        // a first line — draws no duration at all, and never `0ms`, which is a measurement cide
        // did not make.
        let untimed =
            r#"{"type":"reasoning","sessionID":"s","part":{"type":"reasoning","text":"mm"}}"#;
        let mut timed = RenderState {
            now_unix_ms: Some(9_000),
            last_line_unix_ms: Some(6_500),
            ..RenderState::default()
        };
        let rendered = render_event(&mut timed, untimed, None)
            .text()
            .expect("rendered")
            .to_string();
        assert!(rendered.contains("2.5s"), "{rendered}");
        let rendered = render_event(&mut RenderState::default(), untimed, Some(3))
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            rendered.contains("∴ thought") && rendered.contains("#3") && !rendered.contains("ms"),
            "{rendered}"
        );

        // A row opens with when it began: the part's own `time.start`, then the envelope's
        // `timestamp`. Fixtures stamped `1000` or `1` are not clocks and draw no stamp, which is
        // what keeps every assertion above about a row's *start* honest.
        let stamped = r#"{"type":"tool_use","timestamp":1790000060000,"sessionID":"s","part":{"type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"ls"},"output":"","title":"ls","time":{"start":1790000000000,"end":1790000060000}}}}"#;
        let rendered = render_event(&mut RenderState::default(), stamped, Some(4))
            .text()
            .expect("rendered")
            .to_string();
        let expected = super::super::render::stamp(1_790_000_000_000).expect("a clock");
        assert!(
            rendered.starts_with(&format!("{expected}{CYAN}{BOLD}● bash")),
            "the stamp is the call's start, ahead of the glyph: {rendered:?}"
        );
        let envelope = r#"{"type":"text","timestamp":1790000060000,"sessionID":"s","part":{"type":"text","text":"done"}}"#;
        let rendered = render_event(&mut RenderState::default(), envelope, None)
            .text()
            .expect("rendered")
            .to_string();
        let expected = super::super::render::stamp(1_790_000_060_000).expect("a clock");
        assert_eq!(rendered, format!("{expected}done"));

        // A failure the CLI reports at the top level — no `part` at all — names itself.
        let error = r#"{"type":"error","timestamp":1,"sessionID":"ses_x","error":{"name":"ProviderAuthError"}}"#;
        let rendered = render_event(&mut state, error, None)
            .text()
            .expect("rendered")
            .to_string();
        assert!(
            rendered.contains("ProviderAuthError") && !rendered.contains("null"),
            "{rendered}"
        );

        // An event type this build has never met becomes a dim marker, not a screenful of JSON.
        let unknown = render_event(
            &mut state,
            r#"{"type":"session_share","sessionID":"s"}"#,
            None,
        )
        .text()
        .expect("marker")
        .to_string();
        assert!(
            unknown.contains("session_share") && !unknown.contains("sessionID"),
            "{unknown}"
        );

        // What a person may ask for whole, and what nobody will. Reasoning is kept **and** the
        // row names a handle, asserted together over one fixture: keep a line the row does not
        // name and the slot is spent on something no click reaches; name a handle for a line
        // this refuses and the row shows a `#7` that resolves to nothing.
        assert!(keep_event(&tool) && keep_event(text) && keep_event(error));
        assert!(keep_event(&reasoning) && rendered_reasoning.contains("#8"));
        assert!(!keep_event(STEP_START) && !keep_event("! permission requested: bash (*)"));
    }

    /// `--auto` rides the project default (`agents.permissionMode`) — and the message stays the
    /// last token either way, because a flag written after the positional would be read as
    /// another word of the message. The module header carries the measurement that reversed the
    /// old never-pass-`--auto` stance: a headless auto-reject does not refuse one tool, it ends
    /// the turn.
    #[test]
    fn the_skip_default_passes_auto_and_the_message_stays_last() {
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.unattended = Unattended::Auto;
        let args = spawn(&plan).spec.args;
        assert!(args.iter().any(|a| a == "--auto"), "{args:?}");
        assert_eq!(
            args.last().map(String::as_str),
            Some(plan.prompt.as_str()),
            "a token after the positional message is another word of the message: {args:?}"
        );

        // Off means off — the way back to auto-rejects is one config line, and it must work.
        let args = spawn(&plan_for(&agent)).spec.args;
        assert!(!args.iter().any(|a| a == "--auto"), "{args:?}");
    }

    /// The document is valid JSON and the role's prompt survives it **byte for byte**, which is
    /// the whole reason it is built with serde: a prompt is a document a human wrote, so a quote,
    /// a backslash and a newline are ordinary content rather than exotic input.
    ///
    /// (M40) The task-less twin of the test below: the correction to the tracker paragraph is the
    /// third paragraph of the inline agent's prompt, after the other two, so a role pointed at
    /// either binary reads the same brief — `harness.rs`'s parity test is the cross-check.
    #[test]
    fn the_config_tells_an_adhoc_run_it_stands_in_the_users_tree() {
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.task = None;
        plan.task_title = None;
        let config = config_of(&spawn(&plan).spec);
        assert_eq!(
            config["agent"]["developer"]["prompt"],
            json!(format!(
                "{}\n\n{}\n\n{ADHOC_PREAMBLE}",
                agent.def.system_prompt,
                tracker()
            ))
        );
        // No bridge, no tracker paragraph, no correction: the role's own words alone.
        plan.hook_bin = None;
        assert_eq!(
            config_of(&spawn(&plan).spec)["agent"]["developer"]["prompt"],
            json!(agent.def.system_prompt)
        );
    }

    /// cide's paragraph about the task tracker follows it, after a blank line and never before it,
    /// and the expectation is built from [`TRACKER_PREAMBLE`] rather than from a quoted copy — so
    /// a second definition of that prose cannot pass this file, which is the point of there being
    /// one.
    #[test]
    fn the_config_carries_the_roles_prompt_verbatim() {
        let mut agent = role();
        let prompt = "Say \"hello\".\nUse C:\\tools\\bin and never \\n as a literal.\nReport back.";
        agent.def.system_prompt = prompt.into();

        let config = config_of(&spawn(&plan_for(&agent)).spec);
        assert_eq!(
            config["agent"]["developer"]["prompt"],
            json!(format!("{prompt}\n\n{}", tracker()))
        );
        assert_eq!(
            config["agent"]["developer"]["description"],
            json!("Implements one task end to end.")
        );
        // Load-bearing rather than decorative: the CLI refuses to select a `subagent`-mode agent
        // from `--agent` and silently falls back to its own default one.
        assert_eq!(config["agent"]["developer"]["mode"], json!("primary"));
        assert_eq!(config["$schema"], json!(OPENCODE_CLI.schema));

        // The key is the role's id, because that is what `--agent` names.
        assert_eq!(
            value_of(&spawn(&plan_for(&agent)).spec.args, "--agent"),
            "developer"
        );
    }

    /// The other half of the document: cide's own MCP server, so an opencode run reaches the task
    /// tracker exactly as a claude run does.
    #[test]
    fn the_mcp_server_names_the_hook_binary_in_the_shape_the_schema_wants() {
        let agent = role();
        let config = config_of(&spawn(&plan_for(&agent)).spec);

        let server = &config["mcp"]["cide"];
        // `McpLocalConfig`: `type` and a `command` **array**, program first — not claude's
        // `{command, args}` pair.
        assert_eq!(server["type"], json!("local"));
        assert_eq!(server["command"], json!(["/opt/cide/cide-hook", "mcp"]));
        assert_eq!(server["enabled"], json!(true));

        // A path with a quote in it is the case serde is used for, so it is the case asserted.
        let mut plan = plan_for(&agent);
        plan.hook_bin = Some(PathBuf::from("/home/o\"brien/bin/cide-hook"));
        let config = config_of(&spawn(&plan).spec);
        assert_eq!(
            config["mcp"]["cide"]["command"],
            json!(["/home/o\"brien/bin/cide-hook", "mcp"])
        );
    }

    /// The paragraph and the server are one decision, gated on one value.
    ///
    /// `plan.hook_bin` is what writes the `mcp` block, so it is also what licenses the sentences
    /// telling the role to call `mcp__cide__cide_task_*`. Without the bridge there is no such
    /// server in this document — the user's own `opencode.json` may still have others — and a role
    /// pointed at a vocabulary it cannot see would try, fail, and have nothing to say about why.
    /// The role's own prompt is then in the document exactly as its file wrote it.
    #[test]
    fn without_the_bridge_there_is_no_server_and_nothing_is_said_about_it() {
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.hook_bin = None;

        let config = config_of(&spawn(&plan).spec);
        assert!(config.get("mcp").is_none(), "{config}");
        assert_eq!(
            config["agent"]["developer"]["prompt"],
            json!(agent.def.system_prompt)
        );
    }

    /// The invocation shape, in the order it is written, with the message last.
    #[test]
    fn the_argv_is_the_measured_invocation() {
        let mut agent = role();
        agent.def.model = Some("anthropic/claude-sonnet-4-5".into());
        agent.effort = Some("high".into());
        let plan = plan_for(&agent);
        let spawned = spawn(&plan);
        let args = &spawned.spec.args;

        assert_eq!(args[0], "run", "{args:?}");
        assert_eq!(value_of(args, "--agent"), "developer");
        assert_eq!(value_of(args, "--model"), "anthropic/claude-sonnet-4-5");
        assert_eq!(value_of(args, "--variant"), "high");
        assert_eq!(value_of(args, "--format"), "json");
        // Without this the CLI emits no `reasoning` event at all and the thought row draws
        // nothing — a feature that is correct everywhere and invisible. The flag is the stream's
        // content, not a display preference, so it is asserted beside `--format`.
        assert!(args.iter().any(|arg| arg == "--thinking"), "{args:?}");
        assert_eq!(value_of(args, "--dir"), "/repo/.cide/worktrees/developer");
        assert_eq!(
            value_of(args, "--title"),
            "Developer · Teach the parser about tabs"
        );

        // The message is positional, so it is last and nothing may be written after it.
        assert_eq!(args.last().map(String::as_str), Some(&*plan.prompt));
        // And it is *not* typed into the terminal: there is no interactive prompt to type into,
        // which is the same fact `deliver` answers `Respawn` to.
        assert_eq!(spawned.opening, None);
        assert_eq!(
            spawned.spec.cwd,
            PathBuf::from("/repo/.cide/worktrees/developer")
        );
        assert_eq!(spawned.spec.program, "opencode");
    }

    /// What a definition did not say, cide does not say either.
    #[test]
    fn nothing_the_definition_left_out_reaches_the_command_line() {
        let agent = role();
        let spawned = spawn(&plan_for(&agent));
        let args = &spawned.spec.args;

        for absent in ["--model", "--variant", "--session", "--auto"] {
            assert!(
                !args.iter().any(|a| a == absent),
                "{absent} must not appear for a plan that did not carry it: {args:?}"
            );
        }
        // `tools` is deprecated in favour of `permission`, and cide has no honest translation from
        // a claude `--allowedTools` list into an opencode permission table.
        let config = config_of(&spawned.spec);
        assert_eq!(config["agent"]["developer"].get("tools"), None);
        assert_eq!(config["agent"]["developer"].get("permission"), None);
    }

    /// The environment: what the child is told, and the two names that are deliberately absent.
    #[test]
    fn the_child_is_told_which_run_it_is_and_nothing_it_cannot_use() {
        let agent = role();
        let plan = plan_for(&agent);
        let spec = spawn(&plan).spec;

        assert_eq!(env_value(&spec, "CIDE_RUN"), Some(&*plan.run.to_string()));
        assert_eq!(
            env_value(&spec, "CIDE_AGENT_SOCK"),
            Some("/run/user/1000/cide-agents-42.sock")
        );
        // No hook can be installed into this CLI, so a routing key would be a name nobody reads —
        // and a hook socket would be a path nothing writes to. Liveness comes from the stream.
        assert_eq!(env_value(&spec, "CIDE_SESSION"), None);
        assert_eq!(env_value(&spec, "CIDE_HOOK_SOCK"), None);

        // The tempting wrong move, refused in the test as well as in the comment: a project's own
        // `opencode.json` is the user's configuration.
        assert_eq!(env_value(&spec, "OPENCODE_DISABLE_PROJECT_CONFIG"), None);
        assert!(
            !spec
                .env_remove
                .iter()
                .any(|k| k == "OPENCODE_DISABLE_PROJECT_CONFIG"),
            "{:?}",
            spec.env_remove
        );

        // The shared list every child in this application gets.
        assert_eq!(env_value(&spec, "TERM"), Some("xterm-256color"));
        assert_eq!(env_value(&spec, "TERM_PROGRAM"), Some("cide"));
    }

    /// A follow-up is a new child that continues the same conversation.
    #[test]
    fn a_follow_up_respawns_into_the_session_the_harness_minted() {
        let agent = role();
        let plan = plan_for(&agent);

        assert_eq!(OpencodeHarness.deliver("carry on"), Delivery::Respawn);

        let spawned = OpencodeHarness
            .respawn_spec(&plan, SESSION)
            .expect("a continuable plan");
        let args = &spawned.spec.args;
        assert_eq!(value_of(args, "--session"), SESSION);
        // The conversation already has a title; renaming it mid-flight is not this call's to do.
        assert!(!args.iter().any(|a| a == "--title"), "{args:?}");
        // Same worktree, same configuration, same message position — only the two tokens above
        // differ, which is why one function builds both.
        assert_eq!(spawned.spec.cwd, plan.cwd);
        assert_eq!(args.last().map(String::as_str), Some(&*plan.prompt));
        // The configuration includes cide's paragraph about the tracker, because a second turn of
        // this conversation is a second *process*: everything the first child was told has to be
        // told again, and a follow-up that quietly dropped the role's brief or the tracker rule
        // would behave differently from the turn before it for reasons nothing anywhere records.
        assert_eq!(
            config_of(&spawned.spec)["agent"]["developer"]["prompt"],
            config_of(&spawn(&plan).spec)["agent"]["developer"]["prompt"]
        );
        assert_eq!(
            config_of(&spawned.spec)["agent"]["developer"]["prompt"],
            json!(format!("{}\n\n{}", plan.agent.def.system_prompt, tracker()))
        );
    }

    /// The claude side of a respawn, asserted from here because this file owns the trait's
    /// respawn vocabulary. It used to refuse (`deliver` is `Stdin`, so a *follow-up* never
    /// respawns) — what changed is the snapshot: a run restored after a cide restart has no
    /// child to type into, and its continuation is `--resume` into the same conversation. The
    /// argv is the claim: resume the run's own session, mint nothing.
    #[test]
    fn a_claude_respawn_resumes_the_runs_own_session() {
        let mut agent = role();
        agent.def.harness = cide_ipc::Harness::Claude;
        let plan = plan_for(&agent);
        let spawn = crate::harness::ClaudeHarness
            .respawn_spec(&plan, &plan.session.to_string())
            .expect("a continuing claude child");

        let args = &spawn.spec.args;
        assert_eq!(value_of(args, "--resume"), plan.session.to_string());
        assert!(
            !args.iter().any(|a| a == "--session-id"),
            "a resume that also minted an id would be the 2.1.227 shape the CLI rejects: {args:?}"
        );
        assert!(
            spawn.opening.is_some(),
            "the continuation prompt is still typed into the restored TUI"
        );
    }

    /// Two refusals that are values rather than an `ENOENT` from three processes below.
    #[test]
    fn a_plan_this_harness_cannot_run_is_refused_with_a_reason() {
        let mut agent = role();
        agent.def.harness = cide_ipc::Harness::Claude;
        assert_eq!(
            OpencodeHarness.spawn_spec(&plan_for(&agent)).unwrap_err(),
            HarnessError::WrongHarness {
                plan: cide_ipc::Harness::Claude,
                harness: cide_ipc::Harness::Opencode,
            }
        );

        let agent = role();
        let mut plan = plan_for(&agent);
        plan.prompt = "   ".into();
        assert_eq!(
            OpencodeHarness.spawn_spec(&plan).unwrap_err(),
            HarnessError::NoPrompt
        );
    }

    /// A step's figures, and the arithmetic that says they were read the right way round. (M80)
    ///
    /// The fixture is a measured line, and its own `total` is the oracle: `5725` is
    /// `5696 + 4 + 25`, so opencode's `input` is the *uncached* prompt and its cache reads are
    /// counted beside it, not inside it. That is exactly the shape `TokenUsage` asks for, which
    /// is why nothing is subtracted here — codex's `exec` stream (until M93) counted its cache
    /// reads *inside* `input` and had to subtract them.
    #[test]
    fn a_step_finish_reports_what_it_spent() {
        let spent = usage(STEP_FINISH).expect("a step_finish carries its tokens");
        assert_eq!(spent.input, 5_696);
        assert_eq!(spent.output, 4);
        assert_eq!(spent.reasoning, 25);
        assert_eq!(spent.cache_read, 0);
        assert_eq!(spent.cache_write, 0);
        // The CLI's own `total` and cide's `context` agree while no cache is in play — which is
        // the claim that the two arithmetics are the same arithmetic.
        assert_eq!(spent.context(), 5_725);

        let tools = usage(STEP_FINISH_TOOLS).expect("a tool-calling step spends too");
        assert_eq!(tools.context(), 812);

        // A cached prompt is context the model read, so it counts — and it is still reported
        // apart, because it is the part that was cheap.
        let cached = STEP_FINISH.replace(
            r#""cache":{"read":0,"write":0}"#,
            r#""cache":{"read":4096,"write":128}"#,
        );
        let cached = usage(&cached).expect("still a step_finish");
        assert_eq!(cached.cache_read, 4_096);
        assert_eq!(cached.cache_write, 128);
        assert_eq!(
            cached.context(),
            5_725 + 4_096,
            "a cache write went to the provider, not into this conversation"
        );

        // Every other line the stream carries says nothing about a spend — including the
        // neighbouring step marker, and a `tool_use` whose own `state` has a `tokens`-shaped
        // nothing in it.
        for quiet in [STEP_START, TEXT, "not json at all", "{}"] {
            assert_eq!(usage(quiet), None, "{quiet}");
        }
        // The substring gate is a gate and not the answer: a line that merely mentions the
        // words is still parsed and still refused.
        assert_eq!(
            usage(r#"{"type":"text","part":{"text":"\"type\":\"step_finish\""}}"#),
            None
        );
    }

    /// The id is on the first line, which is what makes the capture a scan rather than a ladder.
    #[test]
    fn the_session_id_is_read_off_the_first_line_and_a_warning_is_survived() {
        assert_eq!(capture(STEP_START).as_deref(), Some(SESSION));
        assert_eq!(capture(STEP_FINISH).as_deref(), Some(SESSION));

        // A PTY turns every `\n` into `\r\n`, so a line split on `\n` still ends in a carriage
        // return. It must not be the difference between a resumable run and a lost one.
        assert_eq!(
            capture(&format!("{STEP_START}\r")).as_deref(),
            Some(SESSION)
        );

        // The CLI prints plain, ANSI-styled lines of its own — a rejected permission, an agent
        // name that did not resolve — into the same stream.
        assert_eq!(
            capture("\u{1b}[33m!\u{1b}[0m permission requested: bash (*); auto-rejecting"),
            None
        );
        assert_eq!(capture(""), None);
        assert_eq!(capture("{not json at all"), None);
        // JSON, but nothing that names a session.
        assert_eq!(
            capture(r#"{"type":"error","timestamp":1,"error":{"name":"x"}}"#),
            None
        );
        assert_eq!(capture(r#"{"type":"text","sessionID":""}"#), None);
    }

    /// The mapping the module header tabulates, over the real lines.
    #[test]
    fn the_stream_is_the_whole_state_machine() {
        let h = OpencodeHarness;
        let line = |current: RunState, text: &str| h.observe(current, Observation::Line(text));

        // A run leaves `Starting` on the first thing its child says.
        assert_eq!(
            line(RunState::Starting, STEP_START),
            Some(RunState::Running)
        );
        assert_eq!(line(RunState::Starting, TEXT), Some(RunState::Running));
        // …and a transition to the state it already holds is not a transition.
        assert_eq!(line(RunState::Running, TEXT), None);

        // Both halves of `step_finish` answer the same thing now. The final-looking one
        // (`reason: "stop"`) used to answer `Idle` — the misreading that released the slot,
        // admitted the queued run and got the working child of run 06202dd6 wound down
        // mid-task; see `observe`'s doc. No line ends the turn; the exit does.
        assert_eq!(
            line(RunState::Running, STEP_FINISH),
            None,
            "a step finishing is a run that is working — never a turn handed back"
        );
        assert_eq!(line(RunState::Running, STEP_FINISH_TOOLS), None);
        assert_eq!(
            line(RunState::Starting, STEP_FINISH),
            Some(RunState::Running),
            "…and from `Starting` it is evidence of life, exactly like any other event"
        );
        assert_eq!(
            line(RunState::Starting, STEP_FINISH_TOOLS),
            Some(RunState::Running),
            "an intermediate step is a run that is working, not one that has stopped"
        );

        // Nothing else in the stream says anything about the run.
        assert_eq!(line(RunState::Running, "some ANSI-styled warning"), None);
        assert_eq!(
            line(
                RunState::Running,
                r#"{"type":"error","timestamp":1,"sessionID":"ses_x","error":{"name":"ProviderAuthError"}}"#
            ),
            None,
            "the exit that follows carries the truthful record"
        );

        // No hook cide can install reaches this CLI.
        assert_eq!(
            h.observe(
                RunState::Running,
                Observation::Hook(&HookFrame::new("Stop", json!({})))
            ),
            None
        );
    }

    /// Only the reaper ends a run, and two states absorb everything else.
    #[test]
    fn a_frozen_or_finished_run_is_not_moved_by_a_late_line() {
        let h = OpencodeHarness;

        let paused = RunState::Paused {
            since_unix_ms: 1_700_000_000_000,
        };
        assert_eq!(
            h.observe(paused.clone(), Observation::Line(STEP_START)),
            None,
            "only a resume clears a freeze cide asserted"
        );

        let finished = RunState::Finished { code: 0 };
        assert_eq!(
            h.observe(finished.clone(), Observation::Line(STEP_FINISH)),
            None,
            "the last lines of a dead child's output are read after it is reaped"
        );

        // The exception, from any state: an exit is the only observation carrying ground truth.
        assert_eq!(
            h.observe(RunState::Idle, Observation::Exit(3)),
            Some(RunState::Finished { code: 3 })
        );
        assert_eq!(
            h.observe(paused, Observation::Exit(0)),
            Some(RunState::Finished { code: 0 })
        );
        assert_eq!(h.observe(finished, Observation::Exit(0)), None);
    }

    /// The document is one the **real** `opencode` accepts, and the role is registered under the
    /// name `--agent` will look for.
    ///
    /// `#[ignore]`d for this workspace's stated reason: it needs the binary on `PATH`. It needs
    /// nothing else — no account, no network turn, no quota — because `agent list` resolves the
    /// configuration and prints the roster without ever reaching a model. That makes it the
    /// cheapest possible check of the one claim this whole file rests on, and the one to run when
    /// a release moves the schema:
    ///
    /// ```sh
    /// cargo test -p cide-agents opencode -- --ignored
    /// ```
    /* == the model list ======================================================================= */

    #[test]
    fn the_model_filter_keeps_ids_and_drops_everything_else() {
        // The banner `opencode --help` prints today. `models` does not, and the whole point of a
        // probe is that "today" is not a guarantee: without this filter a release that grew one
        // would offer `█▀▀█ █▀▀█ …` as a selectable model, which is then written into a role file
        // and handed to `--model`.
        let stdout = "\
█▀▀█ █▀▀█ █▀▀█ █▀▀▄
opencode/big-pickle
lmstudio/openai/gpt-oss-20b

warning: a provider was skipped
unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M
notamodel
";
        assert_eq!(
            model_ids(&OPENCODE_CLI, stdout),
            vec![
                "opencode/big-pickle".to_string(),
                // A second slash and a colon are both real: these two ids are measured off a
                // working install, and a stricter pattern would silently drop them.
                "lmstudio/openai/gpt-oss-20b".to_string(),
                "unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M".to_string(),
            ],
            "ids survive; a banner, a blank, a warning sentence and a bare word do not"
        );
    }

    #[test]
    fn the_model_list_keeps_the_clis_order_and_drops_repeats() {
        // Order is the CLI's, which groups by provider. Sorting would scatter a provider's models
        // through the menu and put whichever id starts with `a` on the row a person picks without
        // reading.
        let stdout = "zed/one\nanthropic/two\nzed/one\nzed/three\n";
        assert_eq!(
            model_ids(&OPENCODE_CLI, stdout),
            vec![
                "zed/one".to_string(),
                "anthropic/two".to_string(),
                "zed/three".to_string()
            ]
        );
    }

    #[test]
    fn a_line_with_whitespace_after_the_slash_is_prose_and_not_a_model() {
        // The rule that does the work: opencode's ids never contain a space, and every sentence a
        // CLI prints does.
        assert!(!is_model_id("see https://opencode.ai/docs for the list"));
        assert!(!is_model_id("Models: anthropic/claude"));
        assert!(!is_model_id("/leading"));
        assert!(!is_model_id("trailing/"));
        assert!(is_model_id("anthropic/claude-sonnet-4-5"));
    }

    /// The list is real, and it is this machine's.
    ///
    /// `#[ignore]`d for the same reason as the test below: it needs the binary on `PATH`. It
    /// needs nothing else — `models` reads configuration and never reaches a model, so it costs
    /// no quota — which makes it the cheapest check that the flag is still spelled `models` and
    /// still prints one `provider/model` per line.
    ///
    /// ```sh
    /// cargo test -p cide-agents opencode -- --ignored
    /// ```
    #[test]
    #[ignore = "needs the real opencode binary on PATH"]
    fn the_real_opencode_lists_models() {
        let listed = OpencodeHarness
            .models(None, &cide_ipc::LlmSettings::default())
            .expect("`opencode models` should answer on a machine with the binary");
        assert!(
            !listed.is_empty(),
            "a configured opencode lists at least one model"
        );
        for id in &listed {
            assert!(
                is_model_id(id),
                "every line that survived the filter is an id: {id}"
            );
        }
    }

    #[test]
    #[ignore = "needs the real opencode binary on PATH"]
    fn the_real_opencode_registers_the_inline_role() {
        let mut agent = role();
        agent.def.system_prompt = "Quote \" backslash \\ newline\nand a second line.".into();
        let plan = plan_for(&agent);
        let spec = spawn(&plan).spec;
        let config = env_value(&spec, "OPENCODE_CONFIG_CONTENT").expect("a config");

        let out = std::process::Command::new("opencode")
            .args(["agent", "list"])
            .env("OPENCODE_CONFIG_CONTENT", config)
            .output()
            .expect("opencode on PATH");
        let listed = String::from_utf8_lossy(&out.stdout);
        assert!(
            listed.contains("developer (primary)"),
            "the inline role is not registered:\n{listed}"
        );
    }

    /// The printed document to the three kept fields, and every absence a `None` rather than a
    /// refusal. See `UserConfig`.
    #[test]
    fn the_resolved_configuration_yields_its_model_and_providers() {
        let printed = r#"{
  "$schema": "https://opencode.ai/config.json",
  "model": "deepseek/deepseek-flash",
  "small_model": "deepseek/deepseek-flash",
  "compaction": { "auto": true },
  "provider": { "deepseek": { "options": { "apiKey": "sk" } }, "vllm": { "npm": "@ai-sdk/openai-compatible" } },
  "keybinds": { "leader": "ctrl+x" }
}"#;
        let config = parse_user_config(printed).expect("json");
        assert_eq!(config.model.as_deref(), Some("deepseek/deepseek-flash"));
        assert_eq!(
            config.small_model.as_deref(),
            Some("deepseek/deepseek-flash")
        );
        assert!(config.provider.contains("vllm"), "{}", config.provider);

        // The same providers, another model: a different configuration. The same document with
        // a keybind changed: the same one — a restart is not worth a keybind.
        let moved = parse_user_config(&printed.replace("deepseek/deepseek-flash", "vllm/qwen"))
            .expect("json");
        assert_ne!(moved, config);
        let rebound = parse_user_config(&printed.replace("ctrl+x", "ctrl+a")).expect("json");
        assert_eq!(rebound, config);

        let bare =
            parse_user_config(r#"{ "model": "", "provider": "not an object" }"#).expect("json");
        assert_eq!(
            bare,
            UserConfig {
                model: None,
                small_model: None,
                provider: String::new()
            }
        );

        let refused = parse_user_config("opencode: no such command").expect_err("not json");
        assert!(refused.contains("not JSON"), "{refused}");
    }

    /// The installed binary answers `debug config` with a document the parser reads. Free: it
    /// reads files and prints, and never reaches a model.
    #[test]
    #[ignore = "runs the real opencode"]
    fn a_real_debug_config_is_readable() {
        let config =
            user_config(&OPENCODE_CLI, &std::env::temp_dir()).expect("opencode is installed");
        // `{:?}` and not the fields: the provider block carries keys, and the redaction is the
        // thing worth seeing here.
        eprintln!("{config:?}");
        assert!(
            !format!("{config:?}").contains("apiKey"),
            "a key reached a Debug rendering"
        );
    }

    /// The real harness on a finished run's conversation is the **TUI** on `--session`, not
    /// another `run`; and an id the run never reported is refused. (M42)
    #[test]
    fn continuing_a_conversation_opens_the_tui_on_its_session() {
        use cide_ipc::{Harness, HarnessSession};

        let spec = OpencodeHarness
            .continue_spec(&HarnessSession {
                harness: Harness::Opencode,
                id: "ses_0a1b2c".into(),
                cwd: std::path::PathBuf::from("/p/.cide/worktrees/qa-t-2"),
            })
            .expect("a captured id can be continued");
        assert_eq!(spec.program, "opencode");
        assert_eq!(
            spec.args,
            vec!["--session".to_string(), "ses_0a1b2c".to_string()]
        );
        assert!(
            !spec.args.iter().any(|a| a == "run"),
            "the TUI, not a headless turn: {:?}",
            spec.args
        );
        assert!(
            spec.resume.is_none(),
            "an opencode id is not a cide session"
        );

        let refused = OpencodeHarness
            .continue_spec(&HarnessSession {
                harness: Harness::Opencode,
                id: "   ".into(),
                cwd: std::path::PathBuf::from("/p"),
            })
            .expect_err("nothing to continue");
        assert!(
            matches!(refused, HarnessError::NotAConversation { .. }),
            "{refused}"
        );

        let wrong = OpencodeHarness
            .continue_spec(&HarnessSession {
                harness: Harness::Claude,
                id: cide_ipc::SessionId::new().to_string(),
                cwd: std::path::PathBuf::from("/p"),
            })
            .expect_err("wrong harness");
        assert!(
            matches!(wrong, HarnessError::WrongHarness { .. }),
            "{wrong}"
        );
    }
    // ==========================================================================================
    // The provider document (M45).
    // ==========================================================================================

    /// A settings value exercising all three provider kinds at once.
    fn providers() -> cide_ipc::LlmSettings {
        cide_ipc::LlmSettings {
            providers: vec![
                cide_ipc::LlmProvider::Catalog {
                    id: "openrouter".into(),
                    label: "OpenRouter".into(),
                    enabled: true,
                    api_key: "sk-or-test".into(),
                },
                // No key: the ordinary state for somebody whose shell already exports one.
                cide_ipc::LlmProvider::Catalog {
                    id: "deepseek".into(),
                    label: String::new(),
                    enabled: true,
                    api_key: String::new(),
                },
                cide_ipc::LlmProvider::Custom {
                    id: "ollama".into(),
                    label: "Ollama".into(),
                    enabled: true,
                    npm: "@ai-sdk/openai-compatible".into(),
                    base_url: "http://127.0.0.1:11434/v1".into(),
                    api_key: String::new(),
                    models: vec![
                        cide_ipc::LlmModel {
                            id: "qwen3:8b".into(),
                            label: "Qwen3 8B".into(),
                            context: 32768,
                            output: 4096,
                        },
                        // Half a pair, which must write no `limit` at all — see the emission.
                        cide_ipc::LlmModel {
                            id: "qwen3:32b".into(),
                            label: String::new(),
                            context: 32768,
                            output: 0,
                        },
                    ],
                },
                cide_ipc::LlmProvider::External {
                    id: "openai".into(),
                    label: "ChatGPT".into(),
                    setup: "add opencode-openai-codex-auth to your plugin array".into(),
                    expect: vec!["openai/gpt-5.2".into()],
                },
                cide_ipc::LlmProvider::Catalog {
                    id: "groq".into(),
                    label: String::new(),
                    enabled: false,
                    api_key: "sk-groq-test".into(),
                },
            ],
            pools: Vec::new(),
        }
    }

    /// The rule `cide_git::push::preview` states, in a new place: the menu must describe the runs
    /// that can happen. Two documents built independently would drift into a Model list naming ids
    /// no run can use, or a run reaching a provider the menu never offered.
    #[test]
    fn a_probe_and_a_run_carry_the_same_provider_block() {
        let llm = providers();
        let agent = role();
        let mut plan = plan_for(&agent);
        plan.llm = llm.clone();

        let run = config_of(&spawn(&plan).spec);
        let probe: Value = serde_json::from_str(
            &provider_config_content(&OPENCODE_CLI, &llm).expect("something to say"),
        )
        .expect("valid JSON");

        assert_eq!(run["provider"], probe["provider"]);
        assert!(run["provider"].is_object(), "{run}");
    }

    /// Blank is a key cide omits, never a value cide writes. The document is deep-merged *over*
    /// the user's own configuration with cide's last, so an `apiKey: ""` would blank a working key
    /// in a file cide never touched.
    #[test]
    fn a_catalog_provider_with_no_key_writes_no_block_at_all() {
        let members = provider_members(&providers());
        let block = &members["provider"];
        assert_eq!(
            block["openrouter"]["options"]["apiKey"],
            json!("sk-or-test")
        );
        assert!(
            block.get("deepseek").is_none(),
            "a catalogued provider with no key must contribute nothing, not an empty object: {block}"
        );
    }

    /// A disabled provider is not written, and an external one is never written at all — no
    /// block, no plugin line, no credential.
    #[test]
    fn a_disabled_or_external_provider_writes_nothing() {
        let members = provider_members(&providers());
        let block = &members["provider"];
        assert!(block.get("groq").is_none(), "disabled: {block}");
        assert!(block.get("openai").is_none(), "external: {block}");
    }

    /// Listing a plugin makes opencode install it from npm, which would turn pressing Dispatch
    /// into a package fetch. cide writes the key nowhere, ever.
    #[test]
    fn no_document_ever_carries_a_plugin_key() {
        let mut llm = providers();
        // Even with nothing but the plugin-backed provider configured.
        llm.providers
            .retain(|p| matches!(p, cide_ipc::LlmProvider::External { .. }));
        assert!(
            provider_members(&llm).is_empty(),
            "an external-only settings says nothing"
        );

        let document =
            provider_config_content(&OPENCODE_CLI, &providers()).expect("something to say");
        assert!(!document.contains("plugin"), "{document}");
    }

    /// A custom endpoint's models are what make `--model ollama/qwen3:8b` resolve, and a `0` limit
    /// is a claim about a context window rather than a number to write.
    #[test]
    fn a_custom_endpoint_declares_its_models_and_pairs_its_limits() {
        let members = provider_members(&providers());
        let ollama = &members["provider"]["ollama"];
        assert_eq!(ollama["npm"], json!("@ai-sdk/openai-compatible"));
        // opencode's own capitalisation, which its schema declares under
        // `additionalProperties: false` — `baseUrl` is not a typo that degrades.
        assert_eq!(
            ollama["options"]["baseURL"],
            json!("http://127.0.0.1:11434/v1")
        );
        assert!(
            ollama["options"].get("apiKey").is_none(),
            "blank key omitted: {ollama}"
        );
        let model = &ollama["models"]["qwen3:8b"];
        assert_eq!(model["name"], json!("Qwen3 8B"));
        assert_eq!(model["limit"], json!({ "context": 32768, "output": 4096 }));

        // The measured rule: opencode refuses a `limit` carrying one key and not the other, and a
        // refused document makes the run silently opencode's *default agent*. Half a pair writes
        // no `limit` at all — which the same probe confirms is accepted.
        let half = &ollama["models"]["qwen3:32b"];
        assert!(
            half.get("limit").is_none(),
            "half a limit pair writes none: {half}"
        );
        assert!(
            half.get("name").is_none(),
            "and a blank label writes no name either: {half}"
        );
    }

    /// `$defs.ProviderConfig` is `additionalProperties: false`, and the module header records what
    /// a rejected document costs: the run silently becomes opencode's *default agent*.
    #[test]
    fn the_document_carries_only_keys_the_schema_declares() {
        const DECLARED: &[&str] = &[
            "api",
            "name",
            "env",
            "id",
            "npm",
            "whitelist",
            "blacklist",
            "options",
            "models",
        ];
        let members = provider_members(&providers());
        for (id, entry) in members["provider"].as_object().expect("a map") {
            for key in entry.as_object().expect("a provider object").keys() {
                assert!(
                    DECLARED.contains(&key.as_str()),
                    "`{id}` carries `{key}`, which the schema does not declare"
                );
            }
        }
    }

    /// A role whose *file* says claude, redirected onto opencode by a local override, must be
    /// spawnable by this harness — the guard reads the resolved harness, not the definition.
    ///
    /// Without this the override layer would be inert in the most useful direction: the whole
    /// point is running a committed claude role on opencode here, and the definition's own
    /// `harness:` would have refused it.
    #[test]
    fn a_role_redirected_onto_this_harness_is_not_refused_by_it() {
        let mut agent = role();
        agent.def.harness = cide_ipc::Harness::Claude;
        let mut plan = plan_for(&agent);
        plan.harness = cide_ipc::Harness::Opencode;
        let spawn = OpencodeHarness
            .spawn_spec(&plan)
            .expect("a role redirected onto opencode spawns on opencode");
        assert_eq!(spawn.spec.program, "opencode");
    }

    /// And the guard still fires for a plan genuinely routed to the wrong implementation.
    #[test]
    fn a_plan_for_another_harness_is_still_refused() {
        let mut agent = role();
        agent.def.harness = cide_ipc::Harness::Opencode;
        let mut plan = plan_for(&agent);
        plan.harness = cide_ipc::Harness::Claude;
        assert!(
            matches!(
                OpencodeHarness.spawn_spec(&plan),
                Err(HarnessError::WrongHarness { .. })
            ),
            "the resolved harness is what decides, in both directions"
        );
    }

    /// The pool's candidate must reach **both** sites — the argv and the registered agent — or
    /// `opencode agent list` describes an agent on a model this run is not using.
    #[test]
    fn a_pool_candidate_outranks_the_definition_at_both_sites() {
        let mut agent = role();
        agent.def.model = Some("role/model".into());
        let mut plan = plan_for(&agent);
        plan.choice = Some(cide_ipc::PoolChoice {
            pool: "cheap-first".into(),
            index: 0,
            entry: cide_ipc::PoolEntry {
                provider: "openrouter".into(),
                model: "deepseek/deepseek-chat".into(),
                variant: String::new(),
                max_running: None,
            },
        });
        let spawn = spawn(&plan);
        assert_eq!(
            value_of(&spawn.spec.args, "--model"),
            "openrouter/deepseek/deepseek-chat"
        );
        assert_eq!(
            config_of(&spawn.spec)["agent"]["developer"]["model"],
            json!("openrouter/deepseek/deepseek-chat"),
            "the registered agent must name the same model the argv does"
        );
    }

    /// An entry's variant wins over the role's `effort`; an entry naming none leaves it standing.
    /// This is what stops a role-level `xhigh` being carried onto a candidate whose model does not
    /// offer it, turning a recoverable rate limit into a hard refusal.
    #[test]
    fn an_entrys_variant_beats_the_roles_effort_and_a_blank_one_does_not() {
        let mut agent = role();
        agent.effort = Some("role-effort".into());
        let mut plan = plan_for(&agent);
        let mut entry = cide_ipc::PoolEntry {
            provider: "openai".into(),
            model: "gpt-5.1-codex-mini".into(),
            variant: "medium".into(),
            max_running: None,
        };
        plan.choice = Some(cide_ipc::PoolChoice {
            pool: "p".into(),
            index: 0,
            entry: entry.clone(),
        });
        assert_eq!(value_of(&spawn(&plan).spec.args, "--variant"), "medium");

        entry.variant = String::new();
        plan.choice = Some(cide_ipc::PoolChoice {
            pool: "p".into(),
            index: 0,
            entry,
        });
        assert_eq!(
            value_of(&spawn(&plan).spec.args, "--variant"),
            "role-effort",
            "an entry that says nothing about the variant leaves the role's effort alone"
        );
    }

    /// No pool is exactly the behaviour that shipped before pools existed.
    #[test]
    fn no_candidate_is_the_definitions_own_model() {
        let mut agent = role();
        agent.def.model = Some("role/model".into());
        let plan = plan_for(&agent);
        assert!(plan.choice.is_none());
        assert_eq!(value_of(&spawn(&plan).spec.args, "--model"), "role/model");
    }

    /// The guard that lets `LlmSettings::cleaned` keep an incomplete row: a half-typed provider
    /// is stored, so the user can go on typing into it, and refused **here**, where the value is
    /// used. Dropping it at the storage end is what made every Add button on the Models screen
    /// inert.
    #[test]
    fn a_half_typed_row_is_stored_but_never_emitted() {
        let llm = cide_ipc::LlmSettings {
            providers: vec![
                // What "Add a known provider" produces, before a single keystroke.
                cide_ipc::LlmProvider::default(),
                cide_ipc::LlmProvider::Custom {
                    id: "ollama".into(),
                    label: String::new(),
                    enabled: true,
                    npm: String::new(),
                    base_url: "http://127.0.0.1:11434/v1".into(),
                    api_key: String::new(),
                    // And what "Add model" produces.
                    models: vec![cide_ipc::LlmModel::default()],
                },
            ],
            pools: Vec::new(),
        };
        let members = provider_members(&llm);
        let block = &members["provider"];
        assert_eq!(
            block.as_object().expect("a map").len(),
            1,
            "the id-less provider contributes nothing: {block}"
        );
        assert!(
            block["ollama"].get("models").is_none(),
            "and neither does its id-less model: {block}"
        );
    }

    // ==========================================================================================
    // The failover classifier (M45). The negative corpus is the specification.
    // ==========================================================================================

    /// Probed verbatim from opencode 1.18.29 with LM Studio deliberately not running. Captured,
    /// not composed — `opencode.rs`'s fixture rule, and it matters doubly here because the shape
    /// of a field cide branches on is the thing a release can change.
    const DEAD_ENDPOINT: &str = r#"{"type":"error","timestamp":1788774068053,"sessionID":"ses_f84c2c937ffeZqPyMKYHabMixA","error":{"name":"APIError","data":{"message":"Cannot connect to API: Unable to connect. Is the computer able to access the url?","isRetryable":true,"metadata":{"url":"http://127.0.0.1:1234/v1/chat/completions"}}}}"#;

    /// Probed verbatim: a model id that does not exist. Generic prose, no `isRetryable`, no
    /// status code — and a **typo**, which no other candidate in the pool will fix.
    const BOGUS_MODEL: &str = r#"{"type":"error","timestamp":1788773991554,"sessionID":"ses_f84c2cb0ffeCG22QWVObxw4b4","error":{"name":"UnknownError","data":{"message":"Unexpected server error. Check server logs for details.","ref":"err_c19594b9"}}}"#;

    #[test]
    fn a_dead_endpoint_is_unreachable_on_opencodes_own_verdict() {
        // No status code at all: `isRetryable` is the only signal, which is why it is read.
        assert_eq!(failover(DEAD_ENDPOINT), Some(FailoverReason::Unreachable));
    }

    #[test]
    fn a_status_code_answers_outright() {
        let with = |code: u64| {
            format!(
                r#"{{"type":"error","error":{{"name":"APIError","data":{{"message":"m","statusCode":{code},"isRetryable":false}}}}}}"#
            )
        };
        assert_eq!(failover(&with(429)), Some(FailoverReason::RateLimited));
        assert_eq!(failover(&with(402)), Some(FailoverReason::RateLimited));
        assert_eq!(failover(&with(401)), Some(FailoverReason::Auth));
        assert_eq!(failover(&with(403)), Some(FailoverReason::Auth));
        // A 5xx with no retryable flag still reads as unreachable only if the CLI says so.
        assert_eq!(failover(&with(503)), None);
    }

    #[test]
    fn opencodes_own_auth_error_is_auth() {
        let line = r#"{"type":"error","timestamp":1,"sessionID":"ses_x","error":{"name":"ProviderAuthError","data":{"providerID":"openrouter","message":"bad key"}}}"#;
        assert_eq!(failover(line), Some(FailoverReason::Auth));
    }

    /// **The specification.** A typo must fail loudly once, not burn every candidate in the pool
    /// one turn at a time and report the last one's error as the cause of death.
    #[test]
    fn a_typod_model_id_never_fails_over() {
        assert_eq!(failover(BOGUS_MODEL), None);
    }

    /// The status code is the more specific fact and must beat `isRetryable`. A 404 is a statement
    /// about the request; no other candidate repairs it.
    #[test]
    fn an_explicit_four_hundred_beats_a_retryable_flag() {
        let line = r#"{"type":"error","error":{"name":"APIError","data":{"message":"m","statusCode":404,"isRetryable":true}}}"#;
        assert_eq!(
            failover(line),
            None,
            "a 404 is about the request, however retryable the CLI calls it"
        );
    }

    /// Refused by name, so the refusal is a decision a reader can find rather than a fall-through.
    #[test]
    fn a_turns_own_failures_are_not_the_providers_fault() {
        for name in [
            "MessageOutputLengthError",
            "MessageAbortedError",
            "UnknownError",
        ] {
            let line = format!(
                r#"{{"type":"error","error":{{"name":"{name}","data":{{"message":"m"}}}}}}"#
            );
            assert_eq!(failover(&line), None, "{name}");
        }
    }

    /// A tool failing is not a provider failing. Swapping the model because a `bash` call was
    /// refused would be a spectacular non-sequitur.
    #[test]
    fn a_failed_tool_call_is_not_a_provider_failure() {
        let rejected = r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"bash","state":{"status":"error","input":{"command":"ls"},"error":"The user rejected permission to use this specific tool call."}}}"#;
        assert_eq!(failover(rejected), None);
        for line in [STEP_START, TEXT, STEP_FINISH] {
            assert_eq!(failover(line), None);
        }
    }

    /// Everything that is not a line at all, and the cheap guard that keeps this off the hot path.
    #[test]
    fn prose_and_rubbish_say_nothing() {
        for line in [
            "",
            "{",
            "not json",
            "! permission requested: bash (*)",
            "{}",
        ] {
            assert_eq!(failover(line), None, "{line:?}");
        }
    }

    /// A release that adds a field must widen nothing here; one that adds a *name* falls through
    /// to `None`, which is the safe direction.
    #[test]
    fn an_unknown_shape_is_read_defensively() {
        let future = r#"{"type":"error","error":{"name":"APIError","data":{"message":"m","statusCode":429,"isRetryable":true,"futureField":{"x":[1,2]}}}}"#;
        assert_eq!(failover(future), Some(FailoverReason::RateLimited));
        let unknown_name =
            r#"{"type":"error","error":{"name":"SomethingNew2027","data":{"message":"m"}}}"#;
        assert_eq!(failover(unknown_name), None);
    }

    /// The harness answers through the trait, and every other harness answers nothing.
    #[test]
    fn only_this_harness_diagnoses_its_own_stream() {
        assert_eq!(
            OpencodeHarness.diagnose(DEAD_ENDPOINT),
            Some(FailoverReason::Unreachable)
        );
        for harness in crate::harness::registry() {
            // mimo is the same stream, so it reads it too. (M81)
            if !harness.kind().reads_provider_document() {
                assert_eq!(
                    harness.diagnose(DEAD_ENDPOINT),
                    None,
                    "{:?} must not read another CLI's stream",
                    harness.kind()
                );
            }
        }
    }

    // The verdict half of `test_model`, over captured output. The child half needs a real
    // `opencode` and a real model, which is `tests/real_opencode.rs`'s job.

    #[test]
    fn a_turn_that_produced_text_is_a_working_model() {
        assert!(
            verdict(
                &OPENCODE_CLI,
                &format!("{STEP_START}\n{TEXT}\n{STEP_FINISH}"),
                true,
                ""
            )
            .is_ok()
        );
    }

    #[test]
    fn a_provider_error_is_reported_with_its_own_words() {
        // The probed shape: a dead local endpoint, exit 1, one error line.
        let line = r#"{"type":"error","timestamp":1,"sessionID":"ses_x","error":{"name":"APIError","data":{"message":"Cannot connect to API: Unable to connect.","isRetryable":true,"metadata":{"url":"http://127.0.0.1:1234/v1"}}}}"#;
        let answer = verdict(&OPENCODE_CLI, line, false, "")
            .expect_err("a dead endpoint is not a working model");
        assert!(answer.contains("APIError"), "{answer}");
        assert!(answer.contains("Cannot connect"), "{answer}");
    }

    /// An error line the *stream recovered from* must not fail the test: what matters is whether
    /// the model ended up answering, which is why `answered` is cleared by an error and set again
    /// by the text that follows it.
    #[test]
    fn an_error_the_turn_recovered_from_is_not_a_failure() {
        let recovered = format!(
            "{}\n{TEXT}",
            r#"{"type":"error","timestamp":1,"error":{"name":"APIError","data":{"message":"flaky","isRetryable":true}}}"#
        );
        assert!(
            verdict(&OPENCODE_CLI, &recovered, true, "").is_ok(),
            "text after an error is still an answer"
        );
    }

    /// Exit 0 with nothing usable is not a working model, and says so rather than passing.
    #[test]
    fn a_silent_success_is_not_a_pass() {
        let answer =
            verdict(&OPENCODE_CLI, STEP_FINISH, true, "").expect_err("no text is no answer");
        assert!(answer.contains("produced nothing"), "{answer}");
    }

    /// A child that died before printing JSON falls back to the first useful line of stderr.
    #[test]
    fn a_child_that_printed_nothing_useful_quotes_its_stderr() {
        let answer =
            verdict(&OPENCODE_CLI, "", false, "\n  boom: no such model\n").expect_err("a failure");
        assert!(answer.contains("boom: no such model"), "{answer}");
    }

    /// Parsed defensively: the live error object carries fields the published type does not
    /// declare, and a release that adds one must not turn this into "an unreadable error".
    #[test]
    fn an_error_carrying_unknown_fields_still_reads() {
        let line = r#"{"type":"error","error":{"name":"UnknownError","data":{"message":"nope","ref":"err_1","futureField":{"x":1}}}}"#;
        let answer = verdict(&OPENCODE_CLI, line, false, "").expect_err("a failure");
        assert!(answer.contains("UnknownError: nope"), "{answer}");
    }

    /// An installation that has never opened the Models screen probes exactly as it did before
    /// any of this existed.
    #[test]
    fn an_empty_settings_says_nothing_at_all() {
        let llm = cide_ipc::LlmSettings::default();
        assert!(provider_members(&llm).is_empty());
        assert!(provider_config_content(&OPENCODE_CLI, &llm).is_none());
    }

    // ==========================================================================================
    // MiMo Code, the second flavour (M81).
    // ==========================================================================================

    /// The role fixture, pointed at mimo.
    fn mimo_role() -> LoadedAgent {
        let mut agent = role();
        agent.def.harness = cide_ipc::Harness::Mimo;
        agent
    }

    /// The same invocation under mimo's name, with mimo's two differences and nothing of
    /// opencode's left in it. Every `OPENCODE_*` name is one the fork never reads, so a leaked
    /// one is a configuration nobody sees and a run on mimo's *default* agent — the failure the
    /// whole flavour exists to make impossible.
    #[test]
    fn a_mimo_run_is_opencodes_invocation_under_mimos_names() {
        let mut agent = mimo_role();
        agent.def.model = Some("xiaomi/mimo-v2.6-flash".into());
        let mut plan = plan_for(&agent);
        plan.unattended = Unattended::Auto;
        let spawned = MimoHarness.spawn_spec(&plan).expect("a spawnable plan");
        let spec = &spawned.spec;
        let args = &spec.args;

        assert_eq!(spec.program, "mimo");
        assert_eq!(args[0], "run", "{args:?}");
        assert_eq!(value_of(args, "--agent"), "developer");
        assert_eq!(value_of(args, "--model"), "xiaomi/mimo-v2.6-flash");
        assert_eq!(value_of(args, "--format"), "json");
        assert!(args.iter().any(|arg| arg == "--thinking"), "{args:?}");
        // mimo's `run` has no `--auto`; this is its spelling of the same permission.
        assert!(
            args.iter()
                .any(|arg| arg == "--dangerously-skip-permissions"),
            "{args:?}"
        );
        assert!(!args.iter().any(|arg| arg == "--auto"), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some(&*plan.prompt));

        let document = env_value(spec, "MIMOCODE_CONFIG_CONTENT").expect("mimo's config");
        let config: Value = serde_json::from_str(document).expect("json");
        assert_eq!(config["$schema"], json!(MIMO_CLI.schema));
        assert_eq!(config["agent"]["developer"]["mode"], json!("primary"));
        // The tracker is reached under opencode's spelling, which mimo shares (measured).
        assert!(
            config["agent"]["developer"]["prompt"]
                .as_str()
                .is_some_and(|prompt| prompt.contains("cide_cide_task_get")),
            "{config}"
        );
        assert!(config["mcp"][SERVER].is_object(), "{config}");
        assert!(
            !spec.env.iter().any(|(key, _)| key.starts_with("OPENCODE_")),
            "{:?}",
            spec.env
        );
        assert!(matches!(spawned.binding, SessionBinding::Harness { .. }));

        let resumed = MimoHarness
            .respawn_spec(&plan, "ses_ffe5f35a09c2bffetJ1TxJm6qO")
            .expect("a follow-up");
        assert_eq!(
            value_of(&resumed.spec.args, "--session"),
            "ses_ffe5f35a09c2bffetJ1TxJm6qO"
        );
    }

    /// A flavour refuses the other flavour's plan, exactly as it refuses a claude one: the
    /// resolved harness decides the child, and forking opencode for a mimo plan would run a
    /// role on a CLI the user did not choose, reading it with the right machine by accident.
    #[test]
    fn each_flavour_refuses_the_others_plan() {
        let mimo = mimo_role();
        let refused = OpencodeHarness
            .spawn_spec(&plan_for(&mimo))
            .expect_err("a mimo plan");
        assert!(
            matches!(refused, HarnessError::WrongHarness { .. }),
            "{refused}"
        );

        let opencode = role();
        let refused = MimoHarness
            .spawn_spec(&plan_for(&opencode))
            .expect_err("an opencode plan");
        assert!(
            matches!(refused, HarnessError::WrongHarness { .. }),
            "{refused}"
        );
    }

    /// The TUI a finished mimo run re-opens in: `mimo --trust --session <id>`, and an opencode
    /// conversation is not one it will open — the ids share a shape and live in two stores.
    #[test]
    fn a_mimo_conversation_reopens_in_mimos_tui_and_only_there() {
        use cide_ipc::{Harness, HarnessSession};
        let conversation = |harness| HarnessSession {
            harness,
            id: "ses_ffe5f35a09c2bffetJ1TxJm6qO".into(),
            cwd: PathBuf::from("/p/.cide/worktrees/qa-t-2"),
        };

        let spec = MimoHarness
            .continue_spec(&conversation(Harness::Mimo))
            .expect("a captured id");
        assert_eq!(spec.program, "mimo");
        assert_eq!(
            spec.args,
            vec!["--trust", "--session", "ses_ffe5f35a09c2bffetJ1TxJm6qO"]
        );

        assert!(matches!(
            MimoHarness.continue_spec(&conversation(Harness::Opencode)),
            Err(HarnessError::WrongHarness { .. })
        ));
        assert!(matches!(
            OpencodeHarness.continue_spec(&conversation(Harness::Mimo)),
            Err(HarnessError::WrongHarness { .. })
        ));
    }

    /// `mimo models` annotates each id, and the annotation is cut at its exact separator — for
    /// mimo only, so opencode's filter keeps refusing prose. Captured from 0.1.15.
    #[test]
    fn mimos_annotated_model_list_reads_as_ids() {
        let stdout = "mimo/mimo-auto — window 1M, compacts at 900K\n\
                      xiaomi/mimo-v2.5 — window 1.05M, compacts at 944K\n\
                      xiaomi/mimo-v2.6-flash — window 1.05M, compacts at 944K\n";
        assert_eq!(
            model_ids(&MIMO_CLI, stdout),
            vec![
                "mimo/mimo-auto".to_string(),
                "xiaomi/mimo-v2.5".to_string(),
                "xiaomi/mimo-v2.6-flash".to_string()
            ]
        );
        assert!(
            model_ids(&OPENCODE_CLI, stdout).is_empty(),
            "an annotated line is prose to opencode's reader"
        );
    }

    /// Captured from a real `mimo run --format json` turn (0.1.15, one tool call): the state
    /// machine, the session capture and the usage reader are opencode's and read it unchanged.
    #[test]
    fn a_real_mimo_stream_reads_like_opencodes() {
        const START: &str = r#"{"type":"step_start","timestamp":1790101651585,"sessionID":"ses_ffe5f35a09c2bffetJ1TxJm6qO","part":{"id":"prt_g001a0ca5f8080001Yl58gOb5v","messageID":"msg_g001a0ca5f650a001GT4L53JNF","sessionID":"ses_ffe5f35a09c2bffetJ1TxJm6qO","snapshot":"0472df0c57846f06e707d0819422f438f93347c0","type":"step-start"}}"#;
        const TOOL: &str = r#"{"type":"tool_use","timestamp":1790101656834,"sessionID":"ses_ffe5f35a09c2bffetJ1TxJm6qO","part":{"type":"tool","tool":"bash","callID":"call_3fc8c31ab7a84355bfa9482b","state":{"status":"completed","input":{"command":"echo hi","description":"Runs echo hi once"},"output":"hi\n","metadata":{"output":"hi\n","exit":0,"description":"Runs echo hi once","truncated":false},"title":"Runs echo hi once","time":{"start":1790101656805,"end":1790101656810}},"id":"prt_g001a0ca5f89e0001aoRF39sLw","sessionID":"ses_ffe5f35a09c2bffetJ1TxJm6qO","messageID":"msg_g001a0ca5f650a001GT4L53JNF"}}"#;
        const FINISH: &str = r#"{"type":"step_finish","timestamp":1790101662111,"sessionID":"ses_ffe5f35a09c2bffetJ1TxJm6qO","part":{"id":"prt_g001a0ca5fa992001AYU5VSe9G","reason":"stop","snapshot":"937c9e99612dd46d60d8e0b13769f67e33669dc2","messageID":"msg_g001a0ca5f9527001yLFIke6ng","sessionID":"ses_ffe5f35a09c2bffetJ1TxJm6qO","type":"step-finish","tokens":{"total":30391,"input":30370,"output":3,"reasoning":18,"cache":{"write":0,"read":0}},"cost":0.00425768}}"#;

        assert_eq!(
            capture(START).as_deref(),
            Some("ses_ffe5f35a09c2bffetJ1TxJm6qO")
        );
        assert_eq!(
            MimoHarness.observe(RunState::Starting, Observation::Line(START)),
            Some(RunState::Running)
        );
        assert!(keep_event(TOOL));
        let usage = MimoHarness.usage(FINISH).expect("a step's spend");
        assert_eq!(usage.output, 3);
        assert!(verdict(&MIMO_CLI, &format!("{START}\n{TOOL}\n{FINISH}"), true, "").is_ok());
    }

    /// Under a pool, a mimo child is told to give up on a dead candidate in seconds rather than
    /// the fifteen silent minutes its defaults allow; without one nothing is said, and opencode
    /// is never told anything, since it gives up by itself. See `pool_retry`.
    #[test]
    fn only_a_mimo_run_on_a_pool_bounds_its_retries() {
        let choice = cide_ipc::PoolChoice {
            pool: "cheap-first".into(),
            index: 0,
            entry: cide_ipc::PoolEntry {
                provider: "xiaomi".into(),
                model: "mimo-v2.6-flash".into(),
                variant: String::new(),
                max_running: None,
            },
        };
        let mimo_config = |choice: Option<cide_ipc::PoolChoice>| {
            let agent = mimo_role();
            let mut plan = plan_for(&agent);
            plan.choice = choice;
            let spawned = MimoHarness.spawn_spec(&plan).expect("spawnable");
            serde_json::from_str::<Value>(
                env_value(&spawned.spec, "MIMOCODE_CONFIG_CONTENT").expect("a config"),
            )
            .expect("json")
        };

        let pooled = mimo_config(Some(choice.clone()));
        for class in ["network", "server", "rateLimit", "unknown"] {
            assert_eq!(pooled["retry"][class]["mode"], json!("bounded"), "{pooled}");
        }
        assert!(mimo_config(None).get("retry").is_none());

        let agent = role();
        let mut plan = plan_for(&agent);
        plan.choice = Some(choice);
        assert!(config_of(&spawn(&plan).spec).get("retry").is_none());
        assert!(MimoHarness.failure_exits_zero());
        assert!(!OpencodeHarness.failure_exits_zero());
    }
}
