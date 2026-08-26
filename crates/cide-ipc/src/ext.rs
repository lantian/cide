//! Extensions, and the marketplaces they come from. (M22)
//!
//! # The unit of distribution is a git repository
//!
//! A *marketplace* is a git repository with a `cide-marketplace.json` at its root listing the
//! extensions it holds. An *extension* is a directory in that repository with a
//! `cide-extension.json` describing what it contributes.
//!
//! Git, and not an archive over HTTP, because cide has never had an HTTP client of its own and a
//! marketplace does not need one: `cide-git` already knows how to reach a remote — including the
//! part that is genuinely hard, which is that libgit2 cannot drive an interactive credential
//! helper, so anything authenticated is done by forking the user's own `git`. A marketplace
//! inherits all of that for free, including the user's `~/.gitconfig`, their SSH agent, their
//! proxy and their `url.*.insteadOf`. It also inherits the property that matters most for
//! *reviewing* one: a marketplace has a history, a diff and a signature, which a tarball on a
//! CDN does not.
//!
//! # Identity is a pair
//!
//! [`ExtensionId`] is what the author wrote in their manifest, so two marketplaces may each ship
//! an extension called `sql`. Nothing here pretends the string alone is unique — every reference
//! is an [`ExtensionRef`], and a collision inside one marketplace is *reported* with both paths
//! rather than resolved by a rule nobody asked for.
//!
//! # What a malformed manifest costs
//!
//! Nothing global, on `cide-agents`' rule and for its reasons: one broken manifest never stops
//! the others loading, a defect greys a row rather than hiding it, every refusal carries a path
//! and — where the finding has one — a line, and what is not understood is refused rather than
//! ignored. [`ExtProblem`] is `AgentProblem`'s shape, moved onto the wire because the Extensions
//! panel is the only thing that can act on it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{ExtensionId, MarketplaceId};
use crate::lang::{LanguageDef, LanguageServerDef};

// ==========================================================================================
// Findings.
// ==========================================================================================

/// Whether a finding stopped something.
///
/// The distinction `cide-agents` had to invent and for the same reason: an unknown manifest key
/// **warns** while an unknown capability **greys the extension**. Drawn identically they would be
/// indistinguishable, and a user would spend an afternoon chasing a yellow line that was never
/// going to be the reason their extension would not load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExtSeverity {
    /// Something was refused: a manifest did not load, or a contribution was dropped.
    Error,
    /// Something was accepted with a note.
    Warning,
}

/// One thing wrong with one manifest.
///
/// `line` is `Option` because not every finding has one: *"these two extensions both claim
/// `.sql`"* is a fact about a pair and belongs to neither's line 12. Inventing a line for it
/// would send the user to an innocent line, which is worse than sending them to the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtProblem {
    /// The file the finding is about. Absolute, as read.
    #[ts(type = "string")]
    pub path: PathBuf,
    /// 1-based line within that file, when the finding has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line: Option<u32>,
    pub severity: ExtSeverity,
    /// One sentence, written for the person who has the file open.
    pub message: String,
}

// ==========================================================================================
// Capabilities.
// ==========================================================================================

/// What an extension is allowed to reach.
///
/// A closed set, and closed is the point: an unknown capability in a manifest greys the extension
/// with the list in the sentence, because a build that silently ignored one would run an
/// extension under *less* restriction than its author declared and believed.
///
/// The gate is at **use**, never at load. An extension asking for [`Capability::ProcessSpawn`]
/// still appears in the panel, still shows what it contributes, and is refused the moment it
/// tries — the rule `cide_agents::dispatch_refusal` follows for `bypassPermissions`, and for its
/// reason: refusing at load makes the row *vanish*, and a user cannot grant a permission to
/// something they cannot see.
///
/// # One spelling, and it is the manifest's
///
/// Every variant carries an explicit `#[serde(rename)]` so the value on the wire is the value an
/// author writes in `cide-extension.json` — `editor:read`, not `editorRead`.
///
/// It was `rename_all = "camelCase"` for exactly as long as it took to run, and the failure is
/// worth recording because nothing in the build could see it. The *manifest* spelling came from
/// [`Capability::as_str`] and the *wire* spelling came from serde, the frontend compared against
/// the manifest one, and so `capabilities.has('editor:read')` was false for every extension ever
/// installed: no worker was told about an open file, and every host request was refused. Two
/// panels drew "open a .sql file" over an open `.sql` file, and the check that should have caught
/// it was comparing the frontend's list against `as_str`'s table — both sides of a two-sided
/// agreement, neither of them the wire.
///
/// So `as_str` is now the *only* spelling, serde is told to use it, and `check-ext.mjs` compares
/// against `generated.ts` — the artefact that actually crosses the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Capability {
    /// Read the text, path and language of files open in the editor, and be told when they
    /// change. Does **not** include reading the disk.
    #[serde(rename = "editor:read")]
    EditorRead,
    /// Contribute decorations, diagnostics and an outline back to an open file.
    #[serde(rename = "editor:write")]
    EditorWrite,
    /// Read files under a project root, gitignore-filtered, through the host. Never absolute
    /// paths, never outside a root — the host resolves and refuses, not the extension.
    #[serde(rename = "fs:read")]
    FsRead,
    /// Ask for git status and history through the host. Read-only; there is no `git:write`, and
    /// there will not be one until something needs it badly enough to argue for it here.
    #[serde(rename = "git:read")]
    GitRead,
    /// Run a process. What a `languageServers` contribution needs, and the one capability whose
    /// grant is a real trust decision — the manifest names a binary and cide runs it against the
    /// user's project.
    #[serde(rename = "process:spawn")]
    ProcessSpawn,
}

