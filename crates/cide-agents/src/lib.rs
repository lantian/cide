//! Subagent orchestration: what roles a project has, and whether it may run them. (M18)
//!
//! # The two questions this crate answers, and why they are one crate
//!
//! *What roles does this project have?* — [`defs`], which reads `.cide/agents/<name>.md` and its
//! global twin beside `keymap.json`, merges them, and validates the result into a list the
//! Agents panel can draw.
//!
//! *May it run them?* — [`config`], which reads `.cide/config.json`, the committed,
//! per-project switch that is **off in the absence of the file**.
//!
//! They are one crate because neither is answerable alone. A role is dispatchable only if its
//! own definition is sound *and* the project's config permits it — see [`dispatch_refusal`],
//! which is the only place both halves meet and the only function a dispatch site should have to
//! call. Splitting them would put that conjunction in the caller, where it would be reimplemented
//! once per call site and get the `bypassPermissions` case wrong in at least one of them.
//!
//! # What is deliberately not here
//!
//! Spawning. [`harness`] *describes* a run's child — a `SpawnSpec`, byte for byte the type
//! `cmd/session.rs` builds for a pane — and starts nothing; the registry insert, the exit
//! watcher and the PTY itself belong to `cide-app`, which is the only crate allowed to hold them.
//! So nothing in this crate starts a process, and that is worth keeping true: everything here is
//! a pure function of its arguments and the filesystem, which is what lets these tests build
//! three directories in `/tmp`, and `harness`'s assert on a real argv, with no `claude` on
//! `PATH` and no repository anywhere.
//!
//! # No state, no cache
//!
//! Every function here reads the disk when it is called. `.cide/*` is committed and a teammate's
//! commit or a `git checkout` can change it under the running app, so a cached roster is a
//! roster that is wrong for as long as nobody happens to invalidate it. `cide_fs::filter`
//! already watches `<root>/.cide`, so a change arrives as an ordinary `cide://fs-changed` and
//! the answer is simply read again — which costs a `read_dir` and a handful of small files.

/// The assignment-starts-work policy: which task mutations dispatch which roles. Pure — the
/// registry facts (live runs, slots) are checked by the caller, in `cide-app`.
pub mod autodispatch;
pub mod config;
pub mod defs;
/// `@role` mentions in task prose — the pure scanner; who acts on one is `cide-app`'s question.
pub mod mentions;
pub mod overrides;
// No `///` summary here, deliberately: `harness.rs`'s own `//!` header is the summary, and an
// outer doc comment on the `mod` item merges with it into one fragment that rustdoc then resolves
// in *this* module's scope — so every `[`RunState`]` and `[`SpawnSpec`]` link inside that header
// breaks. `pub mod tools;` below is the same shape and carries the same warning.
pub mod harness;
/// The `cide_task_*` MCP vocabulary. Pure: names, schemas and handlers over a [`tools::TaskSink`],
/// with no socket and no filesystem anywhere in it — see the module header.
pub mod tools;

// The prediction the paragraph above made — "a `LoadedAgent` is already everything a spawn
// needs" — held: [`harness`] takes one by reference and adds only the facts a *run* has that a
// role does not (a session, a worktree, the sockets). Nothing in `defs` moved to make room for it.
//
// The second implementation has landed, and it cost what the trait was chosen to make it cost:
// one `static` and one `registry` entry. What it did *not* fit was `Delivery` — `opencode run` is
// one turn per process, so a follow-up is a respawn rather than a write to stdin, which is why
// that is an enum and not a `write()` method. A trait that had assumed stdin would have had to be
// reopened here instead of extended.

pub use config::{AgentsConfig, CideConfig, Isolation};
pub use defs::{AgentProblem, Catalog, LoadedAgent, Severity};
pub use harness::{
    ClaudeHarness, CodexHarness, ContinueSpec, Delivery, ERASE_MARKER, FailoverReason, Harness,
    HarnessError, HarnessSpawn, Observation, OpencodeHarness, QwenHarness, RenderState, RunPlan,
    SessionBinding, for_kind, registry,
};
pub use tools::{Content, TaskSink, ToolResult, descriptors, dispatch};

use std::path::Path;

use cide_ipc::AgentId;

