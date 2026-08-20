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

pub mod config;
pub mod defs;
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
    ClaudeHarness, Delivery, Harness, HarnessError, HarnessSpawn, Observation, OpencodeHarness,
    RunPlan, SessionBinding, for_kind, registry,
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
/// it makes every other answer moot. Then the role's own [`cide_ipc::AgentDef::unavailable`],
/// which the panel is already drawing. Then the dangerous-permissions gate, which is last because
/// it is the only one that is *about this dispatch* rather than about the state of the world.
pub fn dispatch_refusal(agent: &LoadedAgent, config: &AgentsConfig) -> Option<String> {
    if !config.enabled {
        return Some(format!(
            "Subagents are off for this project. Turn them on in the Agents panel, which writes \
             `{}/config.json`.",
            config::CIDE_DIR
        ));
    }
    if let Some(reason) = &agent.def.unavailable {
        return Some(reason.clone());
    }
    if is_dangerous(agent) && !config.allow_dangerous_permissions {
        return Some(format!(
            "`{}` asks for `permission-mode: {}`, which lets it edit, delete and run anything \
             without asking. This project has not authorised that: set \
             `agents.allowDangerousPermissions` to true in `{}/config.json` if you mean it.",
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

/// How many runs of this role may actually be live, once the project's isolation is accounted
/// for.
///
/// Worktree isolation pins it to 1: there is **one worktree per agent**, not per run, so a second
/// concurrent run of the same role would be a second process editing one checkout with nothing
/// arbitrating between them. That is the failure the isolation exists to prevent, so the role's
/// own `max-concurrent` loses to it.
///
/// The clamp is never silent — [`concurrency_note`] is the sentence that goes with it, and the
/// row that shows a role's concurrency shows that sentence beside it. A silently ignored setting
/// is how a user concludes cide does not work.
pub fn effective_max_concurrent(agent: &LoadedAgent, config: &AgentsConfig) -> u16 {
    let declared = agent.def.max_concurrent.max(1);
    match config.isolation {
        Isolation::Worktree => 1,
        Isolation::Shared => declared,
    }
}

/// The sentence explaining a clamped `max-concurrent`, or `None` when nothing was clamped.
pub fn concurrency_note(agent: &LoadedAgent, config: &AgentsConfig) -> Option<String> {
    let declared = agent.def.max_concurrent.max(1);
    let effective = effective_max_concurrent(agent, config);
    (effective != declared).then(|| {
        format!(
            "`max-concurrent: {declared}` is capped at {effective} while this project uses \
             worktree isolation: there is one worktree per agent, so a role runs one task at a \
             time."
        )
    })
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
                harness: Harness::Claude,
                description: "Implements one task end to end.".into(),
                system_prompt: "You are the developer agent.".into(),
                model: None,
                unavailable: unavailable.map(str::to_string),
                max_concurrent: 3,
            },
            origin: PathBuf::from("/repo/.cide/agents/developer.md"),
            shadows: None,
            tools: vec!["Read".into(), "Edit".into()],
            permission_mode: permission_mode.map(str::to_string),
            effort: None,
        }
    }

    fn enabled() -> AgentsConfig {
        AgentsConfig {
            enabled: true,
            ..AgentsConfig::default()
        }
    }

    /// The refusal that outranks every other, because it makes them all moot.
    #[test]
    fn a_project_that_never_enabled_this_refuses_everything() {
        let agent = role(None, None);
        let why = dispatch_refusal(&agent, &AgentsConfig::default()).expect("refused");
        assert!(why.contains(".cide/config.json"), "name the file: {why}");
    }

    /// The role's own sentence is passed through verbatim: the panel is already drawing it, and
    /// a dispatch that invented a second wording would leave the user with two problems.
    #[test]
    fn an_unavailable_role_refuses_with_its_own_sentence() {
        let agent = role(None, Some("“claude” is not on this app's PATH."));
        assert_eq!(
            dispatch_refusal(&agent, &enabled()).as_deref(),
            Some("“claude” is not on this app's PATH.")
        );
    }

    /// The gate that is the point of the whole `allowDangerousPermissions` key: the role loads,
    /// is listed, and is refused at the moment it would have run — with the fix named.
    #[test]
    fn bypass_permissions_loads_but_is_refused_until_the_project_authorises_it() {
        let agent = role(Some(defs::BYPASS_PERMISSIONS), None);
        assert!(is_dangerous(&agent));

        let why = dispatch_refusal(&agent, &enabled()).expect("refused");
        assert!(
            why.contains("allowDangerousPermissions"),
            "the refusal has to name its own fix: {why}"
        );
        assert!(why.contains("developer"), "and the role: {why}");

        let authorised = AgentsConfig {
            allow_dangerous_permissions: true,
            ..enabled()
        };
        assert_eq!(dispatch_refusal(&agent, &authorised), None);

        // The ordinary case is untouched by the gate.
        assert_eq!(
            dispatch_refusal(&role(Some("acceptEdits"), None), &enabled()),
            None
        );
    }

    /// One worktree per agent means one run at a time, and the clamp is never silent.
    #[test]
    fn worktree_isolation_caps_a_role_at_one_run_and_says_so() {
        let agent = role(None, None);
        let config = enabled();
        assert_eq!(agent.def.max_concurrent, 3);
        assert_eq!(effective_max_concurrent(&agent, &config), 1);
        let note =
            concurrency_note(&agent, &config).expect("a clamp nobody is told about is a bug");
        assert!(note.contains("worktree"), "{note}");

        let shared = AgentsConfig {
            isolation: Isolation::Shared,
            ..config
        };
        assert_eq!(effective_max_concurrent(&agent, &shared), 3);
        assert_eq!(concurrency_note(&agent, &shared), None);
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
            assert!(dispatch_refusal(agent, &project.config.agents).is_some());
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}
