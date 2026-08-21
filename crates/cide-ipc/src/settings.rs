//! User settings — the data behind the Settings tab.
//!
//! Field names and defaults track the design mock's Settings screen exactly, including the
//! toggles' default states, so the UI has nothing to invent.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git::PullDefault;
use crate::{Theme, WindowMode};

// No `Eq`: `EditorSettings`/`TerminalSettings` carry an `f32` font size now, and a float has
// no total equality — which is the honest answer rather than an obstacle. Nothing compares
// settings for equality; `PartialEq` is what the tests and the patch path use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Settings {
    /// *Stack projects in the header* vs *One window per project*.
    pub window_mode: WindowMode,
    pub theme: Theme,

    /// Point size for everything that is **not** a buffer or a terminal.
    ///
    /// Beside `theme` rather than in a group of its own, and for the same reason `theme` is a
    /// scalar: it is one number that repaints the whole window, and a one-field group would
    /// buy nothing but a type. [`SettingsPatch`](crate::SettingsPatch) patches per top-level
    /// field, so a scalar also means this is the one font size a caller can move without
    /// first holding the whole group it lives in — the hazard `ui/src/editor/diffViewMode.ts`
    /// works around for [`EditorSettings`].
    ///
    /// It is a *base*, not the size of any particular label. The chrome draws at fourteen
    /// design sizes between 8px and 20px, and this number scales all of them proportionally
    /// through one multiplier — `--ui-scale` in `tokens.css`, which is this over
    /// [`DEFAULT_UI_FONT_SIZE`]. So at 15 the file tree's 13px rows are 15px and its 10.5px
    /// path labels are 12.1px; the ratios the design mock specifies are preserved, which is
    /// what `./run.sh --audit-chrome` measures.
    ///
    /// Deliberately *not* named `zoom` and deliberately not a percentage. Icons, borders and
    /// splitter widths do not follow it, so it is not a zoom level, and calling it one would
    /// promise a uniform scaling this does not do.
    ///
    /// `f32` for [`EditorSettings::font_size`]'s reason — the Settings control steps by half a
    /// pixel and an integer would snap it. Clamped where a patch lands: see
    /// [`clamp_ui_font_size`].
    pub ui_font_size: f32,

    /// "Each project keeps its own Claude tab — pinned, cannot be closed, only its panes
    /// can." Off would mean a project with no console tab, which the rest of the model
    /// does not currently allow; the toggle exists in the mock and is honoured as
    /// read-only-true until there is a coherent meaning for false.
    pub each_project_keeps_claude_tab: bool,

    /// "Reopen the last project on launch — restores its tab strip and pane layout."
    pub reopen_last_project: bool,

    /// "Keep sessions running when a project window is closed."
    ///
    /// Deliberately *not* worded as the mock's "keep a project running when its window is
    /// closed — live Claude sessions survive in the background". cide runs no background
    /// daemon: quitting the app quits its Claude sessions, and the workspace is restored on
    /// relaunch via `claude --resume`. Promising background survival would be a lie.
    pub keep_sessions_on_window_close: bool,

    /// "Confirm before closing a project with a live session." *Live* means a session is
    /// `Busy` or `AwaitingPermission` — not merely that a process exists, which would warn
    /// constantly.
    ///
    /// Read by `cide_app::cmd::app::app_quit_requested`, and by nothing else. Its whole
    /// authority is over the `blocking` half of `QuitDecision`: off, and a close no longer
    /// asks about an agent mid-turn.
    ///
    /// It governs **sessions only**, and specifically not unsaved file tabs, which are
    /// reported whatever it says. The two are different kinds of loss. Interrupting a turn
    /// costs the turn — the conversation is keyed by `SessionId` and resumes with
    /// `claude --resume` — so a user may reasonably decide they do not want to be asked.
    /// Discarding an unsaved buffer costs the work outright, with nothing to resume from,
    /// and a toggle whose label says "live session" must not quietly turn that guard off
    /// too. If unsaved edits ever need their own toggle, that is a second field with its own
    /// wording, not a second meaning stapled to this one.
    pub confirm_close_with_live_session: bool,

    pub editor: EditorSettings,
    pub terminal: TerminalSettings,
    pub graphics: GraphicsSettings,
    pub claude: ClaudeSettings,
    pub proxy: ProxySettings,
    pub sidebar: SidebarSettings,
    pub explorer: ExplorerSettings,
    pub inspections: InspectionSettings,
    pub git: GitSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window_mode: WindowMode::default(),
            theme: Theme::default(),
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            each_project_keeps_claude_tab: true,
            reopen_last_project: true,
            keep_sessions_on_window_close: true,
            confirm_close_with_live_session: false,
            editor: EditorSettings::default(),
            terminal: TerminalSettings::default(),
            graphics: GraphicsSettings::default(),
            claude: ClaudeSettings::default(),
            proxy: ProxySettings::default(),
            sidebar: SidebarSettings::default(),
            explorer: ExplorerSettings::default(),
            inspections: InspectionSettings::default(),
            git: GitSettings::default(),
        }
    }
}

/// What the file tree walks, and therefore what it draws. (M18)
///
/// # Why these are settings and not simply "show everything"
///
/// The reported bug was that cide shows neither dotfiles nor ignored files, and the two halves
/// of it cost three orders of magnitude apart. That difference is the whole reason this is a
/// pair of booleans with different defaults rather than one:
///
/// * Dotfiles are tens of entries in a repository — `.claude`, `.github`, `.gitignore` itself
///   — and every one of them is a project file a user edits. Hiding them is the surprise, so
///   [`Self::show_hidden_files`] defaults **on**.
/// * Ignored files are `target/`, `node_modules/`, `dist/`. On this repository that is roughly
///   200,000 entries against 1,500 tracked ones, and each becomes an arena node in
///   `cide_fs::Index`, a `Ctrl+P` candidate in the picker, and a row the scrollbar spans. A
///   default that made opening a Rust project cost a full `target/` walk would be a default
///   that made cide feel broken, so [`Self::show_ignored_files`] defaults **off**.
///
/// `.git` is not a third field. It is excluded structurally by `cide_fs::filter::Visibility`,
/// which explains why at length; the short version is that it is a database of one loose object
/// per version of every file ever committed, and nothing in a file tree can usefully act on one.
///
/// # Changing either one re-walks the project
///
/// There is no way to patch an existing index into the other answer — the entries were never
/// walked — so `cmd::settings::settings_set` re-indexes every open project when one of these
/// moves. That is a visible cost (a fresh walk, an empty picker for its duration) and it is why
/// these live here, in stored settings, rather than being a per-panel toggle that could be
/// flipped by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ExplorerSettings {
    /// Draw dot-prefixed entries — `.claude`, `.github`, `.env`.
    pub show_hidden_files: bool,
    /// Draw entries the ignore rules cover, tinted the way IDEA tints them.
    ///
    /// The tint is not decoration: with this on, `target/` is in the tree, in the picker and in
    /// the scrollbar's range, and a row that looked like any other would make the tree read as
    /// though the project contained 200,000 source files. `ui/src/sidebar/treeStatus.ts` resolves
    /// it, and it arrives on the same `git_tree_status` map the M/A/D letters do — an ignored
    /// *directory* travels as one entry and its contents inherit it, which is what keeps this
    /// affordable on the wire.
    pub show_ignored_files: bool,
}

impl Default for ExplorerSettings {
    /// Both on.
    ///
    /// `show_ignored_files` shipped `false`, on a cost argument that is entirely true — one
    /// ignore verdict serves the tree, the picker, Find in Files and the symbol index alike, so
    /// turning it on widens all four at once, and on a built Rust project that is a very large
    /// number of extra rows. It was still the wrong default, because the feature exists to
    /// answer "cide doesn't show gitignored files - but should with special highlight like in
    /// idea", and IDEA shows them in its project view with nothing to configure. A feature that
    /// is off until the person who asked for it finds a switch has not been delivered.
    ///
    /// The cost is now a thing a user turns *off* having seen it, which is the direction that
    /// keeps the default honest: the expensive behaviour is the one that was asked for, and the
    /// cheap one is available to anyone who finds it too slow.
    fn default() -> Self {
        Self {
            show_hidden_files: true,
            show_ignored_files: true,
        }
    }
}

/// The narrowest a sidebar panel may be left, in CSS pixels.
///
/// 180 rather than something smaller because both panels stop being *usable* below it
/// rather than merely tight: the explorer indents 12px per depth and draws 21px mono rows,
/// so a file three levels down has about 12 characters left at 180 and none at 140; the git
/// panel's header carries a segmented Commit/Shelf control plus its counts on one 30px line.
/// A panel that can be dragged to a sliver is a panel a user can lose by accident, and there
/// is no "reset width" gesture to get it back with.
pub const SIDEBAR_MIN_WIDTH: u16 = 180;

/// The widest, in CSS pixels. Roughly 1.5x the mock's 420px git panel.
///
/// This is the *stored* ceiling and it is deliberately generous — it exists to keep a
/// corrupt or hand-edited `workspace.json` from producing a panel wider than any monitor,
/// not to decide what fits. What fits depends on the window, which Rust cannot see, so the
/// frontend applies a second ceiling against the live viewport (`clampSidebarWidth` in
/// `ui/src/chrome/sidebarWidth.ts`) before it paints anything. Restoring a 640px panel into
/// a 720px window is therefore safe: it is clamped on the way to the DOM, and the stored
/// value survives for the next time the window is wide enough to honour it.
pub const SIDEBAR_MAX_WIDTH: u16 = 640;

