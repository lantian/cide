//! Local, uncommitted redirection of a role: which harness it runs on here, and on what. (M45)
//!
//! # Why this layer exists rather than a key in the role's file
//!
//! A role is a **committed** file. `.cide/agents/<id>.md` is reviewed with the code and cloned
//! with the repository, and that is the whole point of it: the team agrees what a role is. But
//! which *provider* one person's machine can reach, and which pool of models they are willing to
//! spend, is not a property of the repository at all — it is a property of that person's laptop
//! and their credentials.
//!
//! Putting a `pool:` key in the role file would have made a teammate's clone name a pool they do
//! not have, on every dispatch, for ever. So the redirection lives here instead: per project, per
//! role, in the profile's own config directory, and never in the checkout. A teammate cloning the
//! repository sees a role with its committed harness and nothing else, which is exactly right.
//!
//! # What this is not
//!
//! Not a second role format. An override may only *redirect* what the definition already
//! declares. It cannot supply a prompt or a tool list, because those are the part of a role that
//! must be reviewable. `cide_agents::defs` remains the only thing that says what a role *is*.
//!
//! The **permission mode** is the exception (M82), and the reason it may be overridden is the
//! reason the pool may. How much an unattended process is trusted to do *on this machine* is the
//! decision of the person whose machine it is. A committed file cannot make it for them, and it
//! must not be able to make it for a teammate. The two-acts rule for `bypassPermissions`
//! (`AgentsConfig::allow_dangerous_permissions`) guards a mode that *arrives* in a committed
//! file with a `git pull`. An override arrives by nobody's hand but the user's, so it does not
//! pass through that gate.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The schema version of the override file, so a later shape can be migrated rather than guessed.
pub const SCHEMA_VERSION: u32 = 1;

/// One redirection. Every field optional; `None` means *say nothing and let the layer below
/// decide*.
///
/// A struct of options rather than an enum of "what to change", because the fields are
/// independent: overriding the harness and leaving the model alone is as ordinary as the reverse,
/// and an enum would have to enumerate the combinations.
/// # `skip_serializing_if` on every field, and it is not tidiness
///
/// `#[ts(optional)]` changes the *TypeScript type* — `pool?: string` — and nothing about serde,
/// which serialises `None` as `null` by default. The screen derives which mode a row is in from
/// whether a field is set, so a cleared field coming back as `null` rather than absent is a
/// control that can never be returned to its "leave this alone" position: `null !== undefined`,
/// so the row reads as still overridden for ever. Absent on the wire is the only spelling that
/// round-trips through `#[serde(default)]` back to `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AgentOverride {
    /// Run this role on a different CLI here.
    ///
    /// **Refused for a Claude Code scope**, and that is a rule with a reason rather than a
    /// limitation: `cide_agents::defs` forces `Harness::Claude` for a `.claude/agents/` role
    /// because "a Claude Code subagent runs under Claude Code; there is no second answer". An
    /// override that contradicted it would be reversing a stated rule silently.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub harness: Option<crate::Harness>,
    /// Run it against this pool, falling down the list as providers refuse.
    ///
    /// Only meaningful when the **effective** harness is opencode. Mutually exclusive with
    /// [`Self::model`]: naming both would be asking for one model and a list of models at once,
    /// and the screen offers one control with two modes rather than two fields that can disagree.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pool: Option<String>,
    /// Run it against this one `provider/model`, without a pool.
    ///
    /// The quick way to try something. See [`Self::pool`] for why only one of the two may be set.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    /// The effort/variant this role runs at here.
    ///
    /// Note a pool *entry* may carry its own variant, which wins over this — see
    /// `cide_ipc::PoolEntry::variant`. So this reaches entries that name none, and roles running
    /// without a pool at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort: Option<String>,
    /// How many of this role may be live at once, here.
    ///
    /// Useful in the direction the committed file cannot know about: more parallelism against a
    /// local model that costs nothing, less against a metered one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_concurrent: Option<u16>,
    /// The permission mode this role runs under here, in claude's vocabulary
    /// (`cide_agents::defs::PERMISSION_MODES`). (M82)
    ///
    /// Beats the role's own `permission-mode:` and the project's `agents.permissionMode`, and is
    /// folded into the role before any harness reads it, so each harness maps it exactly as it
    /// maps a mode the role wrote. The module header argues why this one field of a role's
    /// behaviour may be overridden.
    ///
    /// An unknown word is **ignored with a warning** rather than refused. An override is a
    /// convenience, and `cide_agents::overrides::resolve` refuses nothing but a missing pool.
    /// The role then runs on what its file says.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub permission_mode: Option<String>,
}