/// Everything cide knows about one project's subagents, read fresh.
#[derive(Debug, Clone, Default)]
pub struct ProjectAgents {
    pub config: CideConfig,
    pub catalog: Catalog,
}

impl ProjectAgents {
    /// Whether this project may dispatch anything at all.
    pub fn enabled(&self) -> bool {
        self.config.agents.enabled
    }

    pub fn get(&self, id: &AgentId) -> Option<&LoadedAgent> {
        self.catalog.get(id)
    }
}

/// Read `.cide/config.json` and every role definition that applies to this project.
///
/// **Definitions are read even when orchestration is disabled**, which is the one thing here that
/// looks wasteful and is not. The `Disabled` roster is a screen the user is looking at *in order
/// to decide whether to enable this*, and it can only be honest about what enabling would give
/// them if the roles have been read. Nothing is spawned on the strength of a definition — only on
/// the strength of [`AgentsConfig::enabled`] — so reading them early costs a `read_dir` and gives
/// up nothing.
pub fn load_project(project_root: &Path) -> ProjectAgents {
    let config = config::load(project_root);
    let catalog = defs::load(project_root, config.agents.harness);
    ProjectAgents { config, catalog }
}

/// Why this role cannot be dispatched right now, as a sentence — or `None` when it can.
///
/// # The one function a dispatch site calls, and why it is not three
///
/// Three separate facts can stop a run, they come from two different files, and a call site that
/// checked two of them would be a call site that spawns a `bypassPermissions` agent in a
/// repository that never authorised one. Folding them into a single `Option<String>` makes the
/// safe path the short one: `if let Some(why) = dispatch_refusal(..) { return Err(why) }`.
///
/// The order is the order a user can act on. "Subagents are off for this project" first, because
/// it makes every other answer moot. Then the broken install, which applies identically to every
/// role and which the user cannot fix from the panel. Then the role's own
/// [`cide_ipc::AgentDef::unavailable`], which the panel is already drawing. Then the
/// dangerous-permissions gate, which is last because it is the only one that is *about this
/// dispatch* rather than about the state of the world.
///
/// # The bridge, and why a missing one refuses instead of degrading
///
/// `bridge` is the absolute path to `cide-hook`, or `None` when it is not beside the running
/// executable. It is passed in rather than looked up because only `cide-app` may read
/// `current_exe`; this crate must not learn about packaging.
///
/// `None` used to be a *degrade*: `RunPlan::hook_bin` gates both the MCP server block and the
/// tracker paragraph, so the run simply started with neither, the dispatch reported success, and
/// nothing was logged. A terrastrike audit caught ten consecutive opencode runs in that state —
/// each one spent ten minutes reading cide's source and writing itself a stdio MCP client in
/// `/tmp` so it could file its report, and one of them, hunting for the tools, started a second
/// cide and then `pkill`ed it.
///
/// A run that cannot reach the tracker cannot report, and `TRACKER_PREAMBLE`'s own header calls
/// that the failure the tracker exists to prevent. So it is a refusal, and it is here rather than
/// at the spawn site so that the panel greys every role with the sentence and `cide_agents_list`
/// prints it, instead of offering a button that fails silently three processes down.
pub fn dispatch_refusal(
    agent: &LoadedAgent,
    config: &AgentsConfig,
    bridge: Option<&std::path::Path>,
) -> Option<String> {
    if !config.enabled {
        return Some(format!(
            "Subagents are off for this project. Turn them on in the Agents panel, which writes \
             `{}/config.json`.",
            config::CIDE_DIR
        ));
    }
    if bridge.is_none() {
        return Some(
            "cide cannot find its `cide-hook` binary beside the running executable, so a run \
             would start with no connection to the task tracker: no `cide_task_*` tools, and no \
             way to comment what it did. Rebuild it with `cargo build -p cide-hook`, or launch \
             through `./run.sh`, which builds both."
                .to_string(),
        );
    }
    if let Some(reason) = &agent.def.unavailable {
        return Some(reason.clone());
    }
    // `skip_permissions` passes this gate too, and that is coherence rather than a hole: while
    // the project's own default sends every unattended child out promptless, a role that wrote
    // the same stance down cannot be the one thing refused for it. The two-acts rule bites only
    // where it means something — a project that switched skipping off. See the field's doc.
    if is_dangerous(agent) && !config.allow_dangerous_permissions && !config.skip_permissions {
        return Some(format!(
            "`{}` asks for `permission-mode: {}`, which lets it edit, delete and run anything \
             without asking. This project has switched `agents.skipPermissions` off and has not \
             authorised this role either: set `agents.allowDangerousPermissions` to true in \
             `{}/config.json` if you mean it.",
            agent.def.id,
            defs::BYPASS_PERMISSIONS,
            config::CIDE_DIR
        ));
    }
    None
}