/// How wide the user left each sidebar panel.
///
/// **A width per kind of content, not one for the sidebar.** The mock gives the explorer 252px
/// and the git panel 420px, and that difference is a property of the content, not a stylistic
/// accident: the explorer draws one truncatable name per row, while the git panel draws a path
/// *and* an added/removed figure *and* a stage checkbox on the same line. Sharing a single
/// number would mean every switch between the two views resized the workspace under the user,
/// and whichever panel they had not tuned would be the wrong width — so the resize would feel
/// like it had been forgotten rather than remembered.
///
/// The corollary is that panels whose rows *are* alike share one: [`Self::files_width`] serves
/// the explorer, search and problems, and [`Self::agents_width`] serves both M18 panels.
///
/// Only these are tokens: `--w-sidebar-files` also sizes the search and problems panels (they
/// are the explorer's column with different rows in it), and `--w-sidebar-agents` sizes both
/// M18 panels — which is a decision `tokens.css` already made and this type follows rather than
/// reopens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct SidebarSettings {
    /// `--w-sidebar-files`: the explorer, and with it search and problems.
    pub files_width: u16,
    /// `--w-sidebar-git`.
    pub git_width: u16,
    /// `--w-sidebar-agents`: the Agents panel, and with it Tasks. (M18)
    ///
    /// # Why `#[serde(default = "…")]` is not optional here
    ///
    /// `SidebarSettings` is a **stored** struct. Every existing user's `workspace.json` was
    /// written before this field existed, and serde refuses a missing non-defaulted field — so
    /// without this attribute the whole `sidebar` object fails to deserialise on the first
    /// launch after the upgrade, and with it every setting in the block. The container's
    /// `#[serde(default)]` does not save it either: that supplies a default for *absent
    /// members* of `Settings`, not for a member of this struct that a present `sidebar` object
    /// happens not to mention. [`crate::Pane::conversation`] already had to learn this, and its
    /// note is the one to read before adding the next stored field anywhere in this crate.
    ///
    /// # Why a third width and not [`Self::files_width`]
    ///
    /// Reusing the explorer's number is the tempting move and it is wrong for the same reason
    /// the git panel does not reuse it: 252px is a column of short filenames, one truncatable
    /// name per row. An Agents row carries a role label, a phase, an elapsed figure **and** a
    /// task title on two lines, and a Tasks row carries an id, a title and an agent chip. Git
    /// got 420 on exactly this argument, and 320 is the same argument at this content's width.
    ///
    /// **One number for both new panels**, though, rather than two. They are two views of one
    /// thing — the panel a user widens to read task titles in Agents is the panel they are
    /// about to read task titles in under Tasks — so a user who drags one and finds the other
    /// unchanged has been made to do the same work twice. That is the reverse of the
    /// files/git split, where the two panels are read for different reasons at different times.
    #[serde(default = "default_agents_width")]
    pub agents_width: u16,
    /// `--w-sidebar-ext`: every panel an extension contributes. (M22)
    ///
    /// **One number for every contributed panel**, and that is the one decision here that had a
    /// real alternative. A width per extension would let two extensions with very different
    /// panels each keep their own drag, which is plainly better for the user — and it cannot be
    /// stored here, because this struct is a fixed set of fields and the extension set is not
    /// known until `extensions.json` is read. Storing it per extension means a map keyed by an
    /// identity pair, in a settings object that is broadcast to every window on every change, for
    /// a number most users will never drag once.
    ///
    /// So: one number, defaulting to the explorer's, because a tree of rows is the shape 252px
    /// was chosen for. If a contributed panel ever earns its own width, that is the day to add
    /// the map.
    ///
    /// `#[serde(default = "…")]` for [`Self::agents_width`]'s reason, which is worth re-reading
    /// before adding the next stored field: a missing non-defaulted field makes the whole
    /// `sidebar` object fail to deserialise, taking every setting in the block with it.
    #[serde(default = "default_ext_width")]
    pub ext_width: u16,
}

/// `serde(default)` for [`SidebarSettings::ext_width`].
fn default_ext_width() -> u16 {
    252
}

/// `serde(default)` for [`SidebarSettings::agents_width`]. A bare literal is not a path serde
/// accepts, which is why this exists rather than the number appearing inline.
fn default_agents_width() -> u16 {
    320
}

impl Default for SidebarSettings {
    /// The mock's widths, which are also the literals `tokens.css` ships as the token
    /// values — a fresh workspace and a workspace whose settings failed to load look
    /// identical, which is the point.
    fn default() -> Self {
        Self {
            files_width: 252,
            git_width: 420,
            agents_width: default_agents_width(),
            ext_width: default_ext_width(),
        }
    }
}

impl SidebarSettings {
    /// Every width brought inside [`SIDEBAR_MIN_WIDTH`]..=[`SIDEBAR_MAX_WIDTH`].
    ///
    /// Applied where a patch lands rather than where the workspace is read, so the stored
    /// file converges on a legal value instead of being re-clamped forever on every load.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            files_width: self.files_width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            git_width: self.git_width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            agents_width: self
                .agents_width
                .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            ext_width: self.ext_width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
        }
    }
}

/// Unified rows, or the two sides beside each other.
///
/// Stored rather than held in the pane, because a diff pane is opened and closed constantly
/// — every file in the git panel is a new tab — and a mode that resets with the pane is a
/// mode nobody can stay in. It lives under `editor` rather than `git` because both diff
/// surfaces read it: the git pane and Claude's `openDiff` proposal are the same question
/// asked about different documents.
///
/// **Unified is the default and that is deliberate.** It is what the pane has always shown,
/// it is the shape the per-line staging selection is expressed in (one row is one
/// `hunk:line`), and it is the only one that survives a narrow pane — see `DIFF_SPLIT_MIN_PX`
/// in `ui/src/panes/GitDiffPane.tsx`, which falls back to it rather than offering a split
/// nobody can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DiffView {
    #[default]
    Unified,
    Split,
}

/// The mock's mono size, and the default for both code surfaces.
///
/// One constant because they default equal — see [`TerminalSettings::font_size`]. It is also
/// the literal in `tokens.css`'s `--fs-code`/`--fs-term`, and `check-fonts.mjs` fails if the
/// two disagree, because a mismatch means the first settings write silently restyles the app.
pub const DEFAULT_CODE_FONT_SIZE: f32 = 12.5;

/// The band a stored font size is held inside.
///
/// Enforced in Rust as well as in the frontend's number input, because `settings.json` is a
/// file a user can edit and a zero reaches xterm as a zero-wide cell — a blank pane with no
/// error, which is indistinguishable from every other way a pane comes up blank.
pub const MIN_CODE_FONT_SIZE: f32 = 6.0;
pub const MAX_CODE_FONT_SIZE: f32 = 40.0;

/// Bring a font size inside the band, and turn a non-finite one into the default.
///
/// `NaN` deserves the explicit arm: it compares false against every bound, so a naive
/// `clamp` propagates it, and `NaN` px makes every cell in the pane `NaN` wide.
#[must_use]
pub fn clamp_font_size(size: f32) -> f32 {
    if !size.is_finite() {
        return DEFAULT_CODE_FONT_SIZE;
    }
    size.clamp(MIN_CODE_FONT_SIZE, MAX_CODE_FONT_SIZE)
}

/// The chrome's base size, and the one everything that is not a buffer or a terminal follows.
///
/// This is the literal `tokens.css` has always carried on `html, body`, so the default is the
/// size the app already drew before there was a setting — which is why [`Settings`] can take
/// this field on `#[serde(default)]` with no `persist.rs` migration behind it. A defaulted
/// field is an *inference* whenever the default is a behaviour (see [`EditorSettings::autosave`]
/// for the case that needed writing down); here the default is the status quo, so an existing
/// workspace that has never seen this field renders exactly as it did.
///
/// The other three copies of this number are `UI_BASE_FONT_SIZE` in `ui/src/settings/fontScale.ts`,
/// `--fs-ui-13` in `ui/src/styles/tokens.css`, and the divisor in `ui/public/theme-boot.js`.
/// `check-ui-scale.mjs` pins all four together, for [`DEFAULT_CODE_FONT_SIZE`]'s reason: a
/// mismatch means the first settings write silently restyles the app.
pub const DEFAULT_UI_FONT_SIZE: f32 = 13.0;

/// The band the chrome size is held inside, and it is much narrower than the code band.
///
/// Not 6..=40, because the two are not the same kind of number. A code size governs one
/// surface that scrolls; this one multiplies *every* chrome size at once, including the 8px
/// pin chip and the 34px header. Below 9 that chip is under 6px of glyph and the tab strip's
/// close buttons are not hittable; above 20 the header alone is over 52px and the header,
/// tab strip and status bar together take a fifth of a 1080p window before any content.
pub const MIN_UI_FONT_SIZE: f32 = 9.0;
pub const MAX_UI_FONT_SIZE: f32 = 20.0;