impl AgentOverride {
    /// Does this override actually say anything?
    ///
    /// An override that says nothing is stored rather than dropped — the row exists because the
    /// user opened it, and `LlmSettings::cleaned`'s lesson applies here too: a value the backend
    /// refuses to keep is a row the screen can never create. It is *resolution* that skips it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.harness.is_none()
            && self.pool.is_none()
            && self.model.is_none()
            && self.effort.is_none()
            && self.max_concurrent.is_none()
            && self.permission_mode.is_none()
    }
}

/// One project's overrides: a default for every role, plus the roles that differ.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProjectOverrides {
    /// Applied to every role in the project that does not name its own.
    ///
    /// The common case by a distance — "everything here runs on opencode against my cheap pool"
    /// is one row, where a per-role table would be one row per role and would silently miss the
    /// next role somebody adds.
    pub all: AgentOverride,
    /// Per role, by [`crate::AgentId`]'s string. Beats [`Self::all`] field by field.
    ///
    /// A map and not a list because the key is cide's here, not the user's: it is a role id that
    /// already exists, chosen from a menu rather than typed, so the argument
    /// `LlmSettings::providers` makes for a `Vec` does not apply.
    pub roles: std::collections::BTreeMap<String, AgentOverride>,
}

impl ProjectOverrides {
    /// The override that applies to one role: its own, or the project-wide default.
    ///
    /// **Not a merge of the two.** A role's row replaces the project default outright, because a
    /// half-merge is the shape nobody can predict: a user who set `all` to opencode+pool and then
    /// gave one role `harness: claude` means that role runs on claude *without* the pool, and a
    /// field-by-field merge would hand it a pool its harness cannot use.
    #[must_use]
    pub fn for_role(&self, role: &str) -> &AgentOverride {
        self.roles.get(role).unwrap_or(&self.all)
    }
}

/// Every project's overrides, as the file on disk holds them. (M45)
///
/// Keyed by **project root path**, which carries one known cost worth stating rather than
/// discovering: moving or renaming a project loses its overrides. The alternative — a project id
/// — is worse, because ids are minted per workspace and the file outlives any one of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AgentOverrides {
    pub version: u32,
    /// Absolute project root → that project's overrides.
    ///
    /// This is the **live** table, and the only one anything resolves against. Profiles
    /// ([`Self::profiles`]) are stored beside it and *copied into* it on a switch, so the fork,
    /// the queue's caps, the roster and `ChildSettings` read exactly what they read before
    /// profiles existed — a second place a run could take its harness from would be the
    /// "computed here, obeyed nowhere" split `cide_agents::overrides` keeps paying for.
    pub projects: std::collections::BTreeMap<String, ProjectOverrides>,
    /// Absolute project root → that project's named override profiles.
    ///
    /// Added without a schema bump: `#[serde(default)]` loads an older file unchanged. A build
    /// from before profiles *reading* a newer file drops this field on its next save — the live
    /// table survives, the saved profiles do not — which is the accepted cost of not refusing
    /// the file outright.
    ///
    /// `#[ts(skip)]`: the webview never reads the whole file — it is answered one project at a
    /// time through `OverrideProfilesState`.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[ts(skip)]
    pub profiles: std::collections::BTreeMap<String, ProjectProfiles>,
}

impl Default for AgentOverrides {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            projects: std::collections::BTreeMap::new(),
            profiles: std::collections::BTreeMap::new(),
        }
    }
}