/// Whether this role asks to skip every permission prompt.
///
/// # Why this is checked at dispatch and not at load
///
/// Refusing it at load would make the role **vanish** from the roster — and a user who cannot
/// find the agent they just wrote has nothing to search for, no row to read a sentence off, and
/// no reason to suspect a config file they have never opened. Refusing it at dispatch puts the
/// whole explanation in front of them at the moment they asked for the thing, and names the key
/// that fixes it. That is the same argument `cide_ipc::AgentDef::unavailable` makes about
/// uninstalled harnesses, one step further along: *load everything, refuse late, always say why*.
///
/// It is checked at dispatch rather than only in the panel for the reason `claude_cli`'s module
/// header gives about its own refusals: the panel's job is to make the refusal legible, and the
/// dispatch's job is to be true. `.cide/config.json` is hand-editable and an `invoke` is one
/// call away from being made by something other than the panel.
pub fn is_dangerous(agent: &LoadedAgent) -> bool {
    agent.permission_mode.as_deref() == Some(defs::BYPASS_PERMISSIONS)
}

/// How many runs of this role may be live at once: its own `max-concurrent`, floored at 1.
///
/// This used to clamp to 1 under worktree isolation — one worktree per *agent* meant a second
/// concurrent run of the role would be a second process editing one checkout — and the clamp
/// came with a `concurrency_note` sentence so it was never silent. Both are gone, because the
/// premise changed: a run with a task now takes a worktree **per task** ([`checkout_name`]),
/// so two tasks of one role are two checkouts and the collision the clamp guarded against no
/// longer exists between them. What still cannot run twice is two children in *one* checkout —
/// the same (role, task) pair — and that is enforced where it is now a per-checkout fact rather
/// than a per-role number: the registry's admission holds a run whose checkout is occupied, in
/// the queue, until it is not. A dispatch with no task takes no checkout at all
/// ([`run_checkout`], M40) and is bounded by this number and the project's alone.
pub fn effective_max_concurrent(agent: &LoadedAgent, _config: &AgentsConfig) -> u16 {
    agent.def.max_concurrent.max(1)
}

/// The worktree a (role, task) pair stands in, as the name `cide_git::worktree` builds a path
/// and branch from.
///
/// `<role>-<task-slug>` for a run pointed at a task — so a role's tasks parallelise in separate
/// checkouts (each on branch `cide/<name>`), and a *re*-dispatch of the same task lands in the
/// checkout holding that task's earlier work, which the first live workstream's debug notes
/// called the one reliable recovery channel. The bare `<role>` for `None` is **not a checkout
/// any dispatch takes any more** (M40): a run with no task stands in the project root, decided
/// by [`run_checkout`], which is the function a dispatch site calls. The `None` arm survives for
/// `cide_agent_integrate` without a task, which names `cide/<role>` — the base branch task-less
/// runs committed to before M40, and the only way work left there is still reachable.
///
/// The slug is the task id forced through the worktree grammar (`[a-z0-9-]`, lowercased,
/// anything else becomes `-`), because `.cide/tasks.json` is hand-editable and this string
/// becomes a directory name and a ref name. A task id that sanitises to nothing falls back to
/// the bare role name rather than minting a name from thin air. The mapping must stay
/// **deterministic** — a resumed run recomputes its checkout from the same (role, task) pair
/// and has to arrive at the directory its transcript lives under.
///
/// The separator is `-`, which a role name may also contain, so the composite is not parseable
/// back into its halves — nothing parses it, the registry always computes forward from the
/// pair. The collision that ambiguity permits (a role literally named `developer-t-7` beside a
/// role `developer` with task `t-7`) degrades to two runs sharing a checkout — the pre-feature
/// status quo — not to corruption, and admission still serialises them.
pub fn checkout_name(agent: &AgentId, task: Option<&cide_ipc::TaskId>) -> String {
    let base = agent.0.as_str();
    let Some(task) = task else {
        return base.to_string();
    };
    let slug: String = task
        .0
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return base.to_string();
    }
    // The worktree grammar caps a name at 64; a role may already spend 32. Truncating the slug
    // keeps determinism (same input, same cut) at the cost of a theoretical collision between
    // two very long task ids — which degrades to a shared checkout, as above.
    let room = 64 - base.len() - 1;
    let slug: String = slug.chars().take(room).collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return base.to_string();
    }
    format!("{base}-{slug}")
}

