//! Newtype identifiers.
//!
//! These are all UUID-backed and serialise as plain strings, so the TypeScript side sees
//! `type PaneId = string` — distinct nominal types in Rust, cheap on the wire.
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
    fn window_state_key_collapses_the_uuid() {
        assert_eq!(WindowLabel::shell().state_key(), "shell");
        assert_eq!(WindowLabel::detached_pane().state_key(), "pane");
        assert_eq!(WindowLabel("main".into()).state_key(), "main");
    }
}