impl Capability {
    /// Every capability, in the order the install sheet lists them — widest blast radius last.
    pub const ALL: &'static [Capability] = &[
        Capability::EditorRead,
        Capability::FsRead,
        Capability::GitRead,
        Capability::EditorWrite,
        Capability::ProcessSpawn,
    ];

    /// The manifest spelling, which is also what the panel prints.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EditorRead => "editor:read",
            Self::EditorWrite => "editor:write",
            Self::FsRead => "fs:read",
            Self::GitRead => "git:read",
            Self::ProcessSpawn => "process:spawn",
        }
    }

    /// One line for the install sheet, in the second person.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::EditorRead => "read the files you have open",
            Self::EditorWrite => "add highlighting, problems and an outline to files you have open",
            Self::FsRead => "read files in your project",
            Self::GitRead => "read your git status and history",
            Self::ProcessSpawn => "run programs on your machine",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.as_str() == text)
    }
}

// ==========================================================================================
// Contributions.
// ==========================================================================================

/// Where a contributed panel lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PanelLocation {
    /// A button on the activity rail and a panel in the left sidebar.
    Sidebar,
    /// A tab in the bottom tool window, beside Log and the file histories.
    Bottom,
}

/// A panel an extension contributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PanelDef {
    /// Unique within the extension. Becomes the tail of the rail view id, `ext:<market>.<ext>.<id>`.
    pub id: String,
    /// The tab caption and the rail button's tooltip.
    pub label: String,
    /// A 24×24 SVG path `d` attribute, stroked in `currentColor` — the rail draws nothing else.
    ///
    /// A path string and not markup, and validated against the path grammar at load, because the
    /// rail renders it into an attribute and a manifest that could put markup there could put a
    /// `<script>` there. Optional: a bottom-panel tab has a caption and no icon.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub icon: Option<String>,
    pub location: PanelLocation,
}

/// A command an extension contributes to the palette and the keymap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtCommandDef {
    /// Unique within the extension. The full palette id is `ext.<market>.<ext>.<id>`, and
    /// **command ids are API** — a user's `keymap.json` names them, so this must never be renamed
    /// once shipped.
    pub id: String,
    /// The row's title, in the palette's voice: a verb phrase, sentence case.
    pub title: String,
    /// Extra words the palette's fuzzy match should find this row by.
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// One choice in a [`SettingKind::Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingChoice {
    /// What is stored and what the worker reads.
    pub value: String,
    /// What the Settings page shows. Defaults to `value` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
}

/// What kind of value a setting holds, and therefore what control edits it.
///
/// Four, and the set is closed for the reason [`crate::ext::PanelDef`] draws from a fixed icon
/// table: a setting cide has no control for is a setting the user cannot change, and the answer to
/// a genuinely missing kind is to add one here — where it gets a control, a theme and a check —
/// rather than to open a hole for arbitrary markup.
///
/// The `default` is part of the *kind* rather than a field beside it, because a default has to
/// have the kind's type and a separate field could not be made to. It is also what a setting reads
/// as before the user has ever touched it, so an extension always has a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
#[ts(export)]
pub enum SettingKind {
    Toggle {
        default: bool,
    },
    Text {
        default: String,
        /// Shown in an empty field. Not a default — a placeholder the user never replaces stores
        /// the empty string, which is the distinction a settings screen most often gets wrong.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        placeholder: Option<String>,
    },
    Number {
        default: f64,
        /// Clamped to this band on the way in, wherever the value came from. A hand-edited
        /// `extensions.json` goes through the same coercion the spin control does — a clamp that
        /// lives only in the UI is one `invoke` away from being bypassed, which is the argument
        /// `cide_ipc::settings`' own clamps already make.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        max: Option<f64>,
    },
    Choice {
        default: String,
        choices: Vec<SettingChoice>,
    },
}

