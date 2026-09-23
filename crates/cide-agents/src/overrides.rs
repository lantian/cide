//! Resolving a role against its local override. Pure, and reads no disk. (M45)
//!
//! `cide_ipc::overrides` is the shape and `cide_core::persist` is the file; this is the *rule*.
//! It is a free function over values for the reason the rest of this crate is: nothing here may
//! reach for a workspace or an `AppHandle`, so the caller loads the overrides at the edge and
//! hands them in — the arrangement `RunPlan::claude` already establishes.

use cide_ipc::{
    AgentOverride, Harness, LlmProvider, LlmSettings, PoolChoice, PoolEntry, ProjectOverrides,
};

use crate::defs::LoadedAgent;
use crate::harness::opencode::UserConfig;

/// What a role will actually run as, here, on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The CLI this run forks.
    pub harness: Harness,
    /// The ordered candidates a run falls down, empty when no pool applies.
    pub pool: Vec<PoolEntry>,
    /// Which pool they came from, for the run's row. `None` when [`Self::pool`] is empty.
    pub pool_name: Option<String>,
    /// The single model, when no pool applies. `None` leaves the provider default.
    pub model: Option<String>,
    /// `--variant`/`--effort`.
    pub effort: Option<String>,
    /// This role's concurrency cap, as [`crate::effective_max_concurrent`] folds it.
    ///
    /// The number is *enforced* at the dispatch, by the queue — a run is stamped with it when it
    /// is minted and the admission gate counts against that stamp. This copy is the same
    /// answer for a reader that already has a `Resolved` (the roster's row), and it is the same
    /// answer because both come from that one function: from M45 to M73 this field was computed
    /// here and read by nothing, exactly as `model` and `effort` were before `apply` — so an
    /// override that said "three of this role here" wrote a file, drew a value, and left the
    /// queue admitting one.
    pub max_concurrent: u16,
    /// The role's `permission-mode`, with the override's folded over it. (M82)
    ///
    /// `None` means neither said anything, and the project's `agents.permissionMode` then
    /// decides (`RunPlan::unattended`). Reaches a harness only through [`Self::apply`], for the
    /// reason that doc gives about `model` and `effort`.
    pub permission_mode: Option<String>,
    /// Why this run cannot start, or `None`.
    ///
    /// The **only** refusal this module produces, and it is deliberately narrow: an override that
    /// names a pool which does not exist. Everything else degrades, because an override is a
    /// convenience and a convenience must not stop a dispatch.
    pub refusal: Option<String>,
}

impl Resolved {
    /// The candidate a run starts on, or `None` for a run with no pool.
    #[must_use]
    pub fn first_choice(&self) -> Option<PoolChoice> {
        let entry = self.pool.first()?;
        Some(PoolChoice {
            pool: self.pool_name.clone().unwrap_or_default(),
            index: 0,
            entry: entry.clone(),
        })
    }

    /// Everything a child is forked with that a person can change from Settings, a role file or
    /// opencode's own configuration, as one comparable value. See [`ChildSettings`].
    ///
    /// `opencode` is what the binary resolved for this project, read at the edge
    /// (`harness::opencode::user_config`); it is kept only for an opencode child, for the same
    /// reason as the providers.
    #[must_use]
    pub fn child_settings(
        &self,
        llm: &LlmSettings,
        opencode: Option<&UserConfig>,
    ) -> ChildSettings {
        // `reads_provider_document`, not `== Opencode`: mimo reads the same document. (M81)
        let for_opencode = self.harness.reads_provider_document();
        ChildSettings {
            harness: self.harness,
            pool: self.pool.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            permission_mode: self.permission_mode.clone(),
            // Only opencode reads the provider document (`harness::opencode::provider_members`),
            // so only an opencode child is a different child when a provider changes. Carrying
            // the providers for every harness would restart a `claude` run over a key it never
            // sees.
            providers: for_opencode.then(|| llm.providers.clone()),
            opencode: for_opencode.then(|| opencode.cloned()).flatten(),
        }
    }

    /// Fold opencode's own default model in, where nothing in cide names one.
    ///
    /// **A continued opencode session keeps the model it last used**, whatever the configuration
    /// now says (`harness::opencode::UserConfig` carries the measurement), so a role that names
    /// no model was a role whose model no continuation could move. With the default spelled here
    /// every opencode child gets an explicit `--model` — the same one a fresh child would have
    /// picked — and a continuation follows the configuration like a fresh start does. Applied
    /// only for opencode, only under no pool (the pool *is* the model choice) and only where the
    /// override and the role are both silent, so nothing a person wrote is displaced by it.
    #[must_use]
    pub fn with_default_model(mut self, default: Option<String>) -> Self {
        if self.harness.reads_provider_document() && self.pool.is_empty() && self.model.is_none() {
            self.model = default.filter(|model| !model.trim().is_empty());
        }
        self
    }

