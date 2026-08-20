//! Newtype identifiers.
//!
//! Most of these are UUID-backed and serialise as plain strings, so the TypeScript side sees
//! `type PaneId = string` — distinct nominal types in Rust, cheap on the wire.
//!
//! Three are **not** uuid-backed — [`WindowLabel`], [`AgentId`] and [`TaskId`] — and each says
//! at length why. The rule they share, and the one to apply to the next id added here: an id a
//! *person* writes, or that a *model* has to quote back verbatim, cannot be a uuid. Everything
//! cide mints for itself can and should be.
//!
//! `SessionId` is special: for Claude panes it **is** the value passed to
//! `claude --session-id`, which is why restoring a workspace needs no extra bookkeeping
//! to map a pane back to its conversation.

use serde::{Deserialize, Serialize};
use std::fmt;
use ts_rs::TS;
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
        #[serde(transparent)]
        #[ts(export, type = "string")]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }

        /// Parse one back from its `Display` form.
        ///
        /// Needed because these ids make round trips outside our own types: a Claude Code
        /// hook reports `session_id` as a string, and the IDE protocol names panes the same
        /// way. Parsing rather than accepting the string keeps an id that we did not mint —
        /// a `claude` a user started by hand in a cide shell — from being treated as a
        /// session this app owns.
        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.parse::<Uuid>()?))
            }
        }
    };
}

uuid_id!(
    /// One opened project (a header tab, or a window in `PerProject` mode).
    ProjectId
);
uuid_id!(
    /// One workspace tab inside a project.
    TabId
);
uuid_id!(
    /// One leaf in a tab's pane tree.
    PaneId
);
uuid_id!(
    /// One interior node of a pane tree; carries the split ratio.
    SplitId
);
uuid_id!(
    /// A PTY-backed session. For Claude panes this is the `--session-id` uuid.
    SessionId
);
uuid_id!(
    /// One git repository root within a (possibly multi-root) project.
    RepoId
);
uuid_id!(
    /// A live attachment of a webview sink to a session.
    AttachmentId
);
uuid_id!(
    /// One dispatched subagent execution. (M18)
    ///
    /// A uuid because **cide mints it and no human ever types it** — the exact opposite of
    /// [`AgentId`] and [`TaskId`] below, which are short strings precisely because people and
    /// models do.
    ///
    /// # Why this is not a [`SessionId`]
    ///
    /// A run reuses the whole session machinery — `SpawnSpec` → `PtySession::spawn` →
    /// `SessionRegistry::insert` — so the temptation to key it by the session it will own is
    /// real, and it is wrong at both ends of the run's life.
    ///
    /// A run exists **before** its child does: it is dispatched into a queue, where it already
    /// has to be listable, cancellable and attributable to a task. Keyed by its session, a
    /// queued run would have no identity at all — nothing for the panel to draw a row for and
    /// nothing for a cancel to name.
    ///
    /// And a run goes on meaning something **after** the child dies: its exit code, the task it
    /// was dispatched against, whether its turn was frozen mid-flight. The session registry
    /// reaps a dead child, so a session-keyed run would vanish the moment it finished, taking
    /// the one row a user actually wants to read — the one that just failed — with it.
    ///
    /// So `AgentRun` carries both: this id for the run's whole life, and
    /// `session: Option<SessionId>` for the part of it that has a process.
    RunId
);
uuid_id!(
    /// One tab of the Git tool window's Log surface. (M18)
    ///
    /// A uuid on the same rule the header states: cide mints it, no person ever writes it, and
    /// nothing outside cide has to quote it back.
    ///
    /// # Why a history tab needs an identity of its own
    ///
    /// The tempting key is the thing the tab is *about* — the repository, or the path whose
    /// history it shows. Both fail on the gesture the surface exists for. Two tabs on the same
    /// file with different filters (one following renames, one not; one on `main`, one on
    /// `HEAD`) is the ordinary way to compare two readings of a history, and a key derived from
    /// the subject makes the second tab replace the first. A whole-repository tab has no path to
    /// key on at all, and every one of them would collide.
    ///
    /// Deliberately **not** a [`TabId`]. A workspace tab is a pane host in the tab strip; this
    /// is a row of chips inside one panel at the bottom of a window, it is never focused,
    /// detached, split or restored the way a workspace tab is, and sharing the type would invite
    /// exactly one of those operations to be handed one of these.
    HistoryTabId
);
uuid_id!(
    /// One tab of the tool window's own tab bar — Log, Console, whatever M18 adds beside them.
    /// (M18)
    ///
    /// Separate from [`HistoryTabId`] because they name two different levels of the same panel:
    /// this is *which surface the tool window is showing*, and a `HistoryTabId` is *which
    /// history the Log surface is showing*. One tool tab can hold many history tabs, so a single
    /// id space would make the containment relation unrepresentable and let a `close` for one
    /// level be handed an id from the other — which type-checks, does nothing, and is silent.
    ToolTabId
);

/// A Tauri window label, e.g. `shell:<uuid>` or `pane:<uuid>`.
///
/// Kept as a string rather than a uuid newtype because Tauri itself keys windows by label
/// and `tauri-plugin-window-state`'s `map_label` collapses the prefixes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export, type = "string")]
pub struct WindowLabel(pub String);

impl WindowLabel {
    pub fn shell() -> Self {
        Self(format!("shell:{}", Uuid::new_v4()))
    }

    pub fn detached_pane() -> Self {
        Self(format!("pane:{}", Uuid::new_v4()))
    }