/// The checkout a dispatch is stamped with — and the directory its fork lands in — or `None` for
/// a run that stands in the project root. (M40)
///
/// # The one rule, in one place
///
/// Two sites decide where a run stands, and they must agree: `cmd::agents::plan_dispatch` stamps
/// the name the admission gate serialises on, and `agents::start_child` recomputes it at the fork
/// to `ensure` the directory. They used to each carry the same three-arm `match`, which is one
/// edit away from a run gated on a directory it does not take — or taking one it was never gated
/// on, which is two children in one checkout with nothing arbitrating. Now both call this.
///
/// # Why a run with no task has no checkout
///
/// A task-less dispatch is the orchestrator asking a role to *check something*, run the tests, or
/// do a small piece of work directly — the cases the user named when asking for the road at all.
/// A worktree is the wrong home for those twice over: the result of a check is a sentence in the
/// run's pane and nothing to merge; the result of a small direct edit wants to be *in the tree the
/// user is looking at*, not on a `cide/<role>` branch somebody must remember to integrate. Before
/// M40 such a run took the role's base worktree and serialised there; nothing ever dispatched one
/// (the panel always names a task), so no work is stranded by the change.
///
/// `Shared` isolation and a role's own `worktree: false` put every run in the root exactly as
/// before — the `Some` arm is the *intersection* of "the project isolates", "this role wants a
/// checkout" and "there is a task to name it by".
pub fn run_checkout(
    agent: &LoadedAgent,
    config: &AgentsConfig,
    task: Option<&cide_ipc::TaskId>,
) -> Option<String> {
    match (config.isolation, agent.def.worktree, task) {
        (Isolation::Worktree, true, Some(task)) => Some(checkout_name(&agent.def.id, Some(task))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cide_ipc::{AgentDef, Harness};
    use std::path::PathBuf;

    fn role(permission_mode: Option<&str>, unavailable: Option<&str>) -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope: cide_ipc::agents::AgentScope::Project,
                harness: Harness::Claude,
                description: "Implements one task end to end.".into(),
                system_prompt: "You are the developer agent.".into(),
                model: None,
                unavailable: unavailable.map(str::to_string),
                max_concurrent: 3,
                worktree: true,
            },
            origin: PathBuf::from("/repo/.cide/agents/developer.md"),
            shadows: None,
            tools: vec!["Read".into(), "Edit".into()],
            permission_mode: permission_mode.map(str::to_string),
            effort: None,
            extras: Vec::new(),
        }
    }

    fn enabled() -> AgentsConfig {
        AgentsConfig {
            enabled: true,
            ..AgentsConfig::default()
        }
    }

    /// A `cide-hook` that is where it should be.
    ///
    /// Every test below is about some *other* refusal, so each one needs the bridge to pass —
    /// which is the point of it being a named helper rather than a bare `Some(...)`: a test that
    /// meant to exercise the permissions gate and accidentally tripped the bridge gate would
    /// still pass, on the wrong sentence.
    fn bridge() -> Option<&'static std::path::Path> {
        Some(std::path::Path::new("/opt/cide/cide-hook"))
    }

    /// The refusal that outranks every other, because it makes them all moot.
    #[test]
    fn a_project_that_never_enabled_this_refuses_everything() {
        let agent = role(None, None);
        let why = dispatch_refusal(&agent, &AgentsConfig::default(), bridge()).expect("refused");
        assert!(why.contains(".cide/config.json"), "name the file: {why}");
    }

    /// A build with no `cide-hook` beside it refuses every role, and says which file is missing.
    ///
    /// The regression this whole argument exists for. `RunPlan::hook_bin` gates both the MCP
    /// server block and the tracker paragraph, so `None` used to mean *start the run without
    /// them*: the dispatch reported success and the child came up as an ordinary session with no
    /// way to reach the board. Ten consecutive opencode runs shipped in that state before anybody
    /// noticed, and they only noticed because the runs said so in comments they had filed through
    /// an MCP client they wrote themselves.
    #[test]
    fn a_build_with_no_bridge_refuses_every_role_and_names_the_binary() {
        let agent = role(None, None);
        let why = dispatch_refusal(&agent, &enabled(), None).expect("refused");
        assert!(why.contains("cide-hook"), "name the binary: {why}");
        assert!(
            why.contains("cargo build -p cide-hook") || why.contains("run.sh"),
            "name the fix: {why}"
        );
        // Says what it costs, not just that something is missing: a sentence that named a file
        // and stopped would leave the reader to guess whether it mattered.
        assert!(why.contains("tracker"), "{why}");

        // The same role with the bridge present is not refused for this — which is what makes the
        // assertion above about the bridge rather than about the role.
        assert_eq!(dispatch_refusal(&agent, &enabled(), bridge()), None);
    }

    /// "Subagents are off" still outranks it.
    ///
    /// The order in `dispatch_refusal` is the order a user can act on, and a project that never
    /// switched subagents on has nothing to do with a missing binary: telling them to rebuild
    /// `cide-hook` would send them after the wrong thing entirely.
    #[test]
    fn the_project_switch_outranks_a_missing_bridge() {
        let agent = role(None, None);
        let why = dispatch_refusal(&agent, &AgentsConfig::default(), None).expect("refused");
        assert!(why.contains(".cide/config.json"), "{why}");
        assert!(!why.contains("cide-hook"), "{why}");
    }

    /// And a missing bridge outranks the role's own fault, because it is true of every role.
    ///
    /// A user staring at a roster where every row carries a different sentence has no way to see
    /// that one thing is wrong with all of them.
    #[test]
    fn a_missing_bridge_outranks_a_roles_own_fault() {
        let agent = role(None, Some("“claude” is not on this app's PATH."));
        let why = dispatch_refusal(&agent, &enabled(), None).expect("refused");
        assert!(why.contains("cide-hook"), "{why}");
    }

    /// The role's own sentence is passed through verbatim: the panel is already drawing it, and
    /// a dispatch that invented a second wording would leave the user with two problems.
    #[test]
    fn an_unavailable_role_refuses_with_its_own_sentence() {
        let agent = role(None, Some("“claude” is not on this app's PATH."));
        assert_eq!(
            dispatch_refusal(&agent, &enabled(), bridge()).as_deref(),
            Some("“claude” is not on this app's PATH.")
        );
    }

    /// The gate that is the point of the whole `allowDangerousPermissions` key: the role loads,
    /// is listed, and is refused at the moment it would have run — with the fix named.
    #[test]
    fn bypass_permissions_loads_but_is_refused_until_the_project_authorises_it() {
        let agent = role(Some(defs::BYPASS_PERMISSIONS), None);
        assert!(is_dangerous(&agent));

        // The default project skips prompts for every unattended child
        // (`AgentsConfig::skip_permissions`, and its doc carries the measurement), so a role
        // that wrote the same stance down passes — refusing it would be a refusal about
        // nothing, and the gate's own comment says so.
        assert_eq!(dispatch_refusal(&agent, &enabled(), bridge()), None);

        // The two-acts rule bites where it means something: a project that switched skipping
        // off is exactly the project that meant to be asked.
        let asking = AgentsConfig {
            skip_permissions: false,
            ..enabled()
        };
        let why = dispatch_refusal(&agent, &asking, bridge()).expect("refused");
        assert!(
            why.contains("allowDangerousPermissions"),
            "the refusal has to name its own fix: {why}"
        );
        assert!(
            why.contains("skipPermissions"),
            "and the switch that armed the gate: {why}"
        );
        assert!(why.contains("developer"), "and the role: {why}");

        let authorised = AgentsConfig {
            allow_dangerous_permissions: true,
            ..asking
        };
        assert_eq!(dispatch_refusal(&agent, &authorised, bridge()), None);

        // The ordinary case is untouched by the gate whichever way skipping is set.
        assert_eq!(
            dispatch_refusal(&role(Some("acceptEdits"), None), &enabled(), bridge()),
            None
        );
        assert_eq!(
            dispatch_refusal(&role(Some("acceptEdits"), None), &asking, bridge()),
            None
        );
    }

    /// `max-concurrent` means what it says under both isolations. The worktree clamp this test
    /// used to pin is gone — per-task checkouts ([`checkout_name`]) dissolved its premise — and
    /// the remaining floor is 1, because a malformed `max-concurrent: 0` must not make a role
    /// undispatchable with nothing anywhere saying why.
    #[test]
    fn a_roles_declared_concurrency_stands_under_both_isolations() {
        let agent = role(None, None);
        let config = enabled();
        assert_eq!(agent.def.max_concurrent, 3);
        assert_eq!(effective_max_concurrent(&agent, &config), 3);
        let shared = AgentsConfig {
            isolation: Isolation::Shared,
            ..config
        };
        assert_eq!(effective_max_concurrent(&agent, &shared), 3);
    }

    /// The checkout mapping: deterministic, grammar-safe, and falling back rather than minting.
    #[test]
    fn a_checkout_is_named_by_role_and_task_and_survives_a_hostile_task_id() {
        let role = AgentId("developer".into());
        let task = |id: &str| cide_ipc::TaskId(id.to_string());

        // The two ordinary shapes.
        assert_eq!(checkout_name(&role, None), "developer");
        assert_eq!(
            checkout_name(&role, Some(&task("t-62"))),
            "developer-t-62",
            "a minted id passes through as itself"
        );

        // Hand-edited ids are forced through the worktree grammar, never trusted into a path.
        assert_eq!(
            checkout_name(&role, Some(&task("T 62/../x"))),
            "developer-t-62----x",
            "uppercase folds, everything else becomes a dash"
        );
        assert_eq!(
            checkout_name(&role, Some(&task("///"))),
            "developer",
            "an id that sanitises to nothing falls back to the role's bare name"
        );

        // Determinism is load-bearing: a resumed run recomputes this and must land in the
        // directory its transcript lives under.
        assert_eq!(
            checkout_name(&role, Some(&task("t-62"))),
            checkout_name(&role, Some(&task("t-62")))
        );

        // The 64 cap of the worktree grammar holds whatever the file says.
        let long = task(&"x".repeat(200));
        assert!(checkout_name(&role, Some(&long)).len() <= 64);
    }

    /// Where a run stands is one decision, and this is it: the intersection of the project
    /// isolating, the role wanting a checkout, and a task to name it by. A run with no task
    /// stands in the project root under every isolation (M40) — the panel never dispatched one,
    /// so the base-worktree posture it replaces stranded nothing.
    #[test]
    fn a_run_takes_a_checkout_only_for_a_task_under_worktree_isolation() {
        let agent = role(None, None);
        let config = enabled();
        let task = cide_ipc::TaskId("t-62".to_string());

        assert_eq!(
            run_checkout(&agent, &config, Some(&task)).as_deref(),
            Some("developer-t-62")
        );
        assert_eq!(
            run_checkout(&agent, &config, None),
            None,
            "no task, no worktree: the run stands in the project root"
        );

        let shared = AgentsConfig {
            isolation: Isolation::Shared,
            ..enabled()
        };
        assert_eq!(run_checkout(&agent, &shared, Some(&task)), None);
        assert_eq!(run_checkout(&agent, &shared, None), None);

        let mut root_only = role(None, None);
        root_only.def.worktree = false;
        assert_eq!(
            run_checkout(&root_only, &config, Some(&task)),
            None,
            "`worktree: false` opts the role out of the checkout for its tasks too"
        );
    }

    /// A project with no `.cide/` at all is off, whatever roles the user has defined globally.
    #[test]
    fn a_project_with_nothing_in_it_is_off() {
        let root = std::env::temp_dir().join(format!("cide-agents-{}-project", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");

        let project = load_project(&root);
        assert!(!project.enabled());
        // Whatever the roster holds, nothing in it can be dispatched.
        for agent in &project.catalog.agents {
            assert!(dispatch_refusal(agent, &project.config.agents, bridge()).is_some());
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}