/// Bring a chrome size inside the band. Same shape, and same `NaN` arm, as [`clamp_font_size`].
///
/// The `NaN` arm matters more here than there: this number reaches CSS as a *divisor*, and a
/// `NaN` scale makes every `calc()` in the ladder invalid, which drops the declaration and
/// leaves 405 rules with no `font-size` at all — a whole window of UA-default serif.
#[must_use]
pub fn clamp_ui_font_size(size: f32) -> f32 {
    if !size.is_finite() {
        return DEFAULT_UI_FONT_SIZE;
    }
    size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct EditorSettings {
    /// Point size for the editor's mono face.
    ///
    /// `f32`, not `u8`, and the type is the fix rather than a refinement of it: the design
    /// mock specifies **12.5px**, `tokens.css` ships that, and an integer cannot hold the
    /// project's own default. The two defaults here were 13 and 12 — neither the mock's number
    /// nor each other's — which went unnoticed for exactly as long as nothing read them.
    ///
    /// Clamped where a patch lands, not here: see [`clamp_font_size`].
    pub font_size: f32,
    pub tab_size: u8,
    pub insert_spaces: bool,
    pub show_minimap: bool,
    pub word_wrap: bool,
    pub trim_trailing_whitespace_on_save: bool,
    /// Write a changed file when it loses focus, and after a minute with no edits. (M15)
    ///
    /// **On by default**, which is why `persist::v2_to_v3` writes the value into every existing
    /// document rather than letting `#[serde(default)]` supply it. The distinction matters for
    /// exactly the reason [`crate::ProxyScope`]'s did: a defaulted field means an existing
    /// user's setting is an *inference* from a constant in this build, so the day somebody
    /// changes the default for new installs, every workspace on disk silently changes behaviour
    /// — and this particular behaviour writes the user's files. Written down, an upgrade is a
    /// fact on disk and a later change to the default touches new installs only.
    ///
    /// The *interval* is deliberately not here. Rust does nothing with it; a single
    /// `AUTOSAVE_IDLE_MS` in `ui/src/editor/autosave.ts` has one home and nothing to disagree
    /// with. What crosses the wire is the yes/no.
    pub autosave: bool,
    /// How a diff is laid out. See [`DiffView`].
    pub diff_view: DiffView,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_CODE_FONT_SIZE,
            tab_size: 4,
            insert_spaces: true,
            show_minimap: true,
            word_wrap: false,
            trim_trailing_whitespace_on_save: false,
            autosave: true,
            diff_view: DiffView::Unified,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct TerminalSettings {
    /// Point size for the terminal's mono face. See [`EditorSettings::font_size`] for the type.
    ///
    /// Defaults **equal to the editor's**, which is a decision rather than a coincidence:
    /// `tokens.css` unified the two because a terminal beside an editor at a half-pixel
    /// difference reads as two typefaces, and that shipped once already. Two controls means
    /// they *can* diverge; nothing should make them diverge on their own.
    pub font_size: f32,
    pub scrollback: u32,
    /// Which xterm.js renderer to prefer.
    ///
    /// A setting rather than a probe because the healthy and unhealthy paths are
    /// indistinguishable from JavaScript: WebGL context creation succeeds even on a
    /// software rasterizer, and `WEBGL_debug_renderer_info` is masked — on Linux it reports
    /// "Apple GPU" regardless of hardware.
    pub renderer: TerminalRenderer,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_CODE_FONT_SIZE,
            scrollback: 5_000,
            renderer: TerminalRenderer::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TerminalRenderer {
    /// WebGL for the most recently focused panes, DOM for the rest. WebKitGTK caps
    /// concurrent contexts at roughly 8-16 and a pane grid can exceed that.
    #[default]
    Auto,
    Webgl,
    Dom,
}

/// Linux rendering workarounds, applied before the webview is created.
///
/// Each is off by default and each costs something; see `docs/adr/0006`. `disable_dmabuf`
/// is the exception the app cannot start without on a stock KDE Wayland desktop, so
/// `cide-app::graphics` applies it unconditionally unless the environment already has an
/// opinion — this struct is the user-facing override for the machines that heuristic gets
/// wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct GraphicsSettings {
    pub disable_dmabuf_renderer: Option<bool>,
    pub disable_compositing_mode: Option<bool>,
    pub disable_nvidia_explicit_sync: Option<bool>,
}

/// Environment toggles passed to `claude` children, and how many of them come back on launch.
///
/// The three environment toggles default off. [`Self::resume_all_on_launch`] and
/// [`Self::scroll_speed`] do not, which is why this has a hand-written [`Default`] rather than
/// a derived one — a derive would give them `false` and `0` silently, and it did once: the
/// resume field was added with a `true` written into a `Default` impl that did not exist, the
/// edit was a no-op, and only a test asserting the default caught it. `scroll_speed` would
/// have failed the same way and louder, since a derived `0` is a value the CLI discards.
///
/// Everything here except [`Self::resume_all_on_launch`] and [`Self::cli`] is turned into an
/// environment by [`cide_core::child_env::claude_env`], which is the only consumer and the only
/// place the spelling of these variables is decided. Adding a field to this struct without
/// adding a line there produces a switch that persists, renders, and does nothing — which is
/// what three of these were until that function existed.
///
/// **No longer `Copy`**, as of [`Self::cli`]: a `String` and two `Vec`s cannot be. The one call
/// site that relied on it by name is `cmd::session.rs`'s settings read, which now clones
/// alongside the proxy settings it was already cloning. That is one allocation per spawn, on a
/// path that is about to `fork`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClaudeSettings {
    /// `CLAUDE_CODE_DISABLE_MOUSE=1`. Worth offering because xterm.js has no
    /// shift-to-bypass gesture for mouse capture, so a TUI that grabs the mouse takes
    /// selection with it.
    pub disable_mouse: bool,
    /// `CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT=1`.
    pub alt_screen_full_repaint: bool,
    /// `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1`.
    pub disable_alternate_screen: bool,
    /// Resume **every** Claude pane on launch, rather than only the project's console.
    ///
    /// The plan shipped the cautious answer — one eager pane, the rest showing a Resume
    /// splash — on the reasoning that "reopening a six-pane project should not silently fork
    /// six agents at once", and named this flag as the way out. It is the way out.
    ///
    /// Two things make the caution smaller than it reads. Resuming is not forking: `--resume`
    /// continues a conversation that already exists and starts no new one, and it sends no
    /// prompt, so nothing is spent until the user types. What it does cost is one `claude`
    /// process per pane at launch, which is the honest reason someone might turn this off.
    ///
    /// Default **on**, because the splash is a screen asking a question whose answer is
    /// always yes: a pane restored into a saved layout is a pane the user left open.
    pub resume_all_on_launch: bool,
    /// `CLAUDE_CODE_SCROLL_SPEED` — transcript lines the TUI moves per wheel *report*.
    ///
    /// This is the one lever cide has over how far a Claude pane scrolls, and it needs to be
    /// a setting rather than the constant it used to be, because the number that feels right
    /// depends on hardware nobody here can see: a high-resolution wheel under KDE Wayland
    /// delivers a notch as several small deltas, and xterm.js emits at most one report per
    /// wheel event, so the lines-per-notch a user actually gets is this number times a factor
    /// between roughly a third and one that varies by mouse. See `cmd/session.rs`'s `base_env`.
    ///
    /// Not a float, and not unbounded: the CLI parses this with `parseFloat` and then
    /// `Math.min(n, 20)`, and *discards* anything that is `NaN` or `<= 0` — falling back to a
    /// per-renderer default which, for a terminal announcing itself as xterm.js, is **1**.
    /// So an out-of-range value here would not be ignored, it would make scrolling four times
    /// worse than leaving it alone. [`crate::settings::ClaudeSettings::SCROLL_SPEED`] is the
    /// range, and `cide_core::child_env::claude_env` is where it is enforced.
    pub scroll_speed: u8,

    /// Which `claude` is launched, and with what beyond cide's own argv and environment.
    ///
    /// A sub-struct rather than three more fields here because the three belong together on
    /// screen and travel together in a patch — and because `cide_core::claude_cli`, which is
    /// where the rules over them live, wants one thing to take a reference to.
    pub cli: ClaudeCli,
}

/// The user's own launch configuration for `claude`: the binary, extra arguments, extra
/// environment.
///
/// # Stored verbatim, filtered on the way out
///
/// Nothing here is validated on the way *in*. A refused argument is still stored, still shown
/// in the field the user typed it into, and struck out in the readout beside a sentence saying
/// why — the same arrangement [`ProxySettings`] uses for a credentialed URL, and for the same
/// reason: a value you cannot see is a value you cannot correct, and `ui/src/settings/
/// useSettings.ts` sends its patch fire-and-forget with `.catch(() => {})`, so a rejected write
/// is a field that snaps back and says nothing.
///
/// Enforcement is therefore at the spawn, in `cide_core::claude_cli`, and it is not a
/// duplicate of the screen: `workspace.json` is hand-editable and `settings_set` is one
/// `invoke` away from being bypassed, so a filter that only ran in the UI would let a
/// hand-edited file cost somebody their `--resume`.
///
/// # Why `Debug` is written by hand
///
/// [`Self::env`] holds whatever the user typed, which is where a token for their own MCP
/// server ends up. `Settings` derives `Debug`, so a `tracing::debug!(?settings)` anywhere in
/// the app would print it, and there is no way to review every future call site — the exact
/// argument [`ProxySettings`] makes. Names survive, values do not; a log line saying which
/// variables cide set is most of the value and none of the risk.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClaudeCli {
    /// The program a Claude pane, and the headless one-shot lane, actually run.
    ///
    /// **A bare name here stays a bare name.** `"claude"` — the default — is passed through as
    /// a bare string and resolved by the OS at each spawn, which is deliberate and load-bearing:
    /// the CLI updates itself underneath a running app, and resolving the name once at launch
    /// would pin whichever version was on `PATH` then. cide resolves it only to *check* it, and
    /// throws the resolved path away.
    ///
    /// An absolute or relative path is passed through as written, so a user can pin a specific
    /// version or point at a wrapper (`mise`, `asdf`, a shim, a `direnv` exec) — all of which
    /// are legitimate and none of which answer `--version` in a shape cide can parse. That is
    /// why the verdict on this field warns far more often than it refuses; see
    /// `cide_core::claude_cli::resolve`.
    pub binary: String,

    /// Extra arguments, **one token per entry**, placed before every argument cide adds.
    ///
    /// A vector and not a shell string. A single string needs a quoting parser cide would have
    /// to invent and get wrong at the first `--append-system-prompt "be terse"`; the project's
    /// own precedent is `cide_claude::headless::argv`, which spells its tokens out for the same
    /// reason. One row per argument in the UI, plus a readout of the resulting argv.
    ///
    /// Several tokens are refused — see `cide_core::claude_cli::REFUSED_ARGS`. They are the
    /// ones that duplicate an argument cide already passes, and each of them breaks something
    /// silently rather than loudly.
    pub args: Vec<String>,

    /// Extra environment variables for `claude` children.
    ///
    /// A `Vec` of pairs and not a `BTreeMap`, which is what `InspectionSettings::sources` uses.
    /// The difference is who types the key: nothing types the keys of that map, and here the
    /// user does, so a map would rewrite the key on every keystroke and lose the row the moment
    /// two of them were briefly equal. A vector also matches what `SpawnSpec::env` actually
    /// implements — later wins — so a shadowed duplicate can be shown as shadowed rather than
    /// silently dropped.
    pub env: Vec<ClaudeEnvVar>,

    /// The arguments **cide itself** adds, and whether it still adds them.
    ///
    /// Everything above is what the user adds to a Claude Code launch. This is the other half,
    /// and it is what lets the pane drive something that is *not* Claude Code — `opencode`, or
    /// a wrapper that mints its own conversation ids. See `cide_core::claude_cli::INJECTIONS`,
    /// which holds the flag spellings, the default-on rule and the argument for this shape.
    pub inject: ClaudeInjections,
}

/// One argument cide adds to a Claude pane's command line, and whether it still does.
///
/// # `Default` is written by hand, and it is the most dangerous line in this file
///
/// `bool::default()` is `false`. [`ClaudeCli`] carries a container-level `#[serde(default)]`,
/// so a *derived* `Default` here would make every `workspace.json` written before this field
/// existed — which is every one of them — deserialize with **every injection off**. Every
/// user's hooks and resume would die on the launch after an upgrade, from a screen they never
/// opened, with no runtime symptom pointing at it: the pane starts fine and simply reports
/// nothing. `ProxyScope` carries the same hand-written `Default` for the same class of bug.
///
/// `a_configuration_predating_the_injection_switches_still_injects_everything` is the guard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClaudeInjection {
    /// Whether cide passes this argument at all. `true` is today's behaviour and the default.
    ///
    /// Off, the argument is simply absent from the argv. Nothing else changes — the pane still
    /// gets `CIDE_SESSION` and `CLAUDE_CODE_SSE_PORT`, and the registry still keys on cide's
    /// own id. What is lost is stated on the toggle in `ClaudeCliSection.tsx`, per injection,
    /// because each of these silently disables a feature the user will otherwise report as
    /// broken without connecting it to this switch.
    pub enabled: bool,

    /// The spelling to use instead of the default one, or empty for the default.
    ///
    /// Empty rather than `Option<String>`: the field is edited by a text input that is empty
    /// when untouched, and a `None`/`Some("")` distinction the UI cannot express is a
    /// distinction that only ever produces a bug. `cide_core::claude_cli::injected` trims it
    /// and discards an override that is not a flag (a bare token would become `claude`'s first
    /// positional argument, which is a *prompt*) or that collides with another injection's
    /// spelling.
    pub flag: String,
}

impl Default for ClaudeInjection {
    fn default() -> Self {
        Self {
            // Today's behaviour, exactly. See the struct's note for why this cannot be derived.
            enabled: true,
            // Empty means "whatever `INJECTIONS` spells it", so the default spelling lives in
            // exactly one place and a CLI rename is one edit rather than two.
            flag: String::new(),
        }
    }
}

