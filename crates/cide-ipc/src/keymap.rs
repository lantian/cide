//! Keybindings and the command registry's wire shape.
//!
//! The file format is VS Code's, deliberately: it is the shape most users already know, it
//! supports unbinding, and its `when` clauses are expressive enough without being a
//! language. `~/.config/cide/keymap.json` holds **overrides only** — defaults are compiled
//! in, and resolution layers them in `cide-core::keymap`.
//!
//! ```json
//! [
//!   { "key": "ctrl+p",       "command": "picker.files" },
//!   { "key": "ctrl+k ctrl+s", "command": "settings.keymap" },
//!   { "key": "ctrl+w",       "command": "-tab.close", "when": "tabPinned" }
//! ]
//! ```

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One binding, in the layered keymap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Binding {
    /// A chord: `ctrl+p`, or a multi-stroke sequence like `ctrl+k ctrl+s`.
    ///
    /// Multi-stroke is why the frontend runs its own prefix state machine — CodeMirror's
    /// `KeyBinding.key` parser handles single chords only.
    pub key: String,
    /// The command id, e.g. `pane.split.right`. A leading `-` **removes** a binding
    /// contributed by an earlier layer, which is how a user disables a default.
    pub command: String,
    /// Context expression, e.g. `terminalFocused && !overlayOpen`. `None` means always.
    pub when: Option<String>,
    /// Opaque payload handed to the command.
    #[ts(type = "unknown")]
    pub args: Option<serde_json::Value>,
}

impl Binding {
    pub fn new(key: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            command: command.into(),
            when: None,
            args: None,
        }
    }

    pub fn when(mut self, expr: impl Into<String>) -> Self {
        self.when = Some(expr.into());
        self
    }

    /// True when this entry removes an earlier binding rather than adding one.
    pub fn is_removal(&self) -> bool {
        self.command.starts_with('-')
    }

    /// The command id with any leading `-` stripped.
    pub fn target_command(&self) -> &str {
        self.command.strip_prefix('-').unwrap_or(&self.command)
    }
}

/// Which layer a resolved binding came from. Surfaced in Settings → Keymap so a user can
/// see at a glance what they have overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum KeymapLayer {
    Default,
    Platform,
    User,
}

/// A binding after layering, as sent to the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolvedBinding {
    pub key: String,
    pub command: String,
    pub when: Option<String>,
    #[ts(type = "unknown")]
    pub args: Option<serde_json::Value>,
    pub layer: KeymapLayer,
}

/// A command the palette can run and a key can be bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Command {
    /// Stable id, e.g. `pane.split.right`. Never localised, never renamed casually — user
    /// keymaps refer to it.
    pub id: String,
    /// Shown in the palette, e.g. `Split pane right`.
    pub title: String,
    /// Right-aligned group in the palette, e.g. `Window`, `Claude`, `Terminal`.
    pub group: String,
    /// Context in which the command is applicable, same grammar as `Binding::when`.
    pub when: Option<String>,
}

impl Command {
    pub fn new(id: impl Into<String>, title: impl Into<String>, group: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            group: group.into(),
            when: None,
        }
    }

    pub fn when(mut self, expr: impl Into<String>) -> Self {
        self.when = Some(expr.into());
        self
    }
}