/// One setting an extension contributes to the Settings page.
///
/// # Why an extension may have settings at all
///
/// Because the alternative is each one inventing its own. An extension that needed a number would
/// otherwise put a field in its own panel, or read a file of its own, or ask the user to edit
/// JSON — three surfaces cide cannot theme, cannot validate and cannot show in the one place a
/// person looks for *configure the thing*.
///
/// # Global, not per project
///
/// Stored in `extensions.json` beside the grant that installed the extension, on the same argument
/// `cide_ext::config`'s header makes for the install itself: an extension is a *tool*, chosen once
/// and wanted everywhere, and a per-project copy would mean setting it again in every checkout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingDef {
    /// Unique within the extension. What the worker reads it by, and the key it is stored under —
    /// so, like a command id, it is API and must not be renamed once shipped.
    pub id: String,
    /// The control's label. A noun phrase, sentence case.
    pub label: String,
    /// One line under the label. Absent is fine for a setting whose label says everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    pub kind: SettingKind,
}

impl SettingDef {
    /// The value this setting reads as, given whatever was stored for it.
    ///
    /// **Total, and never fails.** A stored value of the wrong type, out of band, or naming a
    /// choice that no longer exists falls back to the default — which is the conservative
    /// direction and the only one that keeps an extension's promise that a setting always has a
    /// value of the declared type. `extensions.json` is a file the user may hand-edit and a
    /// marketplace may change a `choices` list in an update, so both of those are ordinary rather
    /// than exotic.
    ///
    /// The one place a value is decided, so the spin control, the hand edit and the update all get
    /// the same answer.
    #[must_use]
    pub fn coerce(&self, stored: Option<&serde_json::Value>) -> serde_json::Value {
        match &self.kind {
            SettingKind::Toggle { default } => serde_json::Value::Bool(
                stored
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(*default),
            ),
            SettingKind::Text { default, .. } => serde_json::Value::String(
                stored
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(default)
                    .to_string(),
            ),
            SettingKind::Number { default, min, max } => {
                let raw = stored
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(*default);
                // Non-finite before the clamp: `NaN.clamp` panics, and a `NaN` can arrive from a
                // hand-edited file through `serde_json`'s arbitrary-precision path.
                let value = if raw.is_finite() { raw } else { *default };
                let value = min.map_or(value, |low| value.max(low));
                let value = max.map_or(value, |high| value.min(high));
                serde_json::Number::from_f64(value)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number)
            }
            SettingKind::Choice { default, choices } => {
                let wanted = stored.and_then(serde_json::Value::as_str);
                let known = wanted.filter(|value| choices.iter().any(|c| c.value == *value));
                serde_json::Value::String(known.unwrap_or(default).to_string())
            }
        }
    }
}

/// Everything one extension declares.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Contributions {
    pub languages: Vec<LanguageDef>,
    pub language_servers: Vec<LanguageServerDef>,
    pub panels: Vec<PanelDef>,
    pub commands: Vec<ExtCommandDef>,
    pub settings: Vec<SettingDef>,
}

// ==========================================================================================
// Identity, and what is installed.
// ==========================================================================================

/// One extension, named the only way that is unambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ExtensionRef {
    pub marketplace: MarketplaceId,
    pub extension: ExtensionId,
}

impl std::fmt::Display for ExtensionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.marketplace, self.extension)
    }
}

/// Where a contribution came from.
///
/// On the wire because the Extensions panel has to answer *"why is my `.sql` file coloured like
/// that"* without the user reading two manifests. cide ships SQL and YAML grammars of its own, so
/// the ordinary state after installing either extension is that a builtin lost — and a user who
/// cannot see that happen has no way to tell a working extension from an inert one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ContributionSource {
    /// Compiled into this build.
    Builtin,
    /// Contributed by an installed, enabled extension.
    Extension { extension: ExtensionRef },
}