/// One project's named override tables, and which of them is live. (M123)
///
/// # Why whole tables, not per-row presets
///
/// The gesture this serves is "codex is out of quota; move this project onto something else,
/// and back tomorrow". That touches the project-wide row *and* every role row that named codex
/// with its own model and effort, so the unit that switches is the whole [`ProjectOverrides`].
/// A per-row preset would leave the user re-picking a preset on every row, which is the chore.
///
/// # The active profile is the live table
///
/// While a profile is active, every edit to the live table is mirrored into it
/// ([`AgentOverrides::set_project`]), so switching away never loses a change and there is no
/// "unsaved" state for the screen to draw. The alternative — a snapshot saved on demand — was
/// refused: a switch that silently discards the afternoon's edits is the failure a person only
/// finds out about when they switch back.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProjectProfiles {
    /// The profile the live table belongs to, or `None` when it belongs to none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active: Option<String>,
    /// Name → table.
    pub profiles: std::collections::BTreeMap<String, ProjectOverrides>,
}

/// What a caller does to a project's profiles. One command carries all of them, for
/// `agent_overrides_set`'s reason: a single small file, rewritten whole, needs no verb per field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "op")]
#[ts(export)]
pub enum OverrideProfileOp {
    /// Answer the state, change nothing.
    List,
    /// Make `name` live, or with `None` detach the live table from any profile (it keeps its rows).
    Switch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        name: Option<String>,
    },
    /// Store the live table under `name` — replacing one of that name — and make it active.
    Save { name: String },
    /// Forget `name`. The live table is untouched, even when `name` was the active one.
    Delete { name: String },
    /// Rename `from` to `to`, keeping it active if it was.
    Rename { from: String, to: String },
}

/// The answer to an [`OverrideProfileOp`]: the live table and the profiles, so a screen redraws
/// both from one reply rather than guessing what a switch did to the rows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct OverrideProfilesState {
    pub overrides: ProjectOverrides,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active: Option<String>,
    /// Profile names, sorted.
    pub names: Vec<String>,
}

impl AgentOverrides {
    /// One project's overrides, or the empty set. Never `None`: a project nobody has overridden
    /// is the ordinary state, and it means the same as a project with an empty table.
    #[must_use]
    pub fn project(&self, root: &str) -> ProjectOverrides {
        self.projects.get(root).cloned().unwrap_or_default()
    }

    /// One project's profiles, or none.
    #[must_use]
    pub fn profiles_of(&self, root: &str) -> ProjectProfiles {
        self.profiles.get(root).cloned().unwrap_or_default()
    }

    /// The live table and the profiles, as one answer.
    #[must_use]
    pub fn profiles_state(&self, root: &str) -> OverrideProfilesState {
        let profiles = self.profiles_of(root);
        OverrideProfilesState {
            overrides: self.project(root),
            active: profiles.active,
            names: profiles.profiles.into_keys().collect(),
        }
    }

    /// Replace a project's live table — and, while a profile is active, that profile too.
    ///
    /// A project whose whole table is empty is removed rather than stored as an empty object, so
    /// the file does not accumulate a row per project ever opened. The active profile is mirrored
    /// *including* when emptied: a profile whose rows were all cleared is a profile that says
    /// "run what is committed", which is a perfectly good thing to switch to.
    pub fn set_project(&mut self, root: &str, table: ProjectOverrides) {
        if let Some(saved) = self.profiles.get_mut(root)
            && let Some(active) = saved.active.clone()
        {
            saved.profiles.insert(active, table.clone());
        }
        match table.all.is_empty() && table.roles.is_empty() {
            true => {
                self.projects.remove(root);
            }
            false => {
                self.projects.insert(root.to_string(), table);
            }
        }
    }