/// The arguments cide adds to a Claude pane, each independently switchable.
///
/// Named fields rather than a map keyed by a string: the set is closed — it is exactly what
/// `cide_core::claude_cli::INJECTIONS` enumerates — and a map would let `workspace.json` name
/// an injection that does not exist, which is a setting wired to nothing.
///
/// Hand-written `Default` for the same reason [`ClaudeInjection`]'s is; a derived one here
/// would be correct only for as long as that one stays hand-written, which is not a property
/// worth depending on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClaudeInjections {
    /// `--session-id <uuid>`. Off: no stable pane-to-conversation identity, and resume stops
    /// working — the CLI mints its own id and files the transcript under it.
    pub session_id: ClaudeInjection,
    /// `--resume <uuid>`. Off: a restored pane starts a fresh conversation.
    pub resume: ClaudeInjection,
    /// `--fork-session`. Off: Split → fork branches nothing; it becomes a plain resume.
    pub fork_session: ClaudeInjection,
    /// `--settings <inline json>`. Off: **no hooks at all** — see the toggle's own copy.
    pub settings: ClaudeInjection,
    /// `--mcp-config <inline json>` attaching cide's own MCP server: `cide-hook mcp`, a stdio
    /// bridge to the socket named by `$CIDE_AGENT_SOCK`. (M18)
    ///
    /// Off: the pane still works and still gets every MCP server **you** configured — cide has
    /// never passed `--strict-mcp-config`, and this switch does not start. What it loses is
    /// cide's own server: no `mcp__cide__cide_task_*`, so nothing this session does reaches
    /// `.cide/tasks.json`, and on a project's console pane no `mcp__cide__cide_agent*` either,
    /// so subagents cannot be dispatched at all. See the toggle's own copy.
    ///
    /// # This field is absent from every `workspace.json` that exists, and defaults to *on*
    ///
    /// It was added after the injector shipped, so every stored `inject` object on disk names
    /// the four above and not this one. Two things already in this file fill it, and neither is
    /// spare: the container's `#[serde(default)]` supplies a member a *present* object does not
    /// mention, and [`Self::default`] is hand-written so what it supplies cannot become
    /// `bool::default()`. Remove either and every upgraded pane silently loses its task tools —
    /// the pane starts, the tracker is simply empty and `mcp__cide__*` is not a tool the model
    /// has. `an_inject_block_written_before_the_task_tools_switch_still_carries_them` pins the
    /// literal shape that is on disk today, the way
    /// `a_sidebar_written_before_the_agents_panel_existed_still_reads` does next door.
    ///
    /// The default is *on* rather than off for the reason a tracker is a feature rather than an
    /// integration: a user who has never heard of any of this should get the task tools, and
    /// the one who does not want them is the one who will go and find this switch.
    pub mcp_config: ClaudeInjection,
}

// Clippy is right that this is `#[derive(Default)]` *today*, and wrong about what that costs.
// The derive is correct only for as long as `ClaudeInjection`'s hand-written `Default` stays
// hand-written; the day somebody derives that one, this one silently becomes "every injection
// off" for every `workspace.json` on disk — no hooks, no resume, on the launch after an
// upgrade. Written out so the two live next to each other and one cannot quietly change the
// other's meaning.
#[allow(clippy::derivable_impls)]
impl Default for ClaudeInjections {
    fn default() -> Self {
        Self {
            session_id: ClaudeInjection::default(),
            resume: ClaudeInjection::default(),
            fork_session: ClaudeInjection::default(),
            settings: ClaudeInjection::default(),
            mcp_config: ClaudeInjection::default(),
        }
    }
}

impl Default for ClaudeCli {
    fn default() -> Self {
        Self {
            // The bare name, exactly what the frontend used to hardcode. See the field.
            binary: "claude".to_string(),
            args: Vec::new(),
            env: Vec::new(),
            // All of them on: the argv a pane is spawned with is byte-for-byte what it was
            // before these switches existed. See `ClaudeInjection`'s note.
            inject: ClaudeInjections::default(),
        }
    }
}

impl fmt::Debug for ClaudeCli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every field named explicitly, and the same hazard `ProxySettings` carries: a field
        // added above and forgotten here vanishes from every debug print rather than failing
        // to compile. `a_claude_cli_debug_print_names_every_field` is the guard, and it reads
        // the list off the serialized value so it cannot fall behind either.
        f.debug_struct("ClaudeCli")
            .field("binary", &self.binary)
            // Arguments are not secret in the way a value is — they are flags, and the whole
            // point of a log line here is to say which ones a child got. `--append-system-prompt
            // <text>` is the closest thing to a leak and it is text the user typed into a field
            // labelled "arguments"; redacting it would make the line useless for the failure it
            // exists to explain.
            .field("args", &self.args)
            // Values do not survive. See the struct's note.
            .field("env", &self.env)
            // Not secret at all, and the single most useful thing in this line when a pane has
            // no hooks or will not resume: it says whether cide still passes the flag.
            .field("inject", &self.inject)
            .finish()
    }
}

/// One `NAME=value` pair the user asked for.
///
/// A named struct rather than a `(String, String)` because it crosses the wire and a tuple
/// arrives in TypeScript as a positional array — `pair[0]` and `pair[1]` in the component that
/// draws the two inputs, which is one transposition away from writing the value into the name.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ClaudeEnvVar {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for ClaudeEnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The name, and the fact that there is a value, and nothing else. `ClaudeCli`'s own
        // `Debug` delegates to this one for the whole list, so the redaction has to be here or
        // it is nowhere: `Vec<T>`'s `Debug` prints each element with `T`'s.
        write!(
            f,
            "{}={}",
            self.name,
            if self.value.is_empty() {
                "\"\""
            } else {
                "<redacted>"
            }
        )
    }
}

impl ClaudeSettings {
    /// The range the CLI will actually honour for [`Self::scroll_speed`], inclusive.
    ///
    /// Published rather than inlined because three places need to agree about it: the clamp in
    /// `cide_core::child_env::claude_env`, the settings control's `min`/`max`, and the test
    /// that pins the clamp. A `min` of 1 rather than 0 is the CLI's rule, not a preference —
    /// zero is one of the values it throws away.
    pub const SCROLL_SPEED: std::ops::RangeInclusive<u8> = 1..=20;
}

impl Default for ClaudeSettings {
    fn default() -> Self {
        Self {
            disable_mouse: false,
            alt_screen_full_repaint: false,
            disable_alternate_screen: false,
            // On. See the field's own note: a pane restored into a saved layout is a pane the
            // user left open, and the splash asks a question whose answer is always yes.
            resume_all_on_launch: true,
            // Three, which is what `base_env` hardcoded before this field existed, so nobody
            // who never opens Settings sees a change. It is not a measured optimum — see the
            // field's note and `base_env`'s.
            scroll_speed: 3,
            cli: ClaudeCli::default(),
        }
    }
}

/// What cide does about the proxy variables in a child's environment.
///
/// Three states rather than a bool, because "do not proxy" and "do not interfere" are
/// different answers and a corporate laptop needs both. A single on/off switch would have to
/// pick one of them to be its off position: off-as-inherit leaves a user whose profile
/// exports `HTTP_PROXY` with no way to run a pane without it, and off-as-direct silently
/// breaks every pane the moment the app ships to somebody who does export one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProxyMode {
    /// Whatever the environment already says. cide sets nothing and scrubs nothing.
    ///
    /// The default, and the only mode in which a child's proxy environment is not entirely
    /// cide's doing. One exception, and it is not a proxy setting: if the inherited
    /// environment *does* name a proxy, the loopback exemption is still merged into
    /// `NO_PROXY` — see [`ProxySettings::no_proxy`].
    #[default]
    Inherit,
    /// The URLs below, overriding anything inherited. A blank field means the variable is
    /// **removed**, not left alone: a mode that says "this is the proxy" must not let a
    /// forgotten profile export supply the half the user left empty.
    Manual,
    /// No proxy for anything cide spawns. Every spelling is scrubbed from the child.
    Direct,
}

/// What the settings above are allowed to do to one kind of child.
///
/// # Why a third state, and why it is the interesting one
///
/// [`ProxyMode`] answers *what the proxy is*. This answers *who gets it*, and the two are
/// genuinely different questions: "use this proxy for `claude` but let `git push` reach the
/// network the way it always has" is one configuration, not a mode.
///
/// The trap is that "cide does not add a proxy for git" and "git does not use a proxy" are
/// **not the same sentence**. `std::process::Command` inherits this process's environment
/// wholesale, and `cide_core::child_env::prepare_command` touches only two things: values
/// prefixed with `$APPDIR`, and `PATH`. A proxy URL is neither — it never points inside an
/// AppImage and it is not `PATH` — so it survives untouched. A cide
/// launched from a shell that exports `HTTPS_PROXY` therefore *already* sends every
/// `git push` through that proxy, and a two-state switch could only ever decide whether cide
/// adds one on top. So there are three states and the middle one is the default:
///
/// * [`Self::Configured`] — the mode and addresses above decide this child's environment.
/// * [`Self::Untouched`] — cide sets nothing and removes nothing. Whatever cide itself was
///   launched with reaches the child. This is what every child except a pane does today.
/// * [`Self::Direct`] — every proxy variable is removed, whatever the mode says and whatever
///   cide inherited. The only state that can promise a child is off the proxy.
///
/// One honesty limit, stated on screen as well as here: `Direct` empties git's *environment*.
/// `git` still honours `http.proxy` in `~/.gitconfig`, and cide does not edit the user's
/// gitconfig — the same rule that keeps the Claude hooks in an inline `--settings` payload
/// rather than in `~/.claude/settings.json`. The guarantee is "cide puts no proxy in this
/// child's environment", never "this child will not find one elsewhere".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProxyTarget {
    /// [`ProxySettings::mode`] and the addresses beside it decide the child's environment.
    #[default]
    Configured,
    /// cide neither sets nor removes a proxy variable for this child.
    ///
    /// Nearly [`ProxyMode::Inherit`], and *not* identical to it: `Inherit` still merges the
    /// loopback exemption into `NO_PROXY` when something was inherited, because cide's IDE
    /// integration is a loopback MCP server that a proxy would silently swallow. `Untouched`
    /// does not even do that, which is right for a child that has no business knowing cide
    /// has a loopback port at all.
    Untouched,
    /// Every proxy variable, in both spellings, is removed from the child.
    Direct,
}

/// Which children the proxy settings reach.
///
/// # The default is today's behaviour, exactly, and that is the whole requirement
///
/// The population that has configured a proxy at all is, by construction, people for whom the
/// current arrangement works. Flipping any of these three would break one of them silently —
/// a proxy failure surfaces as a hang or a timeout with nothing on screen naming cide — so the
/// new capability is opt-in and the defaults reproduce the shipped behaviour: panes proxied,
/// cide's own `git` left exactly as it was.
///
/// `Default` is therefore **hand-written**. `#[derive(Default)]` would give
/// `ProxyTarget::Configured` for all three, which silently puts every existing user's `git`
/// onto cide's proxy — the one change this type exists to make impossible by accident.
///
/// And the defaults are not the only guard: `cide_core::persist` migrates a schema-1
/// `workspace.json` by **writing this object into the document**, rather than letting the
/// serde default supply it. The difference matters the day somebody decides a fresh install
/// should default to claude-only: with the value on disk that is a change to new installs,
/// and with it defaulted it is a silent re-scoping of every live configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProxyScope {
    /// `claude` panes **and** the headless one-shot lane (commit messages, explanations).
    ///
    /// One field for both because they are one thing to a user: a `claude` cide started. The
    /// one-shot lane never read the proxy setting at all before this field existed — it built
    /// its own `std::process::Command` and inherited cide's raw environment — which is why
    /// this doc names it explicitly rather than leaving it to be discovered.
    pub claude: ProxyTarget,
    /// `$SHELL` panes.
    ///
    /// The case the request did not name, so the choice is stated rather than assumed: a
    /// shell pane is the *user's shell*, and a `git pull` typed into one follows this setting
    /// rather than [`Self::git`]. Defaulting it to anything but `Configured` would break the
    /// corporate user who opens a shell pane to run `npm install` through their proxy today.
    pub shells: ProxyTarget,
    /// cide's own `git` shell-outs: the Push and Fetch/Pull buttons, and nothing else.
    ///
    /// **`Untouched` by default and it has to be.** See [`ProxyTarget`]: today git inherits
    /// cide's environment, `Configured` would move it onto cide's proxy instead, and `Direct`
    /// would take a working corporate push away. Only "leave it alone" reproduces what is
    /// already on people's machines.
    pub git: ProxyTarget,
}