/// A language in the resolved registry, with the source that won.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LanguageBinding {
    pub def: LanguageDef,
    pub source: ContributionSource,
    /// A source this one displaced, if any. Reported rather than silent — see
    /// [`ContributionSource`].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub supersedes: Option<ContributionSource>,
}

/// A language server in the resolved registry, with the source that declared it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerBinding {
    pub def: LanguageServerDef,
    pub source: ContributionSource,
}

/// A panel in the resolved registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PanelBinding {
    /// `ext:<marketplace>.<extension>.<panel>` — the `ActivityView` the rail and the sidebar use.
    pub view: String,
    pub def: PanelDef,
    pub extension: ExtensionRef,
}

/// The merged, live contribution set: what the editor, the LSP supervisor and the rail actually
/// use.
///
/// Rides [`crate::Bootstrap`] rather than being fetched after the window opens, and that is a
/// requirement rather than an optimisation: fold restore runs inside the editor's mount dispatch
/// and cannot await, so a language's [`crate::lang::FoldSpecDto`] has to be present at first
/// paint. See that type's own note.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolvedContributions {
    /// Every language, builtin and contributed, in lookup order.
    pub languages: Vec<LanguageBinding>,
    pub servers: Vec<ServerBinding>,
    pub panels: Vec<PanelBinding>,
    /// Palette rows, already prefixed `ext.<marketplace>.<extension>.<id>`.
    pub commands: Vec<ResolvedExtCommand>,
    /// Contributions dropped because two extensions claimed the same thing. Both are named.
    pub conflicts: Vec<ExtProblem>,
}

/// One extension command, as the palette and the keymap see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolvedExtCommand {
    /// The full, prefixed id — `ext.cide-marketplace.sql.showStatements`.
    pub id: String,
    pub title: String,
    pub keywords: Vec<String>,
    pub extension: ExtensionRef,
}

/// An installed extension.
///
/// `Eq` is deliberately absent from here down: a [`SettingKind::Number`] carries an `f64`, so
/// nothing containing a [`Contributions`] can be `Eq`. `PartialEq` is what every caller actually
/// uses — comparing two snapshots, asserting one in a test — and the difference only matters to a
/// `HashMap` key, which none of these is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstalledExtension {
    #[serde(flatten)]
    pub id: ExtensionRef,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Whether the user has it switched on. A disabled extension keeps its row, its manifest and
    /// its installed files; nothing of it reaches the registry.
    pub enabled: bool,
    /// The marketplace commit this copy was taken at. What an update compares against.
    pub commit: String,
    pub capabilities: Vec<Capability>,
    pub contributes: Contributions,
    /// Every contributed setting's current value, keyed by [`SettingDef::id`].
    ///
    /// **Resolved, not stored**: the defaults with whatever the user changed layered on top, every
    /// one already through [`SettingDef::coerce`]. So a reader — the Settings page, the worker —
    /// never has to know what a default is or what happens when a stored value is the wrong type.
    /// The *stored* half, which is only the differences, is `extensions.json`'s.
    #[ts(type = "Record<string, boolean | string | number>")]
    pub settings: std::collections::BTreeMap<String, serde_json::Value>,
    /// The directory the code was copied into. Shown in the panel; also the jail
    /// `cide-ext://` resolves against.
    #[ts(type = "string")]
    pub path: PathBuf,
    /// The worker entry point, relative to [`Self::path`], or `None` for a purely declarative
    /// extension — a language and a server and nothing else, which is a first-class case and not a
    /// degenerate one.
    ///
    /// On the wire rather than assumed to be `main.js`, because the manifest may name anything and
    /// an extension whose entry is `src/worker.js` would otherwise have `has_worker` true and a
    /// 404 behind it. Refused at load if it leaves the installed directory — the same jail
    /// `cide-ext://` uses, reused rather than restated.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub main: Option<String>,
    /// One sentence, when the extension loaded **greyed**: its manifest parsed but something in
    /// it is not usable. `None` for the ordinary case.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unavailable: Option<String>,
    pub problems: Vec<ExtProblem>,
}

/// A row in a marketplace's index, whether or not it is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarketplaceEntry {
    pub id: ExtensionId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    /// The version installed from this marketplace, when one is.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub installed: Option<String>,
    /// Whether the marketplace's copy declares a `version` different from the installed one —
    /// the only thing an update offer compares, so a commit that ships code without a bump is
    /// deliberately invisible here.
    pub update_available: bool,
    /// One sentence, when this row cannot be installed — a manifest that did not parse, a
    /// capability this build does not know. The row is still drawn; see [`Capability`].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unavailable: Option<String>,
}