    pub fn detached_tab() -> Self {
        Self(format!("tab:{}", Uuid::new_v4()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The stable prefix used by `tauri-plugin-window-state`'s `map_label`, so per-window
    /// uuid labels don't accumulate geometry entries forever.
    pub fn state_key(&self) -> &str {
        state_key_of(&self.0)
    }
}

/// The borrowing form of [`WindowLabel::state_key`].
///
/// `tauri-plugin-window-state::map_label` hands out a `&str` and wants a `&str` back with
/// the same lifetime, so it cannot go through an owned `WindowLabel`.
pub fn state_key_of(label: &str) -> &str {
    match label.split_once(':') {
        Some((prefix, _)) => prefix,
        None => label,
    }
}

impl fmt::Display for WindowLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A subagent role name — `developer`, `qa`, `artist`. (M18)
///
/// A string, and not a uuid, because **the user writes it themselves in a file in their own
/// repository**: it is the `name:` key of `.cide/agents/<name>.md`, it is what they type into a
/// dispatch, and it is what the orchestrator names when it hands work over. A uuid is
/// unwritable in every one of those positions.
///
/// The generated-id alternative — cide mints an id and the file carries a display name — was
/// considered and lost for a sharper reason than ergonomics: it would mean the definition file
/// had to be **round-tripped through cide once before it meant anything**, because until cide
/// had read it and written an id back, nothing else could refer to that role. A committed
/// config file that does nothing until the app has rewritten it is not a file a team can
/// review in a pull request, which is the whole point of putting it in the repository.
///
/// Deliberately not validated here. This crate is the wire shape; `cide-agents` is where a name
/// is checked against its file stem and where a duplicate across the project and global
/// directories is reported, because that is the layer that knows the path and the line number
/// to name in the error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export, type = "string")]
pub struct AgentId(pub String);

impl AgentId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AgentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for AgentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// One task in `.cide/tasks.json`, as a short string — `t-17`. (M18)
///
/// The shortness is the design, not a saving. **Agents quote task ids inside prompts and
/// comments**, and that is a position where a uuid fails three separate ways: it costs tokens
/// on every mention; it gets truncated or a digit transposed by a model that is *paraphrasing*
/// a task rather than copying an identifier; and once mangled it cannot be matched back to
/// anything, so the reference is silently lost with no error raised anywhere. `t-17` survives
/// all three, and a human reading the file in a diff can hold it in their head while they read
/// the next hunk.
///
/// Minted by Rust from the **file's own high-water mark**, never from the length of the list.
/// The two agree only until something is deleted, and after that a length-derived id recycles:
/// a second `t-17` would silently re-point every comment, every prompt and every commit message
/// that ever named the first one. So an id is stable for the life of the file and is never
/// reused, which is also what lets a task be quoted somewhere cide cannot see.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export, type = "string")]
pub struct TaskId(pub String);

/// One comment's identity within a task. (M21)
///
/// # Not shaped like [`TaskId`], deliberately
///
/// A task id is `t-17` because a *model* has to quote it in prose and a human has to hold it in
/// their head while reading a diff. Nothing quotes a comment id: it is minted by the panel's own
/// edit gesture and travels straight back to Rust, so the properties that shaped `t-17` — short,
/// memorable, survives paraphrase — buy nothing, and the property that matters instead is that
/// two writers never collide.
///
/// A **string** rather than a `Uuid`, because two kinds of value live here and only one of them
/// is a uuid. A comment written by this build gets `Uuid::new_v4`. A comment already in a
/// `.cide/tasks.json` gets a derived `legacy-<hash>` from `TaskComment::legacy_id`, which is what
/// lets the union rule change from structural equality to id equality without touching a single
/// existing file. Making this a `Uuid` would mean either rewriting every task file on first read
/// or minting a fresh id per read — and a fresh id per read is a merge that duplicates every
/// comment it touches.
///
/// `Default` is the empty string, which is what `#[serde(default)]` yields for a comment loaded
/// from a file written before this existed. `TasksStore`'s repairing loader fills those in; an
/// empty id must never reach the merge, and `repair` is the seam that guarantees it.
#[derive(
    Debug, Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export, type = "string")]
pub struct CommentId(pub String);

impl CommentId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// An id that never reached [`crate::TasksStore`]'s repair, which must not be merged on.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for CommentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TaskId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_values() {
        assert_ne!(PaneId::new(), PaneId::new());
    }

    #[test]
    fn ids_round_trip_as_bare_strings() {
        let id = SessionId::new();
        let json = serde_json::to_string(&id).unwrap();
        // `transparent` means no wrapper object — the wire sees just the uuid.
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<SessionId>(&json).unwrap(), id);
    }

    #[test]
    fn the_string_ids_are_bare_strings_on_the_wire() {
        // `t-17` has to survive being read out of a model's prose and put back on the wire
        // unchanged; a wrapper object would mean every quote of it needed unquoting first.
        let id = TaskId("t-17".into());
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"t-17\"");
        assert_eq!(
            serde_json::from_str::<AgentId>("\"developer\"")
                .unwrap()
                .as_str(),
            "developer"
        );
    }

    #[test]
    fn window_state_key_collapses_the_uuid() {
        assert_eq!(WindowLabel::shell().state_key(), "shell");
        assert_eq!(WindowLabel::detached_pane().state_key(), "pane");
        assert_eq!(WindowLabel("main".into()).state_key(), "main");
    }
}