impl Default for ProxyScope {
    fn default() -> Self {
        Self {
            claude: ProxyTarget::Configured,
            shells: ProxyTarget::Configured,
            // Not `Configured`. See the field.
            git: ProxyTarget::Untouched,
        }
    }
}

/// Proxy configuration, applied to every child cide spawns — `$SHELL` panes and `claude`
/// alike.
///
/// # This is where a password can end up
///
/// `http://user:hunter2@proxy.corp:3128` is a legal and common value, and this struct is
/// serialized into `workspace.json` in plain text, which is the only way a proxy that needs
/// credentials can work at all without a keyring cide does not have. What is *not* acceptable
/// is that value leaking sideways into a log the user then pastes into a bug report, so
/// [`Debug`] is implemented by hand and redacts the userinfo. That is deliberately at the
/// type level rather than at each call site: `tracing::debug!(?settings)` anywhere in the app
/// would otherwise print the password, and there is no way to review every future call site.
///
/// The alternative that lost was storing the credentials separately in the OS keyring. It is
/// the right answer and it is a milestone of its own — Secret Service over zbus, a fallback
/// for machines with no keyring daemon, and a migration for the value already on disk.
/// Redacted `Debug` is what makes the plain-text version defensible in the meantime.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProxySettings {
    pub mode: ProxyMode,
    /// Which children [`Self::mode`] and the addresses below actually reach.
    ///
    /// Deliberately a second field rather than more [`ProxyMode`] variants. Crossing "what
    /// the proxy is" with "who gets it" yields nine variants of which several are
    /// meaningless, and it would break `ProxyMode.ts` — which `ui/scripts/check-proxy.mjs`
    /// asserts against as the list of modes the readout branches on.
    ///
    /// Also deliberately not a `HashMap<Target, _>`: an open map has no natural answer for a
    /// key added by a later build, puts an unbounded shape into `workspace.json`, and turns
    /// the Settings screen into a matrix editor.
    pub scope: ProxyScope,
    /// `HTTP_PROXY`/`http_proxy`. Empty means the variable is not set.
    pub http: String,
    /// `HTTPS_PROXY`/`https_proxy`. Empty **falls back to [`Self::http`]**, because one
    /// CONNECT proxy for both schemes is the overwhelmingly common shape and asking for it
    /// twice is how one of the two ends up stale.
    pub https: String,
    /// `ALL_PROXY`/`all_proxy`, usually a SOCKS URL.
    ///
    /// No fallback from [`Self::http`], unlike `https`: `ALL_PROXY` covers protocols beyond
    /// HTTP, and quietly pointing them at an HTTP CONNECT proxy the user only meant for web
    /// traffic changes behaviour they did not ask for.
    pub all: String,
    /// Extra `NO_PROXY` entries, comma separated — the corporate intranet, a registry mirror.
    ///
    /// `localhost`, `127.0.0.1` and `::1` are always prepended and cannot be removed. cide's
    /// IDE integration is an MCP server on loopback (`CLAUDE_CODE_SSE_PORT`), and a proxy
    /// that swallows loopback turns the headline feature — inline diffs, @-mentions, the
    /// editor selection — off with no error anywhere. Users do not think to exempt a port
    /// they were never told about.
    pub no_proxy: String,
}

impl ProxySettings {
    /// The `HTTPS_PROXY` value, after the fallback described on [`Self::https`].
    pub fn https_url(&self) -> Option<String> {
        normalize_proxy_url(&self.https).or_else(|| normalize_proxy_url(&self.http))
    }
}

impl fmt::Debug for ProxySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every field is named explicitly, and that is the hazard this impl carries: a field
        // added above and forgotten here vanishes from every debug print in the app rather
        // than failing to compile. `a_debug_print_names_every_field` below is the guard, and
        // it reads the field list off the *serialized* value so it cannot forget either.
        f.debug_struct("ProxySettings")
            .field("mode", &self.mode)
            // Booleans and three named states. No secret, and nothing to redact — but it has
            // to be here, because which children got the proxy is half of any answer to "why
            // did this child not reach the network".
            .field("scope", &self.scope)
            .field("http", &redact_proxy_url(&self.http))
            .field("https", &redact_proxy_url(&self.https))
            .field("all", &redact_proxy_url(&self.all))
            // Not a URL and cannot carry userinfo, so it is printed as it stands.
            .field("no_proxy", &self.no_proxy)
            .finish()
    }
}

/// A proxy URL as a child process should see it, or `None` for "not set".
///
/// Blank-is-unset rather than `Option<String>` on the struct: the UI binds text inputs to
/// these fields, and a control that has to distinguish "" from `None` grows a second piece of
/// state that disagrees with the first one.
///
/// A scheme is added when there is none. `proxy.corp:3128` is what people type and what curl
/// accepts (defaulting to HTTP), but `new URL()` in Node throws on it and the Claude CLI is
/// Node — so the same string would proxy a `curl` in a shell pane and fail in the pane next to
/// it. Normalising here means every consumer is handed the same unambiguous answer.
pub fn normalize_proxy_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        Some(trimmed.to_string())
    } else {
        Some(format!("http://{trimmed}"))
    }
}

/// A proxy URL with any credentials removed, safe to put in a log line or an error message.
///
/// The host survives, because a redaction that hides the host too makes the log line useless
/// for the one question it is there to answer — which proxy did this child get. Only the
/// userinfo before `@` is a secret.
///
/// A string walk rather than a URL crate: this must never fail, and a parser that rejects a
/// malformed value would leave the caller holding the raw string with the password in it.
/// Anything this cannot make sense of is redacted whole.
pub fn redact_proxy_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        // Nothing to anchor on, so nothing can be shown to be safe.
        return if url.contains('@') {
            "***".to_string()
        } else {
            url.to_string()
        };
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    // `rsplit_once`, not `split_once`: a password may itself contain an `@`, and splitting on
    // the first one would print the tail of it.
    match rest[..authority_end].rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}{}", &rest[authority_end..]),
        None => url.to_string(),
    }
}

/// How much of a buffer's diagnostics are drawn in the gutter and under the text.
///
/// IDEA's highlighting-level widget, and the reason it exists is a 40,000-line generated file
/// where every line is a warning. Per *editor* rather than global — see
/// `ui/src/editor/highlightLevel.ts` for where the override is held and why it is not persisted.
///
/// All three levels keep the grammar. Turning highlighting down is about problems, not colour,
/// and IDEA's lexer keeps colouring at every level too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HighlightLevel {
    /// No squiggles, no gutter marks. The panel and the status bar are unaffected.
    None,
    /// Only [`crate::DiagnosticKind::Syntax`] — the things that are wrong about the *text*,
    /// which is what stays useful in a file whose semantics are hopeless.
    SyntaxOnly,
    #[default]
    AllProblems,
}

/// Which problems are analysed, and which of them are shown.
///
/// Three axes, and they are not one axis said three times:
///
/// * [`Self::sources`] decides which **processes run**. Turning rust-analyzer off turns off a
///   1–4 GB indexer; nothing is computed, so nothing can be revealed by turning a severity back
///   on afterwards. It is also the only axis Claude sees — see
///   `cide_ide_mcp::tools::get_diagnostics`.
/// * [`Self::severities`] decides what is **shown**, of what was computed. A display filter,
///   applied once in Rust so the panel and the status bar cannot disagree.
/// * The per-editor highlighting level is **not stored here**. [`Self::default_highlight_level`]
///   is only what a newly opened editor starts at; the override lives in the webview for the
///   session and is deliberately not persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct InspectionSettings {
    pub severities: SeverityFilter,
    /// Analyser name → shown. **A source absent from this map is shown.**
    ///
    /// A map rather than a struct of four bools, for two reasons. A new language server should
    /// not need a DTO change and a migration to be toggleable; and `clippy` arrives inside
    /// rust-analyzer's stream under its own name, so the set is not knowable in advance.
    ///
    /// "Absent means shown" is `countBySeverity`'s `other`-bucket rule applied to sources: a
    /// producer we have never seen must not be silently hidden. [`Default`] seeds the one
    /// exception — see below.
    pub sources: std::collections::BTreeMap<String, bool>,
    /// What an editor's highlighting level starts at when nothing has overridden it.
    pub default_highlight_level: HighlightLevel,
    /// Type a note into the pinned Claude pane when new problems appear.
    ///
    /// Off by default, and read the note in `cide_app::cmd::diagnostics` before "improving" this
    /// into a real protocol notification: the CLI has no `diagnostics_changed` method, and one
    /// sent to it would be dropped with no error in either direction. What makes Claude aware
    /// of problems is that it *pulls* — it calls `getDiagnostics` at the start of a turn. This
    /// setting is the small extra of putting a line in front of the user, unsent.
    pub push_to_claude: bool,
    /// Debounce for that note, clamped to [`MIN_PUSH_DEBOUNCE_MS`]..=[`MAX_PUSH_DEBOUNCE_MS`]
    /// where the patch lands rather than on read.
    pub push_debounce_ms: u32,
}

/// The four severities, as toggles.
///
/// `weak_warning` is IDEA's name for what LSP calls **Information** and what
/// [`crate::Severity`] calls `Info`. Mapped rather than added — see that enum's comment for
/// what a fifth severity would cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct SeverityFilter {
    pub error: bool,
    pub warning: bool,
    pub weak_warning: bool,
    pub hint: bool,
}

/// The floor on [`InspectionSettings::push_debounce_ms`].
///
/// A hand-edited `0` would write into the user's live Claude prompt on every publish of a
/// `cargo check` — hundreds of them, into a text field they are typing in.
pub const MIN_PUSH_DEBOUNCE_MS: u32 = 500;

/// The ceiling. Past half a minute the note is about a state the user has already moved on from.
pub const MAX_PUSH_DEBOUNCE_MS: u32 = 30_000;

impl Default for SeverityFilter {
    /// Hand-written, and this is the `resume_all_on_launch` lesson applied before it bites
    /// again: `#[derive(Default)]` gives every `bool` here `false`, which would ship a panel
    /// that shows nothing at all and a status bar reading `✗ 0` over a broken workspace.
    fn default() -> Self {
        Self {
            error: true,
            warning: true,
            weak_warning: true,
            hint: true,
        }
    }
}

impl Default for InspectionSettings {
    fn default() -> Self {
        Self {
            severities: SeverityFilter::default(),
            // Seeded with the one source that is opt-in, which is also what keeps
            // "absent means shown" intact for everything else. A `claude: bool` field beside
            // the map would be a second control writing the same fact.
            //
            // Off because it is the only source that spends the user's quota. rust-analyzer and
            // gopls cost CPU and memory on a machine the user already owns; a Claude inspection
            // costs tokens, and defaulting that on would be a bill nobody agreed to.
            sources: [("claude".to_string(), false)].into_iter().collect(),
            default_highlight_level: HighlightLevel::default(),
            push_to_claude: false,
            push_debounce_ms: 2_000,
        }
    }
}