    /// Apply one profile operation to a project. `Err` is a sentence for a person.
    ///
    /// # Errors
    /// A name that is blank, or names no profile where one must exist, or collides on rename.
    pub fn apply_profile_op(&mut self, root: &str, op: &OverrideProfileOp) -> Result<(), String> {
        let named = |name: &str| -> Result<String, String> {
            let name = name.trim();
            match name.is_empty() {
                true => Err("A profile needs a name.".to_string()),
                false => Ok(name.to_string()),
            }
        };
        // Worked on a copy and written back only on success, so a refused op leaves the file
        // exactly as it found it — not with an empty profile set for this project.
        let live = self.project(root);
        let mut saved = self.profiles_of(root);
        let mut switched_to = None;
        match op {
            OverrideProfileOp::List => {}
            OverrideProfileOp::Switch { name: None } => saved.active = None,
            OverrideProfileOp::Switch { name: Some(name) } => {
                let name = named(name)?;
                let Some(table) = saved.profiles.get(&name).cloned() else {
                    return Err(format!(
                        "This project has no override profile called “{name}”."
                    ));
                };
                // The live table already equals the active profile (every edit is mirrored), so
                // this write is belt and braces for a file edited by hand or by an older build.
                if let Some(active) = saved.active.clone() {
                    saved.profiles.insert(active, live);
                }
                saved.active = Some(name);
                switched_to = Some(table);
            }
            OverrideProfileOp::Save { name } => {
                let name = named(name)?;
                saved.profiles.insert(name.clone(), live);
                saved.active = Some(name);
            }
            OverrideProfileOp::Delete { name } => {
                let name = named(name)?;
                if saved.profiles.remove(&name).is_none() {
                    return Err(format!(
                        "This project has no override profile called “{name}”."
                    ));
                }
                if saved.active.as_deref() == Some(name.as_str()) {
                    saved.active = None;
                }
            }
            OverrideProfileOp::Rename { from, to } => {
                let (from, to) = (named(from)?, named(to)?);
                if from != to {
                    if saved.profiles.contains_key(&to) {
                        return Err(format!("There is already a profile called “{to}”."));
                    }
                    let Some(table) = saved.profiles.remove(&from) else {
                        return Err(format!(
                            "This project has no override profile called “{from}”."
                        ));
                    };
                    saved.profiles.insert(to.clone(), table);
                    if saved.active.as_deref() == Some(from.as_str()) {
                        saved.active = Some(to);
                    }
                }
            }
        }
        match saved.active.is_none() && saved.profiles.is_empty() {
            true => {
                self.profiles.remove(root);
            }
            false => {
                self.profiles.insert(root.to_string(), saved);
            }
        }
        if let Some(table) = switched_to {
            self.set_project(root, table);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cleared field must come back **absent**, not `null`.
    ///
    /// The screen derives a row's mode from whether a field is set, and `null !== undefined` in
    /// TypeScript — so a `null` here is a control the user can move away from "leave as
    /// committed" and never move back. `#[ts(optional)]` alone does not do this: it types the
    /// field optional and leaves serde writing `null`.
    #[test]
    fn a_cleared_field_round_trips_as_absent_not_null() {
        let json = serde_json::to_string(&AgentOverride::default()).expect("serializes");
        assert_eq!(json, "{}", "an unset override says nothing at all");

        let mut over = AgentOverride {
            pool: Some("cheap-first".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&over).expect("serializes"),
            r#"{"pool":"cheap-first"}"#,
            "and a set one names only what it sets"
        );

        // The gesture: pick a pool, then go back to "leave as committed".
        over.pool = None;
        let cleared = serde_json::to_string(&over).expect("serializes");
        assert!(!cleared.contains("pool"), "{cleared}");
        assert_eq!(
            serde_json::from_str::<AgentOverride>(&cleared).expect("parses"),
            AgentOverride::default(),
            "and it round-trips back to saying nothing"
        );
    }

    /// The whole table too, since that is what actually crosses the wire.
    #[test]
    fn an_empty_project_table_carries_no_nulls() {
        let table = ProjectOverrides::default();
        let json = serde_json::to_string(&table).expect("serializes");
        assert!(!json.contains("null"), "{json}");
    }

    fn codex() -> ProjectOverrides {
        ProjectOverrides {
            all: AgentOverride {
                harness: Some(crate::Harness::Codex),
                ..Default::default()
            },
            roles: [(
                "x".to_string(),
                AgentOverride {
                    harness: Some(crate::Harness::Codex),
                    model: Some("astra".into()),
                    effort: Some("high".into()),
                    ..Default::default()
                },
            )]
            .into(),
        }
    }

    fn opencode() -> ProjectOverrides {
        ProjectOverrides {
            all: AgentOverride {
                harness: Some(crate::Harness::Opencode),
                pool: Some("cheap".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// A file written before profiles existed loads, and one with none writes no `profiles` key,
    /// so an older build reading it back sees the shape it always did.
    #[test]
    fn a_file_from_before_profiles_loads_and_round_trips() {
        let old = r#"{"version":1,"projects":{"/p":{"all":{"harness":"codex"},"roles":{}}}}"#;
        let loaded: AgentOverrides = serde_json::from_str(old).expect("parses");
        assert!(loaded.profiles.is_empty());
        assert_eq!(
            loaded.project("/p").all.harness,
            Some(crate::Harness::Codex)
        );
        let json = serde_json::to_string(&loaded).expect("serializes");
        assert!(!json.contains("profiles"), "{json}");
    }

    /// The gesture the feature exists for: codex is out, move onto another set, and back.
    #[test]
    fn switching_swaps_the_live_table_and_keeps_both() {
        let mut file = AgentOverrides::default();
        file.set_project("/p", codex());
        file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Save {
                name: "codex".into(),
            },
        )
        .expect("saves");
        file.set_project("/p", opencode());
        // Editing while "codex" is active edits "codex": save the new rows under their own name
        // *first* is the ordinary order, so undo that edit and do it properly.
        assert_eq!(file.profiles_of("/p").profiles["codex"], opencode());
        file.set_project("/p", codex());
        file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Save {
                name: "opencode".into(),
            },
        )
        .expect("saves");
        file.set_project("/p", opencode());
        assert_eq!(file.profiles_of("/p").profiles["codex"], codex());

        file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Switch {
                name: Some("codex".into()),
            },
        )
        .expect("switches");
        assert_eq!(file.project("/p"), codex());
        assert_eq!(file.profiles_of("/p").active.as_deref(), Some("codex"));
        file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Switch {
                name: Some("opencode".into()),
            },
        )
        .expect("switches");
        assert_eq!(file.project("/p"), opencode());
        let state = file.profiles_state("/p");
        assert_eq!(
            state.names,
            vec!["codex".to_string(), "opencode".to_string()]
        );
    }

    #[test]
    fn an_unknown_profile_is_refused_and_changes_nothing() {
        let mut file = AgentOverrides::default();
        file.set_project("/p", codex());
        let before = file.clone();
        let refused = file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Switch {
                name: Some("nope".into()),
            },
        );
        assert!(refused.is_err());
        assert_eq!(file, before);
        assert!(
            file.apply_profile_op("/p", &OverrideProfileOp::Save { name: "  ".into() })
                .is_err()
        );
    }

    /// Deleting or detaching from the active profile leaves the rows running as they are.
    #[test]
    fn deleting_the_active_profile_keeps_the_live_table() {
        let mut file = AgentOverrides::default();
        file.set_project("/p", codex());
        file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Save {
                name: "codex".into(),
            },
        )
        .expect("saves");
        file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Rename {
                from: "codex".into(),
                to: "c".into(),
            },
        )
        .expect("renames");
        assert_eq!(file.profiles_of("/p").active.as_deref(), Some("c"));
        file.apply_profile_op("/p", &OverrideProfileOp::Delete { name: "c".into() })
            .expect("deletes");
        assert_eq!(file.project("/p"), codex());
        assert!(
            !file.profiles.contains_key("/p"),
            "an empty profile set is not stored"
        );
    }

    /// An emptied active profile is kept: "run what is committed" is a profile worth switching to.
    #[test]
    fn clearing_the_live_table_empties_the_active_profile_without_losing_it() {
        let mut file = AgentOverrides::default();
        file.set_project("/p", codex());
        file.apply_profile_op(
            "/p",
            &OverrideProfileOp::Save {
                name: "codex".into(),
            },
        )
        .expect("saves");
        file.set_project("/p", ProjectOverrides::default());
        assert!(!file.projects.contains_key("/p"));
        assert_eq!(
            file.profiles_of("/p").profiles["codex"],
            ProjectOverrides::default()
        );
    }
}