    /// The role as the harness should read it: the committed definition with this machine's
    /// single-model and effort overrides folded **into** it.
    ///
    /// # Why a fold and not two more `RunPlan` fields
    ///
    /// Every harness reads `plan.agent.def.model` and `plan.agent.effort`, and so does every one
    /// of their tests. `model` and `effort` were computed here from the first day of overrides
    /// and read by nothing — the only caller took `harness`, `pool` and `refusal` off this struct
    /// and handed the harness the file's own `LoadedAgent` — so the Model and Effort fields on
    /// the Settings screen wrote a file, drew a value, and changed no child. A fold at the one
    /// place overrides are resolved is the fix that cannot be forgotten by the next harness,
    /// because the next harness will read the definition exactly as the four existing ones do.
    ///
    /// The file on disk is untouched; this is a value the plan borrows for one fork. While a
    /// pool applies, `def.model` is folded to `None` on purpose — `model` is `None` then, because
    /// the pool *is* the model choice and the candidate outranks the definition in every harness
    /// that takes one.
    #[must_use]
    pub fn apply(&self, agent: &LoadedAgent) -> LoadedAgent {
        let mut folded = agent.clone();
        folded.def.model = self.model.clone();
        folded.effort = self.effort.clone();
        folded.permission_mode = self.permission_mode.clone();
        folded
    }
}

/// What a child is forked with, as far as a person can change it without touching the run.
///
/// Recorded on a run at every fork and compared at **Resume**: a paused run whose settings no
/// longer match is put on a new child continuing the same conversation, instead of `SIGCONT`ing
/// a process whose model, provider and limits were fixed on its argv and environment at the
/// first fork. Equality is the whole interface — the registry never reads a field of this off a
/// run, it asks "would the child I fork now be a different child?" — and the fields are exactly
/// what the harnesses read: the resolved harness, the pool as configured (not the position in
/// it, which is the run's own business), the single model, the effort, and for opencode alone
/// the provider document its `OPENCODE_CONFIG_CONTENT` is built from, which is where a model's
/// context and output limits live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSettings {
    pub harness: Harness,
    pub pool: Vec<PoolEntry>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The resolved `permission-mode`. A mode is fixed on the argv at the fork, so a paused run
    /// whose override moved it is a different child. (M82)
    pub permission_mode: Option<String>,
    /// `Some` for opencode, `None` for every harness that never reads the provider document.
    pub providers: Option<Vec<LlmProvider>>,
    /// What opencode itself resolved — its default model and its providers — for an opencode
    /// child; `None` for every other harness, and for an opencode child forked while the
    /// binary could not be asked.
    ///
    /// **For a mimo child it is what *mimo* resolved** (`mimo debug config`, through the same
    /// `user_config` with the other flavour). The name stayed because the family did: the
    /// field means "this opencode-shaped CLI's own configuration", and a second field that is
    /// `None` whenever this one is `Some` would be a comparison with two halves that can never
    /// both be filled. `harness` sits beside it, so a mimo config never compares equal to an
    /// opencode one as the same child. (M81)
    pub opencode: Option<UserConfig>,
}

impl ChildSettings {
    /// The fields on which `other` differs from this, by name, for a log line: a Resume that
    /// restarts a run should say what moved, and one that does not should be able to say
    /// nothing did.
    #[must_use]
    pub fn changed_fields(&self, other: &Self) -> Vec<&'static str> {
        let mut changed = Vec::new();
        if self.harness != other.harness {
            changed.push("harness");
        }
        if self.pool != other.pool {
            changed.push("pool");
        }
        if self.model != other.model {
            changed.push("model");
        }
        if self.effort != other.effort {
            changed.push("effort");
        }
        if self.permission_mode != other.permission_mode {
            changed.push("permission mode");
        }
        if self.providers != other.providers {
            changed.push("providers");
        }
        if self.opencode != other.opencode {
            changed.push("opencode config");
        }
        changed
    }
}

/// The override row that applies to this role, or `None` where no override may apply.
///
/// A Claude Code subagent is forced to claude at load (`defs`'s "there is no second answer"), so
/// an override must not move it. Answered here rather than at each reader so none can forget,
/// and stated as a *scope* test rather than by comparing harnesses, because a cide-scope role
/// that happens to name claude is perfectly overridable.
///
/// One producer, because two readers ask this question at two different moments —
/// [`resolve`] at the fork and [`crate::effective_max_concurrent`] at the dispatch — and a
/// scope rule spelled twice is a rule that holds in one of them.
#[must_use]
pub fn row_for<'a>(
    agent: &LoadedAgent,
    overrides: &'a ProjectOverrides,
) -> Option<&'a AgentOverride> {
    match agent.def.scope.is_claude_code() {
        true => None,
        false => Some(overrides.for_role(agent.id().as_str())),
    }
}

