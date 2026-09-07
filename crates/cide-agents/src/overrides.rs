//! Resolving a role against its local override. Pure, and reads no disk. (M45)
//!
//! `cide_ipc::overrides` is the shape and `cide_core::persist` is the file; this is the *rule*.
//! It is a free function over values for the reason the rest of this crate is: nothing here may
//! reach for a workspace or an `AppHandle`, so the caller loads the overrides at the edge and
//! hands them in — the arrangement `RunPlan::claude` already establishes.

use cide_ipc::{AgentOverride, Harness, LlmSettings, PoolChoice, PoolEntry, ProjectOverrides};

use crate::defs::LoadedAgent;

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
    /// This role's concurrency cap.
    pub max_concurrent: u16,
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
pub fn resolve(agent: &LoadedAgent, overrides: &ProjectOverrides, llm: &LlmSettings) -> Resolved {
    // A Claude Code subagent is forced to claude at load (`defs`'s "there is no second answer"),
    // so an override must not move it. Answered here rather than at the call sites so no caller
    // can forget, and stated as a *scope* test rather than by comparing harnesses, because a
    // cide-scope role that happens to name claude is perfectly overridable.
    let locked = agent.def.scope.is_claude_code();
    let empty = AgentOverride::default();
    let over: &AgentOverride = match locked {
        true => &empty,
        false => overrides.for_role(agent.id().as_str()),
    };

    let harness = over.harness.unwrap_or(agent.def.harness);
    let effort = over.effort.clone().or_else(|| agent.effort.clone());
    let max_concurrent = over.max_concurrent.unwrap_or(agent.def.max_concurrent);

    // A pool is opencode's alone: no other harness takes a `provider/model`, and applying one
    // would hand a CLI an id it does not parse.
    let wants_pool = harness == Harness::Opencode;

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
    over.pool.as_deref().is_some_and(|name| !name.is_empty()) && harness != Harness::Opencode
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
            assert!(out.refusal.is_none(), "{scope:?}");
            assert!(
                inert_pool(&agent, &overrides),
                "and the screen says the pool does nothing"
            );
        }
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
}