impl InspectionSettings {
    /// Is this producer's output shown? Absent means yes — see [`Self::sources`].
    pub fn shows_source(&self, source: &str) -> bool {
        self.sources.get(source).copied().unwrap_or(true)
    }

    /// Is this severity shown?
    pub fn shows_severity(&self, severity: crate::Severity) -> bool {
        match severity {
            crate::Severity::Error => self.severities.error,
            crate::Severity::Warning => self.severities.warning,
            crate::Severity::Info => self.severities.weak_warning,
            crate::Severity::Hint => self.severities.hint,
        }
    }

    /// Clamp anything a hand-edited `workspace.json` could have put out of range.
    ///
    /// Called where the patch lands, like [`SidebarSettings::clamped`] and
    /// [`clamp_font_size`] — never on read, so the stored value is the one the user set.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.push_debounce_ms = self
            .push_debounce_ms
            .clamp(MIN_PUSH_DEBOUNCE_MS, MAX_PUSH_DEBOUNCE_MS);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two defaults, pinned, because they are the whole shape of the feature.
    ///
    /// A fresh workspace shows `.claude` **and** `target/`. Both halves are a decision rather
    /// than an accident — see [`ExplorerSettings`] — and both are invisible in any other test:
    /// a wrong default here is not a failure anywhere, it is a file tree quietly missing a
    /// directory the user asked to see.
    ///
    /// `show_ignored_files` shipped `false` and was corrected to `true` one report later. It is
    /// pinned here, and `persist::v3_to_v4` carries the correction onto disks that already hold
    /// the old value, so the two must move together: changing this constant without a rung
    /// leaves every existing user on whatever their file happens to say.
    #[test]
    fn a_fresh_workspace_shows_dotfiles_and_ignored_files() {
        let settings = Settings::default();
        assert!(settings.explorer.show_hidden_files);
        assert!(settings.explorer.show_ignored_files);
        assert_eq!(settings.explorer, ExplorerSettings::default());
    }

    /// A settings file written before these fields existed still loads, with the defaults.
    ///
    /// `#[serde(default)]` on the struct is what does it, and it is the difference between a
    /// user's first launch after an update showing their workspace and showing an error.
    #[test]
    fn settings_written_before_the_explorer_group_existed_still_deserialize() {
        let settings: Settings = serde_json::from_str("{}").expect("an empty object is a default");
        assert_eq!(settings.explorer, ExplorerSettings::default());
        let partial: ExplorerSettings =
            serde_json::from_str(r#"{"showIgnoredFiles":true}"#).expect("a partial group");
        assert!(
            partial.show_hidden_files,
            "a field the file does not mention keeps its default rather than becoming false"
        );
        assert!(partial.show_ignored_files);
    }

    /// The password must not survive `{:?}`, however the settings are printed.
    ///
    /// The whole struct, not the field: this is the leak that a future
    /// `tracing::debug!(?settings)` would cause, and the point of the manual `Debug` is that
    /// such a call site does not have to know about proxies to be safe.
    #[test]
    fn a_password_does_not_survive_debug() {
        let proxy = ProxySettings {
            mode: ProxyMode::Manual,
            scope: ProxyScope::default(),
            http: "http://alice:hunter2@proxy.corp:3128".into(),
            https: "http://alice:hunter2@proxy.corp:3128".into(),
            all: "socks5://alice:hunter2@socks.corp:1080".into(),
            no_proxy: "corp.internal".into(),
        };
        let settings = Settings {
            proxy: proxy.clone(),
            ..Settings::default()
        };

        for printed in [format!("{proxy:?}"), format!("{settings:?}")] {
            assert!(!printed.contains("hunter2"), "password leaked: {printed}");
            assert!(!printed.contains("alice"), "username leaked: {printed}");
            // The host is what makes the line worth logging at all.
            assert!(printed.contains("proxy.corp:3128"), "host lost: {printed}");
        }
    }

    /// A `Debug` impl written by hand is a list that can fall behind the struct, and the
    /// failure is silent: the missing field simply stops appearing in every log line.
    ///
    /// Read off `serde_json` rather than restated, so this cannot fall behind either.
    #[test]
    fn a_debug_print_names_every_field() {
        let printed = format!("{:?}", ProxySettings::default());
        let value = serde_json::to_value(ProxySettings::default()).expect("serializes");
        let fields = value.as_object().expect("a struct");
        assert!(!fields.is_empty());
        for name in fields.keys() {
            // The `Debug` impl uses Rust's own names, `serde` the camelCase wire ones. Only
            // `no_proxy`/`noProxy` differ, and comparing on the lower-cased letters alone is
            // enough to catch a field that is absent altogether.
            let flattened: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
            let haystack: String = printed
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_lowercase();
            assert!(
                haystack.contains(&flattened.to_lowercase()),
                "`{name}` is a field of ProxySettings and does not appear in {printed}"
            );
        }
    }

    /// The one that would ship the bug: `#[derive(Default)]` on [`ProxyScope`] puts every
    /// existing user's `git push` onto cide's proxy on the launch after an upgrade.
    #[test]
    fn the_default_scope_is_todays_behaviour_and_not_the_derived_one() {
        let scope = ProxyScope::default();
        assert_eq!(scope.claude, ProxyTarget::Configured);
        assert_eq!(scope.shells, ProxyTarget::Configured);
        assert_eq!(
            scope.git,
            ProxyTarget::Untouched,
            "git inherits cide's own environment today; Configured would move it onto cide's \
             proxy and Direct would take a working corporate push away"
        );
        // And the derived answer, which is what a future `#[derive(Default)]` would give.
        assert_ne!(
            scope.git,
            ProxyTarget::default(),
            "ProxyTarget's own default is Configured, so ProxyScope's Default must stay \
             hand-written"
        );
    }

    /// A settings document written before this field existed still proxies both pane kinds.
    ///
    /// The serde default is the second line of defence — `persist::migrate` writes the object
    /// into the document explicitly — but it is the one that runs for a `ProxySettings`
    /// deserialized anywhere other than through a full workspace load.
    #[test]
    fn a_document_predating_the_scope_field_keeps_both_pane_kinds_proxied() {
        let older: ProxySettings = serde_json::from_str(
            r#"{"mode":"manual","http":"http://proxy.corp:3128","https":"","all":"",
                "noProxy":"corp.internal"}"#,
        )
        .expect("a proxy configuration predating `scope` still deserialises");

        assert_eq!(older.mode, ProxyMode::Manual);
        assert_eq!(older.scope, ProxyScope::default());
        assert_eq!(older.no_proxy, "corp.internal");
    }

    /// Three named states on the wire, in camelCase, because the frontend branches on them.
    #[test]
    fn the_three_targets_survive_the_wire_under_their_camel_case_names() {
        for (target, wire) in [
            (ProxyTarget::Configured, "\"configured\""),
            (ProxyTarget::Untouched, "\"untouched\""),
            (ProxyTarget::Direct, "\"direct\""),
        ] {
            let json = serde_json::to_string(&target).expect("serialize");
            assert_eq!(json, wire);
            assert_eq!(
                serde_json::from_str::<ProxyTarget>(wire).expect("deserialize"),
                target
            );
        }
    }

    #[test]
    fn redaction_keeps_the_host_and_drops_the_userinfo() {
        assert_eq!(
            redact_proxy_url("http://u:p@proxy.corp:3128"),
            "http://***@proxy.corp:3128"
        );
        // An `@` inside the password must not put part of it on screen.
        assert_eq!(
            redact_proxy_url("http://u:p@ss@proxy.corp:3128"),
            "http://***@proxy.corp:3128"
        );
        // Nothing to hide: printed as it stands.
        assert_eq!(
            redact_proxy_url("http://proxy.corp:3128"),
            "http://proxy.corp:3128"
        );
        assert_eq!(redact_proxy_url(""), "");
        // Unparseable *and* carrying an `@`: redacted whole rather than guessed at.
        assert_eq!(redact_proxy_url("u:p@proxy.corp:3128"), "***");
    }

    #[test]
    fn a_bare_host_and_port_gains_the_scheme_node_insists_on() {
        assert_eq!(
            normalize_proxy_url(" proxy.corp:3128 ").as_deref(),
            Some("http://proxy.corp:3128")
        );
        assert_eq!(
            normalize_proxy_url("socks5h://localhost:9050").as_deref(),
            Some("socks5h://localhost:9050")
        );
        assert_eq!(normalize_proxy_url("   "), None);
    }

    /// The one-proxy-for-both case, which is what most corporate setups are.
    #[test]
    fn https_falls_back_to_http_but_never_the_other_way() {
        let proxy = ProxySettings {
            mode: ProxyMode::Manual,
            http: "http://proxy.corp:3128".into(),
            ..ProxySettings::default()
        };
        assert_eq!(proxy.https_url().as_deref(), Some("http://proxy.corp:3128"));

        let https_only = ProxySettings {
            mode: ProxyMode::Manual,
            https: "http://tls.corp:3129".into(),
            ..ProxySettings::default()
        };
        assert_eq!(
            https_only.https_url().as_deref(),
            Some("http://tls.corp:3129")
        );
        assert_eq!(
            normalize_proxy_url(&https_only.http),
            None,
            "http stays unset"
        );
    }