/// Fold a role, its project's config, this project's overrides and the global pools into what
/// will actually run.
///
/// # Precedence, and why a role's row replaces rather than merges
///
/// Per field: the override that applies to this role (its own row, else the project-wide `all` —
/// [`ProjectOverrides::for_role`] decides which, and does not merge them), then the role's own
/// committed value.
///
/// The project's `.cide/config.json` is deliberately **not** a third rung, because it is already
/// folded in: `defs`'s loader applies `agents.harness` as the default for a role that names none,
/// so a `LoadedAgent`'s `harness` is the project's answer where the file was silent. Taking the
/// config here as well would be consulting it twice and inviting the two readings to disagree.
///
/// # The one refusal
///
/// A pool named by an override and absent from settings **refuses the dispatch**, with a
/// sentence. Falling back would answer with a model nobody chose and bill somebody for it, which
/// is the opposite of what setting a pool is usually for — people build one to *cap* spend. The
/// refusal is affordable precisely because both halves are local: the person who made the
/// inconsistency is the person looking at the screen, and no teammate's clone can hit it.
///
/// An override naming a pool while the effective harness is **not** opencode is not a refusal.
/// The pool is simply not applied and the caller may say so; refusing there would make switching
/// a role to claude for an afternoon into an error about a setting the user did not touch.
#[must_use]
/// Which CLI this role actually runs as here. One producer, for [`effective_max_concurrent`]'s
/// reason. (M78)
///
/// The fork resolved this and the **dispatch did not**, so `DispatchSpec::harness` carried the
/// committed file's answer into the registry and stayed there. Two readers of one override
/// disagreeing is how the queue's `max_concurrent` came to be computed here and obeyed nowhere;
/// this is the same fold, one field along, and the damage was larger because `observe` picks the
/// state machine that reads the child's output from it — a role redirected onto another CLI was
/// read with the wrong one and never left `RunState::Starting`. See
/// `AgentRegistry::note_harness`, which is the other half: the fork re-reads the file, because a
/// `git checkout` can move it while a run sits in the queue.
///
/// Through [`row_for`], so the scope rule holds here too: a Claude Code subagent takes no
/// override, and it takes none of this one either.
pub fn effective_harness(agent: &LoadedAgent, overrides: &ProjectOverrides) -> Harness {
    row_for(agent, overrides)
        .and_then(|over| over.harness)
        .unwrap_or(agent.def.harness)
}

pub fn resolve(agent: &LoadedAgent, overrides: &ProjectOverrides, llm: &LlmSettings) -> Resolved {
    let empty = AgentOverride::default();
    let over: &AgentOverride = row_for(agent, overrides).unwrap_or(&empty);

    let harness = effective_harness(agent, overrides);
    let effort = over.effort.clone().or_else(|| agent.effort.clone());
    // Through `effective_max_concurrent` and not off `over` directly, because the *queue* reads
    // that function and this struct is read by the fork: two foldings of one override is how
    // this field came to be computed here and obeyed nowhere — see `Resolved::max_concurrent`.
    let max_concurrent = crate::effective_max_concurrent(agent, overrides);
    // The override's mode, when it is a word the CLIs know, then the role's own. An unknown word
    // is dropped with a warning rather than refused: this module refuses only a missing pool,
    // and handing an unvalidated string to `--permission-mode` is the silent behaviour change
    // `defs::PERMISSION_MODES`' doc refuses for a role file.
    let permission_mode = match over.permission_mode.as_deref().map(str::trim) {
        Some(mode) if crate::defs::PERMISSION_MODES.contains(&mode) => Some(mode.to_string()),
        Some(other) => {
            if !other.is_empty() {
                tracing::warn!(
                    role = %agent.def.id,
                    value = other,
                    "a local permission-mode override is not a mode cide knows; ignoring it"
                );
            }
            agent.permission_mode.clone()
        }
        None => agent.permission_mode.clone(),
    };

    // A pool is opencode's alone: no other harness takes a `provider/model`, and applying one
    // would hand a CLI an id it does not parse.
    let wants_pool = harness.reads_provider_document();

    let mut refusal = None;
    let mut pool = Vec::new();
    let mut pool_name = None;
    if let Some(named) = over.pool.as_deref().filter(|name| !name.is_empty())
        && wants_pool
    {
        match llm.pool(named) {
            Some(found) => {
                // Entries the user has not finished typing are skipped rather than sent: a
                // blank half would spell `--model provider/` on the argv. They are *kept* in
                // settings — see `LlmSettings::cleaned` — and filtered here, where the value
                // is used.
                pool = found
                    .entries
                    .iter()
                    .filter(|entry| !entry.provider.is_empty() && !entry.model.is_empty())
                    .cloned()
                    .collect();
                pool_name = Some(found.name.clone());
                if pool.is_empty() {
                    refusal = Some(format!(
                        "The pool “{named}” has no complete entry — every row is missing a \
                             provider or a model. Finish it on the Models screen in \
                             Settings, or clear this role's pool override."
                    ));
                }
            }
            None => {
                refusal = Some(format!(
                    "This role is overridden onto the pool “{named}”, which is not \
                         configured on this machine. Add it on the Models screen in Settings, or clear the \
                         override in the Agents screen."
                ));
            }
        }
    }

    // The override's single model, then the role's own. Ignored entirely while a pool applies —
    // the pool *is* the model choice, and a run cannot be on both.
    let model = match pool.is_empty() {
        false => None,
        true => over
            .model
            .clone()
            .filter(|model| !model.is_empty())
            .or_else(|| agent.def.model.clone()),
    };

    Resolved {
        harness,
        pool,
        pool_name,
        model,
        effort,
        max_concurrent,
        permission_mode,
        refusal,
    }
}

