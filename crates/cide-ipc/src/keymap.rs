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
    ///
    /// Skipped when absent, and that is about the *file* rather than the wire: Settings →
    /// Keymap rewrites `keymap.json` whole on every edit, and without this every entry a user
    /// had hand-written would come back decorated with `"when": null, "args": null`. The wire
    /// is unaffected — `ts-rs` types this from the Rust type, not from the serde attribute, so
    /// `codegen --check` stays green and the frontend still reads `when` as `string | null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Opaque payload handed to the command.
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// One change Settings → Keymap wants made to `keymap.json`.
///
/// # Why an edit and not a file
///
/// The obvious command shape is "here is the new `Vec<Binding>`, write it". It is also the one
/// that loses this file its whole purpose. `keymap.json` holds **overrides only** — the
/// defaults are compiled in and layered underneath — so a screen that serialised what it is
/// showing would write every shipped binding into the user's file, and from that moment
/// every future change to a default would be frozen out for anyone who had ever touched a
/// binding. Naming the *change* is what keeps the file a diff.
///
/// The second reason is arithmetic. One gesture is routinely two file entries: moving a
/// command off a default chord needs a removal on the old key **and** an add on the new one,
/// and the removal has to carry the `when` of the binding it is removing or it matches
/// nothing (`cide_core::keymap::apply_layer`). Working that out is `cide_core::keymap`'s job,
/// where it is a pure function over a `Vec<Binding>` with tests; a frontend that assembled the
/// entries itself would be a second implementation of layering.
///
/// Applied in order to one loaded vector and written once, so "bind this and unbind whatever
/// held the chord" is a single atomic edit rather than two saves with a broken state between.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum KeymapEdit {
    /// Put `command` on `key` in context `when`, taking it off whatever it was on.
    ///
    /// `when` is copied verbatim from the row the user clicked — never re-derived. A resolved
    /// binding already carries its normalised context, and that string *is* the recipe: a
    /// removal matches on (key, command, `when`), so a `when` that is reconstructed rather
    /// than copied is how an edit silently removes nothing.
    Rebind {
        command: String,
        when: Option<String>,
        key: String,
    },
    /// Take `command` off the keyboard in context `when`, leaving it palette-only.
    Unbind {
        command: String,
        when: Option<String>,
    },
    /// Drop every user entry targeting `command` in context `when`.
    ///
    /// Not "write the default back in": the default is compiled in and re-appears the moment
    /// nothing in the user layer is standing on it. Writing it in would freeze today's default
    /// into the user's file, which is the same mistake as writing the whole table.
    Reset {
        command: String,
        when: Option<String>,
    },
    /// Truncate the file to `[]`. The one destructive gesture on the screen.
    ResetAll,
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
    /// Extra words the palette should match this command on, beyond its title and its id.
    ///
    /// Not synonyms for their own sake. Every entry here answers a phrase somebody actually
    /// typed and got "No matching commands" for: *New branch…* is what the app calls it and
    /// **create** is what the user calls it, and the scorer's five title/id tiers cannot bridge
    /// that — the needle is not a prefix, not a word of the title, not a substring, not in the
    /// id, and (being longer than the title) not even a subsequence of it.
    ///
    /// Kept out of the title rather than folded into it, because the title is the label a user
    /// reads back to check they picked the right row, and "New branch… (create, make)" is a
    /// worse label. Ranked below every title and id tier for the same reason: a keyword hit is
    /// a guess about vocabulary, and a title hit is not.
    pub keywords: Vec<String>,
    /// Why this command cannot run *in any context*, or `None` when it can.
    ///
    /// Distinct from [`when`](Self::when), and the distinction is the whole reason this
    /// field exists. A `when` clause says "not right now, because of where you are" — focus
    /// a Claude pane and the command works. This says "not in this build, whatever you do":
    /// the capability it needs does not exist yet. The palette draws these rows greyed with
    /// the reason instead of hiding them, because a user who is looking for *Restart Claude
    /// session* and finds nothing concludes the app cannot do it and stops looking; a
    /// disabled row with a reason tells them what is actually true.
    ///
    /// The alternative was to delete such commands from the registry until they work. That
    /// loses twice: the id stops being reserved (so a keymap.json naming it becomes an
    /// error rather than a no-op), and nothing anywhere records that the gap exists — which
    /// is how ~35 dead entries came to be listed in the first place.
    ///
    /// Nothing may bind a key to one of these; `cide_core::commands` has a test.
    pub unavailable: Option<String>,
}

impl Command {
    pub fn new(id: impl Into<String>, title: impl Into<String>, group: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            group: group.into(),
            when: None,
            keywords: Vec::new(),
            unavailable: None,
        }
    }

    pub fn when(mut self, expr: impl Into<String>) -> Self {
        self.when = Some(expr.into());
        self
    }

    /// Words the palette matches on besides the title and the id. See [`keywords`](Self::keywords).
    pub fn keywords(mut self, words: &[&str]) -> Self {
        self.keywords = words.iter().map(|word| (*word).to_string()).collect();
        self
    }

    /// Mark the command listable but not runnable, with the reason a user will read.
    ///
    /// Phrase `reason` as a sentence fragment naming what is missing — it is rendered
    /// verbatim in the palette next to the row.
    pub fn unavailable(mut self, reason: impl Into<String>) -> Self {
        self.unavailable = Some(reason.into());
        self
    }
}
