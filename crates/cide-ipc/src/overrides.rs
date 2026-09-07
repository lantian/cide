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
//! declares — it cannot supply a prompt, a tool list or a permission mode, because those are the
//! part of a role that must be reviewable. `cide_agents::defs` remains the only thing that says
//! what a role *is*.

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
    pub projects: std::collections::BTreeMap<String, ProjectOverrides>,
}

impl Default for AgentOverrides {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            projects: std::collections::BTreeMap::new(),
        }
    }
}

impl AgentOverrides {
    /// One project's overrides, or the empty set. Never `None`: a project nobody has overridden
    /// is the ordinary state, and it means the same as a project with an empty table.
    #[must_use]
    pub fn project(&self, root: &str) -> ProjectOverrides {
        self.projects.get(root).cloned().unwrap_or_default()
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
}