/// How a marketplace's clone is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum MarketplaceState {
    /// Connected, cloned, and its index read. `head` is the commit the index was read at.
    Ready { head: String, fetched_at: i64 },
    /// A clone or a fetch is running. Its own state rather than an `Option<…>` because the panel
    /// draws a third thing for it and a spinner over stale rows is a lie about which rows are
    /// current.
    Working { what: String },
    /// The clone directory is gone — a `--fresh` launch, a cleaned `$XDG_STATE_HOME`, a user with
    /// a tidy streak. Recoverable by re-cloning, and distinct from [`Self::Failed`] because
    /// nothing went wrong.
    Missing,
    /// The last clone, fetch or index read failed. `error` is git's own words where there are any:
    /// the user is used to reading them and cide paraphrasing them helps nobody.
    Failed { error: String },
}

/// A connected marketplace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Marketplace {
    pub id: MarketplaceId,
    /// What its own `cide-marketplace.json` calls it, or the source's last path segment before
    /// the index has ever been read.
    pub name: String,
    /// The URL or local path it was connected by, verbatim as the user typed it.
    pub source: String,
    /// Whether the source is reached by forking `git` or by libgit2 — see `cide_ext::market`.
    /// On the wire because it is the difference between *"this will use your credential helper"*
    /// and *"this is a directory on your disk"*, and the user should not have to infer it.
    pub authenticated: bool,
    pub state: MarketplaceState,
    /// The clone directory. Under `$XDG_STATE_HOME`, never in a project.
    #[ts(type = "string")]
    pub path: PathBuf,
    pub entries: Vec<MarketplaceEntry>,
    pub problems: Vec<ExtProblem>,
}

/// Everything the Extensions panel draws, and everything the registry resolved from it.
///
/// One snapshot rather than four calls, on `Bootstrap`'s argument: a panel that has to make
/// several requests before it can paint shows several intermediate states.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionSnapshot {
    /// A monotonic counter, bumped on every accepted mutation.
    ///
    /// Carried because this state has **several writers**: two windows, a background refresh, and
    /// the user running `git pull` in a clone directory by hand. Snapshots can therefore arrive
    /// out of order and the receiver's only defence is a high-water mark — the same argument
    /// `cide://tasks-changed` makes, and the opposite of `cide://agents-changed`, whose roster has
    /// exactly one in-process writer and for which the last one sent is by construction the
    /// newest.
    pub rev: u64,
    pub marketplaces: Vec<Marketplace>,
    pub extensions: Vec<InstalledExtension>,
    pub resolved: ResolvedContributions,
    /// Findings about `extensions.json` itself, which belongs to no marketplace.
    pub problems: Vec<ExtProblem>,
}

/// One extension's page: everything the README tab draws.
///
/// The row *and* the prose, in one answer, because the tab needs both and a page that had to make
/// two calls would draw a header over an empty document for a frame.
///
/// `row` is `None` for an extension its marketplace no longer lists and that is not installed —
/// a tab restored from the reopen stack after the user disconnected. The tab says so; it does not
/// close itself, because a tab that vanished when it was activated would be worse than one that
/// explains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionPage {
    pub id: ExtensionRef,
    /// What the marketplace lists, when it still lists it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub entry: Option<MarketplaceEntry>,
    /// What is on disk, when something is.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub installed: Option<InstalledExtension>,
    /// The README's text, or `None` when the extension ships none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub readme: Option<String>,
    /// The file the README came from, for the tab's tooltip. Absolute.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "string")]
    pub readme_path: Option<PathBuf>,
}

// ==========================================================================================
// Inbound.
// ==========================================================================================

/// Connect a marketplace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ConnectRequest {
    /// A git URL or a local path, as typed. Expanded and normalised in `cide-ext`, not here.
    pub source: String,
}

/// Install, or update, one extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct InstallRequest {
    #[serde(flatten)]
    pub id: ExtensionRef,
    /// The capabilities the user is granting, which must equal the ones the manifest declares.
    ///
    /// Echoed back rather than taken from the manifest, and this is the whole of the consent
    /// mechanism: an install whose granted set does not match what cide is about to read is
    /// refused. Without it a marketplace could add `process:spawn` to a manifest between the sheet
    /// being drawn and the button being pressed, and the user would have approved a different
    /// extension from the one that installed.
    pub granted: Vec<Capability>,
}