    /// The round trip a resize has to survive to still be there after a restart: struct →
    /// the JSON `workspace.json` actually stores → struct.
    ///
    /// Asserted on the *camelCase wire names* rather than only on equality, because equality
    /// would pass just as well if both directions agreed on `files_width` — and the
    /// frontend, which reads this type through `generated.ts`, would then see `undefined`
    /// for both widths and fall back to the mock's defaults on every launch. That is exactly
    /// the bug this feature exists to fix, arriving through the serializer instead.
    #[test]
    fn a_sidebar_width_survives_the_json_round_trip_under_its_wire_name() {
        let settings = Settings {
            sidebar: SidebarSettings {
                files_width: 300,
                git_width: 500,
                agents_width: 340,
                ext_width: 280,
            },
            ..Settings::default()
        };

        let json = serde_json::to_string(&settings).expect("serialize");
        assert!(
            json.contains("\"filesWidth\":300"),
            "wire name changed: {json}"
        );
        assert!(
            json.contains("\"gitWidth\":500"),
            "wire name changed: {json}"
        );

        let back: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.sidebar, settings.sidebar);
    }

    /// A `workspace.json` written before this field existed — i.e. every install that
    /// upgrades into this build.
    ///
    /// It must load, and it must load as the mock's widths rather than as zero. That is what
    /// the container's `#[serde(default)]` buys; an `Option<SidebarSettings>` would have
    /// pushed the same decision onto the frontend, in a place with no default to reach for.
    #[test]
    fn settings_saved_before_the_sidebar_field_existed_still_load() {
        let legacy = r#"{"theme":"dark","reopenLastProject":false}"#;
        let settings: Settings = serde_json::from_str(legacy).expect("legacy settings");
        assert_eq!(settings.sidebar, SidebarSettings::default());
        assert_eq!(settings.sidebar.files_width, 252);
        assert_eq!(settings.sidebar.git_width, 420);
        // And the rest of the file still parsed: the new field did not become required.
        assert!(!settings.reopen_last_project);
    }

    /// The same guarantee for M12's field. Every `workspace.json` on disk predates it.
    #[test]
    fn settings_saved_before_the_inspections_field_existed_still_load() {
        let legacy = r#"{"theme":"dark","sidebar":{"filesWidth":300}}"#;
        let settings: Settings = serde_json::from_str(legacy).expect("legacy settings");
        assert_eq!(settings.inspections, InspectionSettings::default());
        assert!(settings.inspections.severities.error);
        assert_eq!(
            settings.inspections.default_highlight_level,
            HighlightLevel::AllProblems
        );
        // And the rest of the file still parsed.
        assert_eq!(settings.sidebar.files_width, 300);
    }

    /// **The upgrade trap, and it is the one that matters.**
    ///
    /// Every `workspace.json` on disk predates `ClaudeCli::inject`. `ClaudeCli` carries a
    /// container-level `#[serde(default)]`, so a *derived* `Default` on [`ClaudeInjection`] —
    /// where `bool::default()` is `false` — would load every one of those files with every
    /// injection off: no `--settings` and therefore **no hooks at all**, and no `--session-id`
    /// and therefore no resume, for every user, on the launch after an upgrade, from a screen
    /// they never opened. There is no runtime symptom pointing at it; the pane starts fine and
    /// simply reports nothing.
    ///
    /// Hand-edit `ClaudeInjection::default()` to `enabled: false` and this fails.
    #[test]
    fn a_configuration_predating_the_injection_switches_still_injects_everything() {
        // An M16-era value, exactly as that build wrote it.
        let legacy = r#"{"binary":"claude","args":[],"env":[]}"#;
        let cli: ClaudeCli = serde_json::from_str(legacy).expect("legacy claude cli");
        assert_eq!(cli, ClaudeCli::default());
        for injection in [
            &cli.inject.session_id,
            &cli.inject.resume,
            &cli.inject.fork_session,
            &cli.inject.settings,
            &cli.inject.mcp_config,
        ] {
            assert!(injection.enabled, "an upgrade must change nothing");
            assert_eq!(injection.flag, "", "and the spelling is still cide's own");
        }

        // The same for a whole settings document that predates the field, since that is the
        // shape actually on disk.
        let legacy = r#"{"theme":"dark","claude":{"cli":{"binary":"/opt/claude"}}}"#;
        let settings: Settings = serde_json::from_str(legacy).expect("legacy settings");
        assert_eq!(settings.claude.cli.binary, "/opt/claude");
        assert!(settings.claude.cli.inject.settings.enabled);
    }

    /// Every `inject` block on disk today, read by this build. (M18)
    ///
    /// The sibling of `a_sidebar_written_before_the_agents_panel_existed_still_reads`, and the
    /// same class of regression: `mcpConfig` was added after the injector shipped, so every
    /// stored `inject` object names the four keys below and not the fifth. Two things fill it —
    /// the container's `#[serde(default)]` and `ClaudeInjections`' hand-written `Default` — and
    /// this pins the literal old shape rather than trusting that both stay put. Break either and
    /// an upgrade silently costs every pane its task tools: the pane starts, `.cide/tasks.json`
    /// stops being written, and `mcp__cide__*` is simply not a tool the model has.
    #[test]
    fn an_inject_block_written_before_the_task_tools_switch_still_carries_them() {
        // An M18-era value, exactly as that build wrote it: the four keys it knew about, with
        // one of them switched off so the block is not merely the default by another name.
        let old = r#"{"binary":"claude","args":[],"env":[],"inject":{
            "sessionId":{"enabled":true,"flag":""},
            "resume":{"enabled":true,"flag":""},
            "forkSession":{"enabled":false,"flag":""},
            "settings":{"enabled":true,"flag":"--config"}}}"#;
        let cli: ClaudeCli = serde_json::from_str(old).expect("pre-task-tools inject block");
        assert!(!cli.inject.fork_session.enabled, "the block still read");
        assert_eq!(cli.inject.settings.flag, "--config");
        assert!(
            cli.inject.mcp_config.enabled,
            "the task tools are the feature; an upgrade must not take them away"
        );
        assert_eq!(cli.inject.mcp_config.flag, "", "at cide's own spelling");
    }

    /// One toggle set does not turn the others off, which is what a hand-written `Default` on
    /// the *container* is for: `#[serde(default)]` fills a missing field from
    /// `ClaudeInjections::default()`, not from `ClaudeInjection`'s derive.
    ///
    /// This is also the mechanism the newest field leans on, proved on a field that has been
    /// there since M16: four of the five keys are absent from the object below and every one of
    /// them comes back on.
    #[test]
    fn one_named_injection_leaves_the_others_alone() {
        let json = r#"{"inject":{"settings":{"enabled":false}}}"#;
        let cli: ClaudeCli = serde_json::from_str(json).expect("partial inject");
        assert!(!cli.inject.settings.enabled);
        assert!(cli.inject.session_id.enabled);
        assert!(cli.inject.resume.enabled);
        assert!(cli.inject.fork_session.enabled);
        assert!(cli.inject.mcp_config.enabled);
        // …and a toggle with no spelling beside it is still the default spelling.
        assert_eq!(cli.inject.session_id.flag, "");
    }

    /// The wire names, which the Settings screen reads off the generated bindings.
    #[test]
    fn an_injection_survives_the_json_round_trip_under_its_camel_case_name() {
        let mut cli = ClaudeCli::default();
        cli.inject.fork_session.flag = "--branch".into();
        cli.inject.session_id.enabled = false;
        cli.inject.mcp_config.enabled = false;
        let json = serde_json::to_string(&cli).expect("serializes");
        assert!(
            json.contains(r#""forkSession":{"enabled":true,"flag":"--branch"}"#),
            "{json}"
        );
        assert!(json.contains(r#""sessionId":{"enabled":false"#), "{json}");
        assert!(json.contains(r#""mcpConfig":{"enabled":false"#), "{json}");
        assert_eq!(serde_json::from_str::<ClaudeCli>(&json).unwrap(), cli);
    }

    /// The hand-written `Debug` on `ClaudeCli` is a list that can fall behind the struct, and
    /// the failure is silent: the missing field simply stops appearing in every log line. The
    /// injections are the single most useful thing in that line when a pane has no hooks or
    /// will not resume, because they say whether cide still passes the flag at all.
    #[test]
    fn a_claude_cli_debug_print_names_every_field() {
        let printed = format!("{:?}", ClaudeCli::default());
        let value = serde_json::to_value(ClaudeCli::default()).expect("serializes");
        let fields = value.as_object().expect("a struct");
        assert!(!fields.is_empty());
        for name in fields.keys() {
            let flattened: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
            let haystack: String = printed
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_lowercase();
            assert!(
                haystack.contains(&flattened.to_lowercase()),
                "`{name}` is a field of ClaudeCli and does not appear in {printed}"
            );
        }
    }

    /// And the reason that `Debug` is hand-written in the first place still holds: a value the
    /// user typed is a place a token for their own MCP server ends up.
    #[test]
    fn an_environment_value_does_not_survive_debug() {
        let cli = ClaudeCli {
            env: vec![ClaudeEnvVar {
                name: "MY_MCP_TOKEN".into(),
                value: "hunter2".into(),
            }],
            ..ClaudeCli::default()
        };
        let printed = format!("{:?}", cli);
        assert!(printed.contains("MY_MCP_TOKEN"), "{printed}");
        assert!(!printed.contains("hunter2"), "{printed}");
    }

    #[test]
    fn an_inspection_setting_survives_the_json_round_trip_under_its_wire_name() {
        // Equality alone would pass with snake_case on both sides, and the frontend would then
        // read `undefined` for every one of these — the bug
        // `a_sidebar_width_survives_the_json_round_trip_under_its_wire_name` documents.
        let mut settings = Settings::default();
        settings.inspections.severities.weak_warning = false;
        settings.inspections.push_to_claude = true;
        settings
            .inspections
            .sources
            .insert("rust-analyzer".into(), false);

        let json = serde_json::to_string(&settings).expect("serialize");
        for wire in [
            r#""weakWarning":false"#,
            r#""pushToClaude":true"#,
            r#""pushDebounceMs":2000"#,
            r#""defaultHighlightLevel":"allProblems""#,
            r#""rust-analyzer":false"#,
        ] {
            assert!(json.contains(wire), "{wire} missing from {json}");
        }
        assert_eq!(
            serde_json::from_str::<Settings>(&json).expect("round trip"),
            settings
        );
    }

    #[test]
    fn every_analyser_is_on_by_default_except_the_one_that_spends_money() {
        // A derived `Default` would have made all four `false` and shipped an IDE whose panel
        // says nothing runs. This is the assertion that would have caught it.
        let inspections = InspectionSettings::default();
        for source in ["rust-analyzer", "gopls", "tree-sitter", "clippy"] {
            assert!(
                inspections.shows_source(source),
                "{source} was off by default"
            );
        }
        assert!(
            !inspections.shows_source("claude"),
            "Claude inspections cost tokens and must be opt-in"
        );
    }

    #[test]
    fn a_source_nobody_has_heard_of_is_shown_rather_than_hidden() {
        // The `other`-bucket rule applied to producers: an analyser we have never seen must
        // not be silently filtered out, because the map is a list of *exceptions*, not a
        // registry. `clippy` arrives inside rust-analyzer's stream under its own name and was
        // never written to this map by anyone.
        let mut inspections = InspectionSettings::default();
        inspections.sources.insert("gopls".into(), false);
        assert!(inspections.shows_source("some-future-linter"));
        assert!(!inspections.shows_source("gopls"));
    }

    #[test]
    fn a_partial_inspections_object_leaves_the_rest_at_their_defaults() {
        // What a hand-edited file tends to look like.
        let partial = r#"{"inspections":{"severities":{"hint":false}}}"#;
        let settings: Settings = serde_json::from_str(partial).expect("partial inspections");
        assert!(!settings.inspections.severities.hint);
        assert!(
            settings.inspections.severities.error,
            "naming one severity turned the others off"
        );
        assert_eq!(settings.inspections.push_debounce_ms, 2_000);
    }

    #[test]
    fn a_hand_edited_debounce_of_zero_is_clamped_rather_than_honoured() {
        // Zero would write into the user's live Claude prompt on every publish of a
        // `cargo check` — hundreds of writes into a text field they are typing in.
        let wild = InspectionSettings {
            push_debounce_ms: 0,
            ..InspectionSettings::default()
        };
        assert_eq!(wild.clamped().push_debounce_ms, MIN_PUSH_DEBOUNCE_MS);

        let huge = InspectionSettings {
            push_debounce_ms: u32::MAX,
            ..InspectionSettings::default()
        };
        assert_eq!(huge.clamped().push_debounce_ms, MAX_PUSH_DEBOUNCE_MS);
    }

    #[test]
    fn a_severity_toggle_maps_ideas_name_onto_lsps() {
        // IDEA's "weak warning" is LSP's Information is our `Info`. One thing, three names —
        // and the settings row is the only place the user sees IDEA's.
        let mut inspections = InspectionSettings::default();
        inspections.severities.weak_warning = false;
        assert!(!inspections.shows_severity(crate::Severity::Info));
        assert!(inspections.shows_severity(crate::Severity::Warning));
        assert!(inspections.shows_severity(crate::Severity::Hint));
    }

    /// Half a `sidebar` object, which is what a hand-edited file tends to look like.
    #[test]
    fn one_named_width_leaves_the_others_at_their_defaults() {
        let partial = r#"{"sidebar":{"gitWidth":500}}"#;
        let settings: Settings = serde_json::from_str(partial).expect("partial sidebar");
        assert_eq!(settings.sidebar.git_width, 500);
        assert_eq!(settings.sidebar.files_width, 252);
        assert_eq!(settings.sidebar.agents_width, 320);
    }

    /// Every `workspace.json` on disk today, read by this build. (M18)
    ///
    /// The regression this guards is total rather than cosmetic: without
    /// `#[serde(default = "default_agents_width")]` a `sidebar` object that names both of the
    /// widths it knows about and not the third is a *missing field* error, the whole `sidebar`
    /// member fails, and the first launch after the upgrade loses the block. A test naming the
    /// literal old shape is the only thing that keeps the next stored field from repeating it.
    #[test]
    fn a_sidebar_written_before_the_agents_panel_existed_still_reads() {
        let old = r#"{"sidebar":{"filesWidth":300,"gitWidth":500}}"#;
        let settings: Settings = serde_json::from_str(old).expect("pre-M18 sidebar");
        assert_eq!(settings.sidebar.files_width, 300);
        assert_eq!(settings.sidebar.git_width, 500);
        assert_eq!(settings.sidebar.agents_width, 320);
    }

    #[test]
    fn clamping_pulls_every_width_inside_the_band_and_leaves_legal_ones_alone() {
        let clamped = SidebarSettings {
            files_width: 4,
            git_width: 9_000,
            agents_width: 0,
            ext_width: 9_000,
        }
        .clamped();
        assert_eq!(clamped.files_width, SIDEBAR_MIN_WIDTH);
        assert_eq!(clamped.git_width, SIDEBAR_MAX_WIDTH);
        assert_eq!(clamped.agents_width, SIDEBAR_MIN_WIDTH);
        assert_eq!(clamped.ext_width, SIDEBAR_MAX_WIDTH);

        let legal = SidebarSettings {
            files_width: 300,
            git_width: 500,
            agents_width: 340,
            ext_width: 280,
        };
        assert_eq!(legal.clamped(), legal);
        // The defaults are inside the band, or a first launch would move the panel itself.
        assert_eq!(
            SidebarSettings::default().clamped(),
            SidebarSettings::default()
        );
    }
}

#[cfg(test)]
mod font_size_tests {
    use super::*;

    /// The two code surfaces start at the same size, and it is the mock's.
    ///
    /// Not a style preference: `tokens.css` unified them because a terminal beside an editor at
    /// a half-pixel difference reads as two typefaces, and that shipped once. The old defaults
    /// were 13 and 12 — disagreeing with the mock and with each other — which nothing noticed
    /// because nothing read them.
    #[test]
    fn both_code_surfaces_default_to_the_mocks_size() {
        assert_eq!(EditorSettings::default().font_size, DEFAULT_CODE_FONT_SIZE);
        assert_eq!(
            TerminalSettings::default().font_size,
            DEFAULT_CODE_FONT_SIZE
        );
        assert_eq!(DEFAULT_CODE_FONT_SIZE, 12.5, "the mock's mono size");
    }

    /// A hand-edited settings file cannot produce a pane with no visible text.
    ///
    /// Zero is the one that matters: it reaches xterm as a zero-wide cell, which renders as a
    /// blank pane with no error anywhere — indistinguishable from a spawn that failed, a
    /// renderer that died, or a session still connecting.
    #[test]
    fn a_size_that_would_blank_a_pane_is_refused() {
        assert_eq!(clamp_font_size(0.0), MIN_CODE_FONT_SIZE);
        assert_eq!(clamp_font_size(-12.0), MIN_CODE_FONT_SIZE);
        assert_eq!(clamp_font_size(1_000.0), MAX_CODE_FONT_SIZE);
    }

    /// `NaN` gets its own arm because `clamp` propagates it.
    ///
    /// It compares false against both bounds, so `f32::clamp` returns it unchanged — and `NaN`
    /// px makes every cell in the pane `NaN` wide. Reachable from a `settings.json` holding
    /// `null` or a string, which serde will not accept, and from arithmetic upstream, which it
    /// would.
    #[test]
    fn nan_becomes_the_default_rather_than_propagating() {
        assert_eq!(clamp_font_size(f32::NAN), DEFAULT_CODE_FONT_SIZE);
        assert_eq!(clamp_font_size(f32::INFINITY), DEFAULT_CODE_FONT_SIZE);
    }

    /// The fractional default survives a round trip through the wire format.
    ///
    /// This is what `u8` could not do: the field held the project's own design spec and
    /// truncated it. A `12` coming back here would be the old bug, silently restored.
    #[test]
    fn a_half_pixel_size_survives_serde() {
        let mut settings = Settings::default();
        settings.editor.font_size = 13.5;
        let json = serde_json::to_string(&settings).expect("serialize");
        let back: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.editor.font_size, 13.5);
        assert_eq!(back.terminal.font_size, DEFAULT_CODE_FONT_SIZE);
    }

    /// The chrome's default is the size the chrome already drew.
    ///
    /// That equality is what lets [`Settings`] take this field on `#[serde(default)]` with no
    /// migration: a workspace written before the field existed deserializes to the number
    /// `tokens.css` had hard-coded on `html, body` all along, so nothing on screen moves.
    #[test]
    fn the_chrome_defaults_to_the_size_it_already_drew() {
        assert_eq!(Settings::default().ui_font_size, DEFAULT_UI_FONT_SIZE);
        assert_eq!(
            DEFAULT_UI_FONT_SIZE, 13.0,
            "`tokens.css`'s `html, body` base"
        );
    }

    /// The chrome band is deliberately narrower than the code band, and stays that way.
    ///
    /// Pinned as an inequality rather than as two literals because the *relationship* is the
    /// decision: one number here multiplies every chrome size at once, so it cannot be given
    /// the latitude a single scrolling surface gets. Widening this band to the code band is
    /// the change that would make the 8px pin chip disappear and the header eat the window.
    #[test]
    fn the_chrome_band_is_narrower_than_the_code_band() {
        // `const` blocks, so the four facts are checked when the crate compiles and this test
        // is only where they are written down. Clippy asks for it and is right to: an
        // `assert!` over two constants cannot fail at run time, so as a run-time assertion it
        // reads like a check and is a comment.
        const {
            assert!(MIN_UI_FONT_SIZE > MIN_CODE_FONT_SIZE);
            assert!(MAX_UI_FONT_SIZE < MAX_CODE_FONT_SIZE);
            assert!(MIN_UI_FONT_SIZE <= DEFAULT_UI_FONT_SIZE);
            assert!(DEFAULT_UI_FONT_SIZE <= MAX_UI_FONT_SIZE);
        }
    }

    /// A hand-edited settings file cannot hand CSS a multiplier that erases the chrome.
    ///
    /// The `NaN` arm is the one with teeth. This value becomes a divisor in `--ui-scale`, and
    /// an invalid `calc()` is not a wrong size — the engine *drops the declaration*, so all
    /// 405 chrome rules lose their `font-size` together and the window paints in the UA's
    /// default serif at its default size.
    #[test]
    fn a_scale_that_would_erase_the_chrome_is_refused() {
        assert_eq!(clamp_ui_font_size(0.0), MIN_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(-12.0), MIN_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(1_000.0), MAX_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(f32::NAN), DEFAULT_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(f32::INFINITY), DEFAULT_UI_FONT_SIZE);
    }

    /// The chrome size is a scalar, so it moves without carrying a group along with it.
    ///
    /// The point of the field's placement beside `theme`: patching it leaves the two code
    /// sizes exactly where they were. A group would have forced the caller to hold and resend
    /// them, which is the hazard `ui/src/editor/diffViewMode.ts` exists to work around.
    #[test]
    fn a_chrome_size_survives_serde_without_disturbing_the_code_sizes() {
        let settings = Settings {
            ui_font_size: 15.5,
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).expect("serialize");
        let back: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.ui_font_size, 15.5);
        assert_eq!(back.editor.font_size, DEFAULT_CODE_FONT_SIZE);
        assert_eq!(back.terminal.font_size, DEFAULT_CODE_FONT_SIZE);
    }

    /// A workspace written before this field existed still deserializes, and to the old look.
    ///
    /// The whole justification for no `persist.rs` migration, asserted rather than asserted in
    /// prose: `#[serde(default)]` on `Settings` fills the gap, and the value it fills it with
    /// is the one that was hard-coded in the stylesheet.
    #[test]
    fn settings_written_before_the_field_existed_load_unchanged() {
        let mut value = serde_json::to_value(Settings::default()).expect("serialize");
        value
            .as_object_mut()
            .expect("settings is an object")
            .remove("uiFontSize")
            .expect("the field is there to remove");
        let back: Settings = serde_json::from_value(value).expect("deserialize without the field");
        assert_eq!(back.ui_font_size, DEFAULT_UI_FONT_SIZE);
    }
}

/// Git behaviour that is cide's own and not the repository's. (M20)
///
/// # Why this is a group with one field
///
/// A scalar would do today, and `theme` beside it is the precedent for one. It is a group
/// because the *next* two git settings are already visible from here — IDEA's *Update
/// Project* offers a stash-before-update toggle, and the resolver wants its
/// auto-apply-non-conflicting switch — and [`SettingsPatch`](crate::SettingsPatch) patches
/// per top-level field, so promoting a scalar to a group later means every caller that
/// touched the scalar has to be found and rewritten. The cost of being wrong in this
/// direction is one nested struct; in the other it is a migration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct GitSettings {
    /// What a divergent pull does when neither `branch.<name>.rebase` nor `pull.rebase` says.
    ///
    /// **Last** in the resolution order, and that ordering is the design rather than an
    /// implementation detail: `pull.rebase` is a fact about a particular project's workflow,
    /// often set by whoever set the repository up, and a global application preference must
    /// not silently override it. This answers only for the repositories that have not
    /// decided for themselves.
    ///
    /// Defaults to [`PullDefault::Ask`], which is also the only value that can put a dialog
    /// on screen. The dialog's *remember this choice* box does not write here — it writes
    /// `pull.rebase` into the repository, one level up this ladder — so a remembered answer
    /// is scoped to the project it was given for.
    pub pull_strategy: PullDefault,

    /// Apply the changes that only one side made, without being asked, when the three-pane
    /// resolver opens a file.
    ///
    /// IDEA's *Tools | Diff & Merge | Automatically apply non-conflicting changes*, and **off**
    /// by default exactly as it is there. The resolver's centre pane opens as the *base*
    /// revision and every difference either side made is a block you take or reject; applying
    /// two thirds of them before the user has looked would undo the reason for showing them at
    /// all. The toolbar's *Apply non-conflicting* is the same action on demand, and it is one
    /// click.
    pub auto_apply_non_conflicting: bool,
}

// Both fields' defaults are the derive's — `PullDefault::Ask` and `false` — which is why there
// is no hand-written `impl` here. `#[serde(default)]` above is what fills them into a
// `workspace.json` written before this group existed.