/// Whether an override would do anything for this role, for the screen's benefit.
///
/// A pool named while the effective harness is not opencode is *inert* — not refused, but worth
/// saying so on the row, because a setting that silently does nothing is the failure this whole
/// module's shape is arranged to avoid.
#[must_use]
pub fn inert_pool(agent: &LoadedAgent, overrides: &ProjectOverrides) -> bool {
    if agent.def.scope.is_claude_code() {
        return overrides.for_role(agent.id().as_str()).pool.is_some();
    }
    let over = overrides.for_role(agent.id().as_str());
    let harness = over.harness.unwrap_or(agent.def.harness);
    over.pool.as_deref().is_some_and(|name| !name.is_empty()) && !harness.reads_provider_document()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::agents::AgentScope;
    use cide_ipc::{AgentDef, AgentId, ModelPool};

    fn role(scope: AgentScope, harness: Harness) -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope,
                harness,
                description: "d".into(),
                system_prompt: "p".into(),
                model: Some("committed-model".into()),
                color: None,
                unavailable: None,
                max_concurrent: 2,
                worktree: true,
            },
            origin: std::path::PathBuf::from("/repo/.cide/agents/developer.md"),
            shadows: None,
            tools: Vec::new(),
            permission_mode: None,
            effort: Some("committed-effort".into()),
            extras: Vec::new(),
        }
    }

    fn entry(provider: &str, model: &str) -> PoolEntry {
        PoolEntry {
            provider: provider.into(),
            model: model.into(),
            variant: String::new(),
        }
    }

    fn settings(entries: Vec<PoolEntry>) -> LlmSettings {
        LlmSettings {
            providers: Vec::new(),
            pools: vec![ModelPool {
                name: "cheap-first".into(),
                description: String::new(),
                entries,
            }],
        }
    }

    fn with(over: AgentOverride) -> ProjectOverrides {
        ProjectOverrides {
            all: over,
            roles: Default::default(),
        }
    }

    /// [`effective_harness`] over the three cases its two callers depend on. (M78)
    ///
    /// This is the fold the dispatch was missing. The third case is the one that keeps the
    /// **scope rule** in one place: a Claude Code subagent takes no override, so a project-wide
    /// `harness` row must not move one onto another CLI — `row_for` is what refuses it, and a
    /// second reader that folded `over.harness` directly would disagree with the roster about
    /// which roles an override may touch.
    #[test]
    fn the_effective_harness_is_the_override_where_there_is_one() {
        let committed = role(AgentScope::Project, Harness::Opencode);
        assert_eq!(
            effective_harness(&committed, &ProjectOverrides::default()),
            Harness::Opencode,
        );
        assert_eq!(
            effective_harness(
                &committed,
                &with(AgentOverride {
                    harness: Some(Harness::Codex),
                    ..AgentOverride::default()
                }),
            ),
            Harness::Codex,
        );
        // And a Claude Code subagent keeps its own, whatever the project-wide row says.
        let subagent = role(AgentScope::ClaudeProject, Harness::Claude);
        assert_eq!(
            effective_harness(
                &subagent,
                &with(AgentOverride {
                    harness: Some(Harness::Codex),
                    ..AgentOverride::default()
                }),
            ),
            Harness::Claude,
        );
    }

    /// And `resolve` answers the same thing, because it is the same producer — two foldings of
    /// one override is how `max_concurrent` came to be computed twice and obeyed once.
    #[test]
    fn resolve_and_the_effective_harness_cannot_disagree() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let overrides = with(AgentOverride {
            harness: Some(Harness::Codex),
            ..AgentOverride::default()
        });
        let out = resolve(&agent, &overrides, &settings(Vec::new()));
        assert_eq!(out.harness, effective_harness(&agent, &overrides));
        assert_eq!(out.harness, Harness::Codex);
    }

    /// No override at all is exactly today's behaviour, field for field.
    #[test]
    fn nothing_overridden_is_the_committed_role() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let out = resolve(
            &agent,
            &ProjectOverrides::default(),
            &LlmSettings::default(),
        );
        assert_eq!(out.harness, Harness::Claude);
        assert_eq!(out.model.as_deref(), Some("committed-model"));
        assert_eq!(out.effort.as_deref(), Some("committed-effort"));
        assert_eq!(out.max_concurrent, 2);
        assert!(out.pool.is_empty() && out.refusal.is_none());
    }

    #[test]
    fn an_override_wins_field_by_field_over_the_committed_role() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                harness: Some(Harness::Opencode),
                effort: Some("high".into()),
                max_concurrent: Some(6),
                ..Default::default()
            }),
            &LlmSettings::default(),
        );
        assert_eq!(out.harness, Harness::Opencode);
        assert_eq!(out.effort.as_deref(), Some("high"));
        assert_eq!(out.max_concurrent, 6);
        // Untouched fields still come from the file.
        assert_eq!(out.model.as_deref(), Some("committed-model"));
    }

    /// A role's own row **replaces** the project default rather than merging with it — a
    /// half-merge would hand a claude role a pool its harness cannot use.
    #[test]
    fn a_roles_own_row_replaces_the_project_default() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let mut overrides = with(AgentOverride {
            harness: Some(Harness::Opencode),
            pool: Some("cheap-first".into()),
            ..Default::default()
        });
        overrides.roles.insert(
            "developer".into(),
            AgentOverride {
                harness: Some(Harness::Claude),
                ..Default::default()
            },
        );
        let out = resolve(
            &agent,
            &overrides,
            &settings(vec![entry("openrouter", "m")]),
        );
        assert_eq!(out.harness, Harness::Claude);
        assert!(
            out.pool.is_empty(),
            "the project-wide pool must not leak through a role's own row"
        );
        assert!(out.refusal.is_none());
    }

    #[test]
    fn a_pool_resolves_to_its_entries_in_order() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                pool: Some("cheap-first".into()),
                ..Default::default()
            }),
            &settings(vec![entry("openrouter", "a"), entry("lmstudio", "b")]),
        );
        assert_eq!(out.pool.len(), 2);
        assert_eq!(out.pool_name.as_deref(), Some("cheap-first"));
        let first = out.first_choice().expect("a candidate");
        assert_eq!(first.index, 0);
        assert_eq!(first.entry.model_flag(), "openrouter/a");
        // A pool IS the model choice; the committed `model:` must not also be passed.
        assert!(
            out.model.is_none(),
            "a run cannot be on a pool and a model at once"
        );
    }

    /// Half-typed rows are stored but never sent: a blank half would spell `--model provider/`.
    #[test]
    fn an_unfinished_entry_is_skipped_rather_than_sent() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                pool: Some("cheap-first".into()),
                ..Default::default()
            }),
            &settings(vec![entry("", ""), entry("openrouter", "a")]),
        );
        assert_eq!(out.pool.len(), 1);
        assert_eq!(out.pool[0].model_flag(), "openrouter/a");
        assert!(out.refusal.is_none());
    }

    /// The one refusal, and it names both the pool and where to fix it.
    #[test]
    fn a_pool_that_does_not_exist_refuses_and_says_where_to_look() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                pool: Some("gone".into()),
                ..Default::default()
            }),
            &LlmSettings::default(),
        );
        let refusal = out.refusal.expect("a pool that is not there refuses");
        assert!(refusal.contains("gone"), "{refusal}");
        assert!(refusal.contains("Settings"), "{refusal}");
    }

    /// A pool whose every row is half-typed is refused too — otherwise the run would start on
    /// the provider default, silently, which is the outcome the refusal exists to prevent.
    #[test]
    fn a_pool_with_no_complete_entry_refuses() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                pool: Some("cheap-first".into()),
                ..Default::default()
            }),
            &settings(vec![entry("openrouter", "")]),
        );
        assert!(
            out.refusal.is_some(),
            "an unusable pool is not silently ignored"
        );
    }

    /// Not a refusal: switching a role to claude for an afternoon must not become an error about
    /// a setting the user did not touch. The pool simply does not apply, and `inert_pool` is what
    /// lets the screen say so.
    #[test]
    fn a_pool_on_a_non_opencode_harness_is_inert_rather_than_refused() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let overrides = with(AgentOverride {
            harness: Some(Harness::Claude),
            pool: Some("cheap-first".into()),
            ..Default::default()
        });
        let out = resolve(
            &agent,
            &overrides,
            &settings(vec![entry("openrouter", "a")]),
        );
        assert!(out.refusal.is_none(), "not an error");
        assert!(out.pool.is_empty(), "and not applied");
        assert!(inert_pool(&agent, &overrides), "but the screen can say so");
    }

    /// `defs` forces `Harness::Claude` for a `.claude/agents/` role — "there is no second answer"
    /// — so an override must not move one, or it would reverse a stated rule in silence.
    #[test]
    fn a_claude_code_role_cannot_be_overridden() {
        for scope in [AgentScope::ClaudeProject, AgentScope::ClaudeGlobal] {
            let agent = role(scope, Harness::Claude);
            let overrides = with(AgentOverride {
                harness: Some(Harness::Opencode),
                pool: Some("cheap-first".into()),
                model: Some("other".into()),
                effort: Some("high".into()),
                max_concurrent: Some(9),
                permission_mode: Some("plan".into()),
            });
            let out = resolve(
                &agent,
                &overrides,
                &settings(vec![entry("openrouter", "a")]),
            );
            assert_eq!(out.harness, Harness::Claude, "{scope:?}");
            assert!(out.pool.is_empty(), "{scope:?}");
            assert_eq!(out.model.as_deref(), Some("committed-model"), "{scope:?}");
            assert_eq!(out.effort.as_deref(), Some("committed-effort"), "{scope:?}");
            assert_eq!(out.max_concurrent, 2, "{scope:?}");
            assert_eq!(out.permission_mode, None, "{scope:?}");
            assert!(out.refusal.is_none(), "{scope:?}");
            assert!(
                inert_pool(&agent, &overrides),
                "and the screen says the pool does nothing"
            );
        }
    }

    /// The override's permission mode beats the role's own, reaches the harness through
    /// `apply`, and an unknown word leaves the role's alone rather than reaching an argv. (M82)
    #[test]
    fn an_override_permission_mode_beats_the_roles_and_is_folded_in() {
        let mut agent = role(AgentScope::Project, Harness::Claude);
        agent.permission_mode = Some("acceptEdits".into());

        let out = resolve(
            &agent,
            &ProjectOverrides::default(),
            &LlmSettings::default(),
        );
        assert_eq!(out.permission_mode.as_deref(), Some("acceptEdits"));

        let out = resolve(
            &agent,
            &with(AgentOverride {
                permission_mode: Some("auto".into()),
                ..Default::default()
            }),
            &LlmSettings::default(),
        );
        assert_eq!(out.permission_mode.as_deref(), Some("auto"));
        assert_eq!(out.apply(&agent).permission_mode.as_deref(), Some("auto"));

        let out = resolve(
            &agent,
            &with(AgentOverride {
                permission_mode: Some("yolo".into()),
                ..Default::default()
            }),
            &LlmSettings::default(),
        );
        assert_eq!(out.permission_mode.as_deref(), Some("acceptEdits"));
    }

    /// A paused run whose override moved its mode is a different child, so Resume restarts it.
    #[test]
    fn a_changed_permission_mode_is_a_different_child() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let llm = LlmSettings::default();
        let before = resolve(&agent, &ProjectOverrides::default(), &llm).child_settings(&llm, None);
        let after = resolve(
            &agent,
            &with(AgentOverride {
                permission_mode: Some("plan".into()),
                ..Default::default()
            }),
            &llm,
        )
        .child_settings(&llm, None);
        assert_eq!(before.changed_fields(&after), vec!["permission mode"]);
    }

    #[test]
    fn an_override_model_beats_the_committed_one_when_no_pool_applies() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                model: Some("openrouter/deepseek-chat".into()),
                ..Default::default()
            }),
            &LlmSettings::default(),
        );
        assert_eq!(out.model.as_deref(), Some("openrouter/deepseek-chat"));
        assert!(out.pool.is_empty());
    }

    /// An override that says nothing is stored rather than dropped, and resolves to the
    /// committed role — `LlmSettings::cleaned`'s lesson, applied here.
    #[test]
    fn an_empty_override_changes_nothing() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let mut overrides = ProjectOverrides::default();
        overrides
            .roles
            .insert("developer".into(), AgentOverride::default());
        assert!(overrides.roles["developer"].is_empty());
        let out = resolve(&agent, &overrides, &LlmSettings::default());
        assert_eq!(out.harness, Harness::Claude);
        assert_eq!(out.model.as_deref(), Some("committed-model"));
    }

    // ==========================================================================================
    // The fold. `model` and `effort` were resolved here and read by nobody until this existed —
    // the Settings screen's Model and Effort fields changed no child. See `Resolved::apply`.
    // ==========================================================================================

    /// The override's single model and effort land where the harnesses read them, and nothing
    /// else about the role moves.
    #[test]
    fn applying_the_resolution_puts_the_override_where_the_harness_reads_it() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                model: Some("opus".into()),
                effort: Some("high".into()),
                ..Default::default()
            }),
            &LlmSettings::default(),
        );
        let folded = out.apply(&agent);
        assert_eq!(folded.def.model.as_deref(), Some("opus"));
        assert_eq!(folded.effort.as_deref(), Some("high"));

        let mut rest = folded.clone();
        rest.def.model = agent.def.model.clone();
        rest.effort = agent.effort.clone();
        assert_eq!(rest, agent, "only the two overridden fields changed");
        assert_eq!(
            agent.def.model.as_deref(),
            Some("committed-model"),
            "the value handed in is untouched — the fold is a borrowed copy, never the file"
        );
    }

    /// No override is the committed role, field for field, through the fold as well.
    #[test]
    fn applying_nothing_is_the_committed_role() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let out = resolve(
            &agent,
            &ProjectOverrides::default(),
            &LlmSettings::default(),
        );
        assert_eq!(out.apply(&agent), agent);
    }

    /// While a pool applies the definition's model is folded away: the candidate is the model
    /// choice, and a definition still naming one would be a second answer for a harness to read.
    #[test]
    fn a_pool_folds_the_definitions_model_away() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let out = resolve(
            &agent,
            &with(AgentOverride {
                pool: Some("cheap-first".into()),
                ..Default::default()
            }),
            &settings(vec![entry("openrouter", "deepseek-chat")]),
        );
        assert!(!out.pool.is_empty());
        assert_eq!(out.apply(&agent).def.model, None);
    }

    // ==========================================================================================
    // The fingerprint a Resume compares. Equality is the interface; these pin what moves it.
    // ==========================================================================================

    fn custom_provider(context: u32) -> LlmProvider {
        LlmProvider::Custom {
            id: "lmstudio".into(),
            label: String::new(),
            enabled: true,
            npm: String::new(),
            base_url: "http://localhost:1234/v1".into(),
            api_key: String::new(),
            models: vec![cide_ipc::LlmModel {
                id: "qwen3-8b".into(),
                label: String::new(),
                context,
                output: 0,
            }],
        }
    }

    fn settings_with_provider(context: u32) -> LlmSettings {
        LlmSettings {
            providers: vec![custom_provider(context)],
            pools: vec![cide_ipc::ModelPool {
                name: "cheap-first".into(),
                description: String::new(),
                entries: vec![entry("lmstudio", "qwen3-8b")],
            }],
        }
    }

    /// The same configuration twice is the same child — a Resume with nothing changed must be a
    /// plain thaw, or every pause would cost the in-flight turn.
    #[test]
    fn unchanged_settings_fingerprint_equal() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let overrides = with(AgentOverride {
            pool: Some("cheap-first".into()),
            ..Default::default()
        });
        let llm = settings_with_provider(32_000);
        let a = resolve(&agent, &overrides, &llm).child_settings(&llm, None);
        let b = resolve(&agent, &overrides, &llm).child_settings(&llm, None);
        assert_eq!(a, b);
    }

    /// A model's context limit lives in the provider document, which only opencode reads: the
    /// same edit is a different child for an opencode role and the same child for a claude one.
    #[test]
    fn a_context_limit_moves_an_opencode_child_and_not_a_claude_one() {
        let overrides = with(AgentOverride {
            pool: Some("cheap-first".into()),
            ..Default::default()
        });
        let before = settings_with_provider(32_000);
        let after = settings_with_provider(128_000);

        let opencode = role(AgentScope::Project, Harness::Opencode);
        assert_ne!(
            resolve(&opencode, &overrides, &before).child_settings(&before, None),
            resolve(&opencode, &overrides, &after).child_settings(&after, None),
            "opencode is forked with the document the limit is written into"
        );

        let claude = role(AgentScope::Project, Harness::Claude);
        assert_eq!(
            resolve(&claude, &overrides, &before).child_settings(&before, None),
            resolve(&claude, &overrides, &after).child_settings(&after, None),
            "a claude child never sees the provider document"
        );
    }

    /// The pool's entries are compared, not its name or the run's position in it.
    #[test]
    fn editing_the_pool_moves_the_fingerprint() {
        let agent = role(AgentScope::Project, Harness::Opencode);
        let overrides = with(AgentOverride {
            pool: Some("cheap-first".into()),
            ..Default::default()
        });
        let one = settings(vec![entry("openrouter", "deepseek-chat")]);
        let two = settings(vec![
            entry("openrouter", "deepseek-chat"),
            entry("anthropic", "claude-sonnet-4-5"),
        ]);
        assert_ne!(
            resolve(&agent, &overrides, &one).child_settings(&one, None),
            resolve(&agent, &overrides, &two).child_settings(&two, None)
        );
    }

    fn user_config(model: &str) -> UserConfig {
        UserConfig {
            model: Some(model.into()),
            small_model: None,
            provider: r#"{"deepseek":{}}"#.into(),
        }
    }

    /// opencode's own default lands on an opencode role that names nothing, and nowhere else:
    /// not on a pool (the pool is the model choice), not over a role's or an override's own
    /// model, and never on another harness.
    #[test]
    fn opencodes_default_fills_only_a_silent_opencode_role() {
        let llm = LlmSettings::default();
        let default = Some("deepseek/deepseek-flash".to_string());

        let mut silent = role(AgentScope::Project, Harness::Opencode);
        silent.def.model = None;
        let out = resolve(&silent, &ProjectOverrides::default(), &llm)
            .with_default_model(default.clone());
        assert_eq!(out.model.as_deref(), Some("deepseek/deepseek-flash"));
        assert_eq!(
            out.apply(&silent).def.model.as_deref(),
            Some("deepseek/deepseek-flash"),
            "and it reaches the harness through the fold, as `--model`"
        );

        let named = role(AgentScope::Project, Harness::Opencode);
        let out =
            resolve(&named, &ProjectOverrides::default(), &llm).with_default_model(default.clone());
        assert_eq!(out.model.as_deref(), Some("committed-model"));

        let pooled = resolve(
            &silent,
            &with(AgentOverride {
                pool: Some("cheap-first".into()),
                ..Default::default()
            }),
            &settings(vec![entry("openrouter", "deepseek-chat")]),
        )
        .with_default_model(default.clone());
        assert_eq!(pooled.model, None);

        let mut claude = role(AgentScope::Project, Harness::Claude);
        claude.def.model = None;
        let out = resolve(&claude, &ProjectOverrides::default(), &llm).with_default_model(default);
        assert_eq!(out.model, None, "opencode's default is opencode's");

        let blank = resolve(&silent, &ProjectOverrides::default(), &llm)
            .with_default_model(Some("  ".into()));
        assert_eq!(blank.model, None, "a blank default is no default");
    }

    /// opencode's resolved configuration is part of an opencode child's fingerprint and of no
    /// other harness's — the same split as the providers, for the same reason.
    #[test]
    fn opencodes_configuration_moves_an_opencode_child_and_not_a_claude_one() {
        let llm = LlmSettings::default();
        let overrides = ProjectOverrides::default();
        let before = user_config("vllm/qwen3.8-27b-long");
        let after = user_config("deepseek/deepseek-flash");

        let opencode = role(AgentScope::Project, Harness::Opencode);
        let was = resolve(&opencode, &overrides, &llm).child_settings(&llm, Some(&before));
        let now = resolve(&opencode, &overrides, &llm).child_settings(&llm, Some(&after));
        assert_ne!(was, now);
        assert_eq!(was.changed_fields(&now), vec!["opencode config"]);

        let claude = role(AgentScope::Project, Harness::Claude);
        assert_eq!(
            resolve(&claude, &overrides, &llm).child_settings(&llm, Some(&before)),
            resolve(&claude, &overrides, &llm).child_settings(&llm, Some(&after))
        );
    }

    /// The single model, the effort and the harness each move it, for every harness.
    #[test]
    fn model_effort_and_harness_each_move_the_fingerprint() {
        let agent = role(AgentScope::Project, Harness::Claude);
        let llm = LlmSettings::default();
        let base = resolve(&agent, &ProjectOverrides::default(), &llm).child_settings(&llm, None);
        for over in [
            AgentOverride {
                model: Some("opus".into()),
                ..Default::default()
            },
            AgentOverride {
                effort: Some("high".into()),
                ..Default::default()
            },
            AgentOverride {
                harness: Some(Harness::Codex),
                ..Default::default()
            },
        ] {
            assert_ne!(
                resolve(&agent, &with(over.clone()), &llm).child_settings(&llm, None),
                base,
                "{over:?}"
            );
        }
    }
}
