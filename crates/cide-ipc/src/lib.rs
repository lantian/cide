//! Wire types shared by the Rust core and the webview.
//!
//! Every type here is `#[serde(rename_all = "camelCase")]` and derives `TS` so that
//! `cargo xtask codegen` can emit `ui/src/ipc/generated.ts`. Inbound types additionally
//! use `deny_unknown_fields` so a frontend/backend drift fails loudly instead of
//! silently dropping a field.
//!
//! This crate must never depend on `tauri`.

pub mod git;
pub mod gitlab;
pub mod headless;
// The Git tool window: the commit log and its graph, one commit's contents, revisions, blame and
// the commit-level actions. (M18)
//
// Its own module beside `git` rather than a section of it, and its header argues the split at
// length: everything in `git` is organised around a selection that can go stale, and nothing here
// can. Not re-exported at the crate root, on the same rule `git` follows — these names are spelled
// `cide_ipc::history::CommitRow` so the wire's two git vocabularies stay greppable apart. A `///`
// on the `mod` line would resolve this module's own intra-doc links in *this* file's scope, which
// is how the four `unresolved link to LogQuery::limit` warnings appeared and then went away.
/// Image documents. Identity only — never the pixels; the module header says why.
pub mod frame;
pub mod history;
pub mod ids;
pub mod image;
pub mod keymap;
pub mod llm;
pub mod milestones;
/// The New project wizard: a probe of a path, a request, a checklist. (M97)
pub mod new_project;
pub mod overrides;
/// Per-file view memory. Its own module, and deliberately not part of [`workspace`] — the
/// header of `positions.rs` says why at length.
pub mod positions;
/// What a path *is* — the properties card's OS stat, text facts and git summary. (M70)
pub mod properties;
pub mod proposals;
pub mod remote;
pub mod screen;
pub mod settings;
pub mod settings_ops;
/// Editor colour schemes — the `--tk-*` palette, as data. A different axis from [`Theme`],
/// which stays the app's light/dark polarity; the module header argues the split.
pub mod theme;
pub mod workspace;

pub use headless::{HeadlessError, HeadlessRequest, HeadlessResult};
pub use ids::*;
pub use image::{ImageDoc, ImageFormat};
pub use keymap::{Binding, Command, KeymapEdit, KeymapLayer, ResolvedBinding};
pub use llm::{
    LlmModel, LlmProvider, LlmSettings, ModelPool, PoolChoice, PoolEntry, pool_capacity,
};
pub use milestones::{
    CheckResult, GateState, Milestone, MilestonePlan, MilestoneTask, MilestoneTasks,
    MilestonesView, VerifyState,
};
pub use new_project::{
    NewProjectKind, NewProjectOutcome, NewProjectProbe, NewProjectProgress, NewProjectRequest,
    NewProjectStep, NewProjectStepState,
};
pub use overrides::{AgentOverride, AgentOverrides, ProjectOverrides};
pub use positions::{MarkdownView, ViewPosition};
pub use properties::{
    DirSummary, FileProperties, FilePropertiesGit, LineEnding, Owner, PathKind, TextFacts,
};
pub use proposals::{Proposal, ProposalChange, ProposedFile};
pub use settings::{
    ClaudeCli, ClaudeEnvVar, ClaudeInjection, ClaudeInjections, ClaudeSettings, CodexCli,
    CodexInjections, CodexSettings, ConsoleHarness, DEFAULT_CODE_FONT_SIZE, DEFAULT_UI_FONT_SIZE,
    EditorSettings, ExplorerSettings, GitSettings, GraphicsSettings, HighlightLevel,
    InspectionSettings, MAX_CODE_FONT_SIZE, MAX_PUSH_DEBOUNCE_MS, MAX_UI_FONT_SIZE,
    MIN_CODE_FONT_SIZE, MIN_PUSH_DEBOUNCE_MS, MIN_UI_FONT_SIZE, ProxyMode, ProxyScope,
    ProxySettings, ProxyTarget, RemoteBind, RemoteSettings, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
    Settings, SeverityFilter, SidebarSettings, TerminalRenderer, TerminalSettings, clamp_font_size,
    clamp_ui_font_size, normalize_proxy_url, redact_proxy_url,
};
pub use settings_ops::{
    GraphicsRung, GraphicsStatus, KeymapConflict, KeymapEditResult, KeymapProblem, KeymapReport,
    SettingsPatch,
};
pub use theme::{BUILTIN_SCHEME, ColorScheme, SCHEME_SURFACE, SCHEME_TOKENS, scheme_roles};
pub use workspace::{
    DiffAnswer, DiffOrigin, DiffSpec, Direction, DockAnchor, DockSibling, HistoryTab, LayoutNode,
    LogLineDetail, MAX_RATIO, MIN_RATIO, Pane, PaneTree, Project, ProjectRoot, RecentEntry,
    RecentProject, SettingsSection, SpecSubject, TOOL_WINDOW_MAX_HEIGHT, TOOL_WINDOW_MIN_HEIGHT,
    Tab, TabKind, ToolWindowState, WindowRole, Workspace,
};

// --- M8: file tree, watcher, pickers ---
pub mod fs;
pub mod search;

pub use fs::{
    FsChange, FsStatus, NO_ROOT, PasteChoice, PasteCollision, PasteDecision, PasteMode,
    PastedEntry, TreeMatch, TreeMatches, TreeRow, TreeRowKind, WatchBackend, WatchStatus,
};
pub use search::{PickerFrame, PickerItem, PickerRow};
// M11: the content search's own wire types. Same module, different job — see the section
// comment in `search.rs` for why they are not the picker's.
pub use search::{SearchFrame, SearchHit, SearchMode, SearchProblem, SearchQuery};

// --- M12: language support ---
//
// Two modules rather than one, because they answer to different producers and neither knows
// about the other: `symbols` is `cide-lang`'s tree-sitter output, `diagnostics` is the merged
// view over `cide-lsp`, `cide-lang` and a Claude one-shot. The only thing they share is that a
// file has both, and that is not a reason to put them in one file.
pub mod diagnostics;
pub mod docs;
pub mod symbols;

pub use diagnostics::{
    CompletionAnswer, CompletionEdit, CompletionItem, CompletionKind, CompletionResolveAnswer,
    DefinitionAnswer, Diagnostic, DiagnosticKind, DiagnosticSourceId, DiagnosticsSnapshot,
    FormatAnswer, FormatRange, ProbeAnswer, Severity, SourceReport, SourceStatus, Usage,
    UsagesAnswer,
};
pub use docs::{DocsAnswer, DocsLink, DocsMember, DocsSubject, SymbolDocs};
pub use symbols::{
    FileOutline, Symbol, SymbolFrame, SymbolIndexStatus, SymbolKind, SymbolRow, SymbolSpan,
};

// --- M18: subagent orchestration and the task tracker ---
//
// Two modules for the same reason `symbols` and `diagnostics` are two: they answer to different
// producers and neither needs the other's types. `agents` is a process supervisor's view — roles,
// runs, a queue; `tasks` is a committed JSON file in the user's repository. The only thing they
// share is the cross-link, and it is one id each way (`AgentRun::task`, `Task::agent`), carried
// as an id precisely so `cide-agents` never has to hold a `Task`.
pub mod agents;
pub mod tasks;

pub use agents::{
    AgentDef, AgentRoster, AgentRun, DispatchRequest, Harness, LlmLimitsProbe, LlmModelTest,
    LogRunInfo, OrchestrationConfig, OrchestrationPatch, PoolBench, PoolEntryState, PoolEvent,
    PoolEventKind, PoolProviderState, PoolRefusal, PoolRunRef, PoolSkip, PoolSkipped, PoolState,
    PoolStateReport, RunNotify, RunOpen, RunState, TokenUsage,
};
pub use tasks::{
    ATTACHMENTS_DIR, ATTACHMENTS_LEAF, AttachTarget, AttachmentKind, LinkType, StagedFile,
    TASKS_DIR_RELATIVE, Task, TaskAttachment, TaskAuthor, TaskBoard, TaskComment, TaskContent,
    TaskDetail, TaskEdit, TaskFile, TaskLink, TaskLinkSpec, TaskNew, TaskRow, TaskStatus,
    TaskStatusChange,
};

// --- M22: extensions, and the marketplaces they come from ---
//
// Two modules for the reason `symbols` and `diagnostics` are two: `lang` is what a language *is*
// — a tokenizer table, a fold spec, a set of extensions — and it is meaningful with no extension
// anywhere near it, because the builtin languages are described by it too. `ext` is the
// distribution machinery around a manifest that happens to contain some. Keeping them apart is
// what lets `cide-lsp` and the editor depend on a language without depending on the idea of a
// marketplace at all.
pub mod ext;
pub mod lang;

pub use ext::{
    Capability, ConnectRequest, ContributionSource, Contributions, ExtCommandDef, ExtProblem,
    ExtSeverity, ExtensionPage, ExtensionRef, ExtensionSnapshot, InstallRequest,
    InstalledExtension, LanguageBinding, Marketplace, MarketplaceEntry, MarketplaceState,
    PanelBinding, PanelDef, PanelLocation, ResolvedContributions, ResolvedExtCommand,
    ServerBinding,
};
pub use lang::{
    FileExtension, FoldSpecDto, GrammarRule, GrammarSpecDto, LanguageDef, LanguageServerDef,
    RuleAt, ScratchTypeDto,
};

// --- M28: OpenSpec ---
//
// Its own module for `tasks`' reason applied to somebody else's file format: `openspec/` is read
// through the `openspec` CLI and is meaningful with no task anywhere near it, and the only thing
// the two share is one id — `Task::change` — carried so that `cide-spec` never has to hold a
// `Task` and `cide-tasks` never has to know what a requirement is.
pub mod spec;

// What a Docker daemon is running. (M41)
//
// Its own module for `spec`'s reason applied to somebody else's daemon: these are the shapes the
// panels are built against, and the Engine API's own JSON lives in `cide_docker::model`. Not
// re-exported at the crate root — spelled `cide_ipc::docker::ContainerRow`, the rule `git` and
// `history` follow, so that a second vocabulary of rows, states and ids stays greppable apart
// from cide's own.
pub mod docker;

pub use spec::{
    ArtifactState, ChangeSummary, DeltaOperation, SpecAcceptPlan, SpecAccepted, SpecArtifact,
    SpecArtifactRules, SpecArtifactText, SpecBoard, SpecChange, SpecCommand, SpecConfig,
    SpecConfigEdit, SpecDelta, SpecIssue, SpecOperationGuidance, SpecOrigin, SpecProgress,
    SpecRename, SpecRequirement, SpecRequirementSet, SpecScenario, SpecSchema, SpecSummary,
    SpecTask, SpecTouch, SpecValidation, SpecWriteOutcome,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Everything a freshly opened window needs in one round trip.
///
/// One call rather than several: a window that has to make four requests before it can
/// paint will show three intermediate states, and on a slow IPC path that is visible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Bootstrap {
    pub window: WindowLabel,
    pub role: WindowRole,
    pub workspace: Workspace,
    pub keymap: Vec<ResolvedBinding>,
    pub commands: Vec<Command>,
    pub capabilities: Capabilities,
    /// The resolved language, server, panel and command set — builtins merged with whatever the
    /// enabled extensions contribute. (M22)
    ///
    /// Here rather than behind its own command because it cannot be late. `foldSpecFor` is called
    /// inside the editor's mount dispatch and cannot await: a fold spec that arrives one round
    /// trip after first paint means the remembered scroll position is applied against unfolded
    /// heights and the reader lands somewhere they did not leave. The rail has the same problem
    /// in a smaller way — a button that appears a beat after the window does reads as a glitch.
    ///
    /// The *panel* set, not the panels' contents: no extension worker has started at this point
    /// and none needs to have.
    pub extensions: ext::ResolvedContributions,
    /// The colour schemes the user has imported. (M24)
    ///
    /// Here for `extensions`' reason one floor down: the scheme is written onto `<html>` as
    /// custom properties before the first editor mounts, and a palette that arrives a round trip
    /// later is a buffer that paints in the wrong colours and then corrects itself. The compiled
    /// -in `cide` scheme is deliberately *not* in this list — it is what `tokens.css` already
    /// declares, so selecting it means clearing the properties rather than writing any.
    pub schemes: Vec<theme::ColorScheme>,
}

/// Whether a pane's conversation can be picked up where it left off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum SessionRestore {
    /// Claude Code still holds a transcript for this id: spawn with `--resume`.
    Resumable { session: SessionId },
    /// Spawn clean. Every shell, and every Claude pane whose transcript is gone.
    Fresh,
}

/// What should happen to one pane on launch.
///
/// Advice, not instruction: the frontend owns spawning, because only the frontend knows a
/// pane's size, and a child spawned before its slot is laid out draws a ruined first frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PaneRestore {
    /// The window this pane appears in, so a window can keep the entries that are its own.
    pub window: WindowLabel,
    pub project: ProjectId,
    /// The tab holding the pane — for a detached pane, the tab it re-docks into.
    pub tab: TabId,
    pub pane: PaneId,
    pub kind: PaneKind,
    /// Where the session must be spawned for `restore` to hold. Claude Code files a
    /// transcript under the directory it was started in, so resuming from elsewhere finds
    /// nothing.
    #[ts(type = "string")]
    pub cwd: std::path::PathBuf,
    pub restore: SessionRestore,
    /// Whether to spawn on launch rather than waiting to be asked.
    ///
    /// True for exactly one pane per project: the one bound to its primary session. Every
    /// other Claude pane renders a splash with a resume affordance, because reopening a
    /// six-pane project must not silently start six agents at once.
    pub eager: bool,
}

/// What this build can actually do, so the frontend never offers a dead control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Capabilities {
    pub version: String,
    /// `claude --version`, or `None` when the binary is not on `PATH`.
    pub claude_version: Option<String>,
    /// False until the LSP phase; the status bar's diagnostics counts stay a placeholder
    /// and `getDiagnostics` answers empty rather than pretending.
    pub diagnostics: bool,
    /// Whether the IDE-integration MCP server is running for this project.
    pub ide_protocol: bool,
}

/// Result of the mandatory boot-time IPC probe.
///
/// Tauri's fast path (`ipc:` custom protocol) degrades *silently*: if WebKitGTK is older
/// than 2.40 or a CSP rule blocks the scheme, `ipc-protocol.js` catches the fetch
/// rejection, sets `customProtocolIpcFailed = true` permanently and falls back to string
/// `postMessage`. Throughput collapses with no crash and no Rust-side signal, so the
/// frontend has to tell us which path it actually got.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpcHealth {
    /// False means we are on the slow `postMessage` path.
    pub custom_protocol: bool,
    /// Measured throughput of the raw-`Response` pull path.
    pub mib_per_sec: f64,
    /// Reported by the webview, e.g. "2.52.3".
    pub webkit_version: String,
}

/// Terminal geometry, in cells and in cell pixels.
///
/// The pixel fields must be real. Passing zeroes is the usual shortcut and it breaks any
/// program that asks the terminal for its cell size — sixel and inline-image output, and
/// pixel-resolution mouse reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 17,
        }
    }
}

/// Orientation of a split. `Row` places children left/right, `Col` top/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Axis {
    Row,
    Col,
}

/// Which half of a new split the freshly created pane occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Side {
    Before,
    After,
}

/// What a pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PaneKind {
    Claude,
    Shell,
    Diff,
    Editor,
}

/// Whether a pane may be closed.
///
/// The pinned Claude tab's first pane is `Primary`: closing *or detaching* it returns
/// `Err(PanePrimary)` however many panes the tab holds, so the project console can never
/// lose its conversation — not by accident, and not by splitting first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PaneRole {
    Primary,
    Auxiliary,
}

/// What a newly split pane should contain.
///
/// This enum *is* the multiplexing model: "one session but splittable" resolves as one
/// **primary** session plus panes that each declare what they want.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SplitIntent {
    /// `claude --session-id <fresh uuid>` — renders the splash until first input.
    NewClaude,
    /// `claude --resume <src> --fork-session` — shares history up to the split point.
    ForkPrimary,
    /// A second sink on an existing session. No new process.
    ///
    /// `continues` is what the pane keeps for *later*: the harness conversation this session
    /// is a view of, so that once the child ends the pane can re-open the real harness on it
    /// from the right directory (see [`HarnessSession`]). Absent for `claude.mirror` on a pane's
    /// own session, where the pane already knows everything it needs. (M42)
    Mirror {
        session: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        continues: Option<HarnessSession>,
    },
    /// `claude --resume <session>` — a **new child** picking up a conversation that has ended.
    ///
    /// # Why this is not [`Self::Mirror`], and not [`Self::ForkPrimary`]
    ///
    /// `Mirror` attaches a second sink to a session the registry still holds, and spawns nothing;
    /// pointed at a conversation whose child is gone it gives a pane with an empty screen and no
    /// way forward. `ForkPrimary` resumes *and* forks, which mints a second id on purpose so the
    /// two histories diverge — the opposite of picking one up where it left off.
    ///
    /// This one keeps the id. A [`SessionId`] **is** the value cide passes to
    /// `claude --session-id`, which is what makes resume free: the same id spawned with
    /// `--resume` continues the same transcript, and every record that names it — a task's
    /// `session` field above all — goes on naming the right conversation.
    ///
    /// The pane that gets this **owns** its child, unlike a mirror's: it started the process, so
    /// closing the pane must end it. That is why the two cannot share a variant — the ownership
    /// flag the frontend sets is keyed off exactly this distinction, and getting it wrong once
    /// already killed an agent mid-turn (see `cide_app::cmd::agents`' note on `agent_open_pane`).
    Resume { session: SessionId },
    /// The real harness re-opened on a conversation whose child is gone — `claude --resume <id>`
    /// or `opencode --session <id>`, in the directory the conversation was filed under. (M42)
    ///
    /// # Why this is not [`Self::Resume`]
    ///
    /// `Resume` speaks cide's [`SessionId`], which is a *claude* conversation id by construction
    /// and is resumed from the project root. An agent run's conversation is neither: an opencode
    /// run's id is the CLI's own `ses_…`, and both harnesses file the transcript under the run's
    /// **worktree**, so a resume from the root finds nothing at all. [`HarnessSession`] carries
    /// the three facts a re-open needs and the harness spells the command
    /// (`cide_agents::Harness::continue_spec`), so the frontend never learns either CLI's flags.
    ///
    /// The pane that gets this **owns** its child, exactly as `Resume`'s does — it started the
    /// process, so closing the pane ends it — and the ownership flag is set from this intent in
    /// the spawn-plan branch, for the reason recorded beside `agent_open_pane` in
    /// `cide_app::cmd::agents`.
    Continue { conversation: HarnessSession },
    /// `$SHELL -l`
    Shell,
    /// A pane attached to a container — an interactive exec, or a log follow. (M42)
    ///
    /// Carries the container's *name* as well as its id because the pane's title is written at
    /// split time, and asking the daemon for one here would put a round trip inside a layout
    /// mutation. The name is also what [`crate::workspace::DockerPane`] stores and deliberately
    /// never re-resolves — see its own note.
    Docker {
        container: String,
        name: String,
        stream: crate::docker::DockerStream,
    },
    /// A pane for a session that **already exists**, which this pane then owns. (M48, reworked M50)
    ///
    /// # Why a compose run spawns first and splits second
    ///
    /// The first cut carried the argv here and parked it in `layout/spawnPlans.ts` between the
    /// split committing and `TerminalPane` mounting. That is the road `ForkPrimary` and `Mirror`
    /// take, and it has a race those two survive and this one did not: the plan is recorded after
    /// `pane_split` **resolves**, while the pane it is for can render as soon as the
    /// `workspace_changed` broadcast lands — which is sent before the command returns. A mirror
    /// that loses its plan falls through to adopting a held session and looks fine; a compose
    /// pane that lost its plan fell through to `specFor`'s shell arm and opened **a login shell**.
    /// Reported as "compose up does nothing, only opens an empty terminal", and it was exactly
    /// that: an empty terminal.
    ///
    /// So the session is spawned first — an ordinary `session_spawn`, with everything that brings:
    /// the job watcher that lights the pane dot when the run ends, the proxy environment a `pull`
    /// needs, `$EDITOR`, park-across-a-project-switch — and the pane is created already naming it.
    /// `Pane::session` is durable, so no plan has to survive anything.
    ///
    /// # This pane **owns** its child, unlike [`Self::Mirror`]
    ///
    /// The distinction that killed an agent mid-turn once (see `cmd::agents`' note on
    /// `agent_open_pane`): a mirror is a second sink on somebody else's session and closing it
    /// must not end the child, while this pane's session was started *for* it. The frontend sets
    /// `mirrored` from a plan, and this road has no plan — so the flag stays false and closing the
    /// pane ends the run, which is what a person who opened it expects of Ctrl+W.
    ///
    /// # What a restore does
    ///
    /// Nothing re-runs. The id names a session that died with the previous process, so
    /// `sessionIsHeld` refuses it and the pane falls back to a plain shell in the same directory —
    /// with the dead session's parting screen replayed above it by `lifecycle::shell_preload`, so
    /// the compose output the user was reading is still there. A pane that re-spawned its argv at
    /// launch, against a file that may have changed, on a machine just unlocked, is the one
    /// outcome here worth designing against.
    Adopt {
        session: SessionId,
        /// What the pane is called — `compose up : shop`.
        title: String,
    },
    // There is deliberately no `Diff`. A diff pane is not something a *split* can produce:
    // what it renders comes from its tab's `TabKind::Diff { spec }`, and the two gestures
    // that open one — an `openDiff` RPC and the git panel — both create the tab and the pane
    // together (`ide::open_diff_tab`, `cmd::file::tab_open_diff`). The variant existed, was
    // reachable from no code path, and the arm handling it minted a pane titled
    // "<project> : claude — diff" that would have shown nothing: a `PaneKind::Diff` leaf
    // inside a Claude tab has no spec to read.
}

/// A harness's own conversation, and where it has to be re-opened from. (M42)
///
/// The three facts a pane needs to put the **real harness** back on an agent run's conversation
/// after the run's child has gone, and none of the facts it does not: no run id (the run may have
/// left the registry's history by the time the pane comes back after a restart), no argv (the
/// harness spells that, in Rust, at spawn time).
///
/// `id` is a string rather than a [`SessionId`] on purpose. For `claude` it *is* cide's session
/// uuid, because a claude run's session id is the value handed to `--session-id` and `--resume`
/// keeps it. For `opencode` it is the CLI's own `ses_…`, minted by the child and scraped off its
/// output, which no `SessionId` can hold. One field, two spellings, and the harness that wrote it
/// is the one that reads it.
///
/// `cwd` is load-bearing on both harnesses: Claude Code files a transcript under the directory it
/// was started in, and an agent run starts in its worktree (`.cide/worktrees/<role>-<task>`), so
/// `claude --resume` from the project root answers *no such conversation*. It travels here rather
/// than being recomputed from a run because the worktree name is the run's business and the pane
/// outlives the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HarnessSession {
    pub harness: agents::Harness,
    pub id: String,
    #[ts(type = "string")]
    pub cwd: std::path::PathBuf,
}

/// Lifecycle of a session, driven by Claude Code hooks with a PTY-quiet fallback.
///
/// "Live session" for the close-confirm setting means `Busy | AwaitingPermission | Paused` —
/// not "the process exists", which would warn constantly. See [`SessionState::is_live`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
#[ts(export)]
pub enum SessionState {
    Spawning,
    Splash,
    Idle,
    Busy,
    AwaitingPermission,
    AwaitingInput,
    /// `SIGSTOP`ped by cide, and not doing anything until a resume sends `SIGCONT`. (M18)
    ///
    /// # Why there is no payload, when the registry plainly knows more
    ///
    /// The frontend needs exactly **one** fact from the wire here — *this is frozen* — and it
    /// needs it for two jobs: draw the badge, and refuse keystrokes into a pane whose child
    /// cannot read them. Neither job can be done better by knowing what the session was doing
    /// before the freeze.
    ///
    /// What a pane actually wants next is the **post-thaw** state, and that is not something a
    /// payload here could carry honestly: it arrives as its own `cide://session-state` on
    /// resume, after the `SIGCONT`, which is the only moment anybody knows it. So a
    /// `Paused { was: … }` would put a value on the wire that no consumer may act on — a
    /// frontend that redrew from `was` would be redrawing from a state the child has already
    /// left, and a frontend that ignored it would be carrying a field for nothing.
    ///
    /// The pre-freeze state *is* remembered, in `cide_app::agents::AgentRegistry`, in Rust,
    /// beside the `paused_at` it is compared against — because the one decision that reads it
    /// (was this freeze long enough to have killed an in-flight model request?) is taken there
    /// and nowhere else.
    ///
    /// # Not persisted, and pause does not survive a restart
    ///
    /// This enum appears in events and in [`SessionSummary`] and in no `Workspace` field, so
    /// this variant moves no schema version. It could not be persisted usefully in any case:
    /// the shutdown ladder ends every child, and a stopped-then-killed child is just a killed
    /// child.
    Paused,
    Exited {
        code: i32,
    },
}

/// What the registry can still say about a session whose pane was not there to hear it die.
///
/// This is the answer to the *rehydration* question — a pane host evicted and re-created after
/// its child had already gone, so the `cide://session-state` event fired before anything was
/// listening. That path used to ask `session_has_exited`, which answers a `bool`, and so the
/// `— exited —` marker on it never carried a number: the code existed in this process the whole
/// time and was simply never on the wire.
///
/// Four variants because [`cide_pty`] genuinely has four states here and collapsing any two of
/// them loses the code again:
///
/// * `Running` — the child is alive. The pane was rehydrated over a session that outlived it,
///   which is the ordinary case and the reason the question is asked at all.
/// * `Reaping` — `has_exited()` is true but `exit_status()` is still `None`. EOF on the pty
///   master and the reaper's `wait()` are two events on two threads, and this is the window
///   between them. The code exists and is milliseconds away on `cide://session-state`, so the
///   caller must decline to mark and let the event do it.
/// * `Exited` — reaped, with the status `wait()` returned.
/// * `Unknown` — the registry has never held this id. A `SessionId` restored from
///   `workspace.json` after a restart is exactly this: the process that owned it died with the
///   app and no code survives anywhere. **This is the only answer for which a bare
///   `— exited —` is the truth**, and it is why this is a variant rather than an error —
///   `NoSuchSession` would make the one honest case indistinguishable from a failed call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SessionExit {
    Running,
    Reaping,
    Exited { code: i32 },
    Unknown,
}

impl SessionState {
    /// Whether closing this session should prompt for confirmation.
    ///
    /// [`Self::Paused`] answers **true**, which is the one arm worth arguing. A session is only
    /// worth freezing if it had an unfinished turn — nobody pauses an idle prompt — so a paused
    /// session is by construction one the user would mind losing, and `is_live`'s whole job is
    /// "would you mind losing this". Quitting over a freeze is also strictly worse than quitting
    /// over a busy session: the turn is not merely interrupted, it is interrupted in a state the
    /// user deliberately parked and expects to come back to.
    ///
    /// The one production caller is `cide_app::hooks::live_in`, behind `HookServer::live_sessions`
    /// and the close confirm in `cmd::app::live_sessions`; the rest are tests in
    /// `cide_claude::state`. Its map is written only by `hooks::decide` from hook frames, and no
    /// hook frame reports a pause — so the map goes on holding the *pre-freeze* state across a
    /// freeze, which is already the right answer for the close confirm. This arm is what keeps
    /// that answer right if the freeze ever does reach that map.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Busy | Self::AwaitingPermission | Self::Paused)
    }
}

/// Where *Send lines to Claude* actually put the mention.
///
/// # Why the command answers with a pane instead of `()`
///
/// The caller names a pane and the router may not be able to use it — the pane's `claude`
/// has never completed the IDE handshake, most often because that pane is sitting at a
/// resume splash with no process at all. When that happens the mention goes to a Claude in
/// the same project that *can* receive it rather than failing, and the frontend then has two
/// jobs it cannot do without being told which pane was used:
///
/// * **reveal the right one.** The gesture ends by bringing the receiving pane forward and
///   putting the keyboard in it. Revealing the pane that was *asked for* after delivering
///   somewhere else would be strictly worse than the plain error it replaced: the user would
///   be looking at a prompt with nothing in it while their lines sat in another.
/// * **say so.** A selection that lands in conversation B while the user believes it is in A
///   is the one genuinely harmful outcome here, and the only defence is naming the
///   destination out loud.
///
/// [`Self::fallback`] is what separates the two cases, so the ordinary send stays silent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClaudeSendTarget {
    /// The pane the mention was delivered to. Reveal *this* one.
    pub pane: PaneId,
    /// That pane's title, e.g. `cide : claude`. Carried rather than re-derived in the
    /// webview: the two would be read from the same mirror at different instants, and the
    /// only reason this string exists is to appear in a sentence naming where the lines went.
    pub title: String,
    /// Whether [`Self::pane`] is a different pane from the one the gesture named.
    ///
    /// Never true for a send that went where it was aimed, which is the overwhelmingly
    /// common case — so a caller can use it directly as "is there anything to tell the user".
    pub fallback: bool,
}

/// How the app is laid out across OS windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WindowMode {
    /// Every open project docks into the task bar of one window.
    #[default]
    Stacked,
    /// Each project opens as its own OS window.
    PerProject,
}

/// Which theme the webview should paint.
///
/// **Light is the default, by explicit user instruction** — it used to be `Dark`. The
/// default is read in more places than the settings screen: `Settings` is `#[serde(default)]`
/// throughout, so it is also what a `workspace.json` with no `theme` key deserializes to,
/// and it is what `windows.rs` bakes into `?theme=` before a webview exists. Flipping it
/// here is what makes a fresh install open white.
///
/// The variant order is the mock's Appearance segmented control, and it stays Dark-first
/// even though Dark is no longer the default: ts-rs emits the union in declaration order,
/// and reordering would rewrite `generated.ts` for a cosmetic reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Theme {
    Dark,
    #[default]
    Light,
}

#[cfg(test)]
mod theme_tests {
    use super::Theme;

    /// Pins the default, because it is a user decision and nothing else in the tree fails
    /// when it changes: `Settings` is `#[serde(default)]`, so a flip back to `Dark` would
    /// silently change what a fresh install paints and what `windows.rs` writes into
    /// `?theme=`, with every existing test still green.
    #[test]
    fn the_default_theme_is_light() {
        assert_eq!(Theme::default(), Theme::Light);
    }
}

/// A session that would be interrupted by closing something.
///
/// Carries enough to name it on screen without a second round trip. A dialog that says "3
/// sessions are busy" and cannot say *which* leaves the user no way to decide, so the pane
/// title and project name travel with the answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionSummary {
    pub session: SessionId,
    pub project: ProjectId,
    pub project_name: String,
    pub pane_title: String,
    pub state: SessionState,
}

/// How much of one project is *working* right now — the header project tab's badge. (M94)
///
/// Two numbers rather than one, because the tooltip has to name both halves: "2 running" over a
/// project with two agent runs and a console the user is typing in is a number they cannot act
/// on, and the chip itself is capped at `9+` so the sentence is the only place the real figure
/// appears.
///
/// # Working, not merely alive
///
/// `crate::app`'s two predicates decide it (`cide_app::running`), and they are the spinner's own
/// — a badge computed from a fourth definition of *busy* would disagree with the auto-spin timer
/// on exactly the days the exceptions matter. An idle console at a prompt is not counted, and
/// neither is a session that has finished its turn and is waiting for the user: that is the
/// *awaiting* marker's job (`cide://session-awaiting`), and the two chips sit side by side on one
/// tab. They must never describe the same session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectRunning {
    pub project: ProjectId,
    /// Agent runs holding a child or a claim on one.
    pub runs: u32,
    /// Console panes mid-turn, at a permission prompt, frozen, or still starting.
    ///
    /// **De-duplicated by session**: a mirrored pane is two panes showing one child, and a
    /// count that said `2` about it would be counting windows onto a thing rather than the
    /// thing — `ui/src/panes/awaitingRule.ts` argues the same point for the marker beside it.
    pub panes: u32,
}

/// Every project with something running, and the generation that produced the answer.
///
/// One type for the event payload **and** the catch-up command's reply, so a window cannot parse
/// the broadcast and the answer differently.
///
/// **Projects with nothing running are omitted.** [`crate::SessionSummary`]'s neighbour
/// `cide://session-awaiting` established the rule and it is the same one: the set is complete by
/// construction, so absence is the message "nothing running here", and the reader needs no
/// second notion of *unknown*.
///
/// # Why there is a generation on it
///
/// `at` is a process-wide counter, not a clock. `cide://project-running` is a *change*
/// notification, so a window that opened between two changes has heard nothing and must ask —
/// and the reply describes the set as it was when the question landed, which a broadcast can
/// overtake on the way back. Without a number to compare, such a window settles on a stale count
/// and stays there until the next genuine change, which for a long-running agent is many minutes
/// of a wrong badge with nothing logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectRunningSet {
    pub projects: Vec<ProjectRunning>,
    /// `number` and not ts-rs' default `bigint` for a `u64`, which is the right call here and
    /// not everywhere: this one is *compared*, on the receiving side, against a sentinel the
    /// frontend starts at `-1`, and a `bigint < number` comparison is a `TypeError` at runtime
    /// rather than a type error at build. It is a counter that would take a million years at
    /// one bump a microsecond to leave the exact-integer range, so nothing is lost.
    #[ts(type = "number")]
    pub at: u64,
}

/// A file tab whose buffer holds edits that have not been written to disk.
///
/// The path travels with it for the same reason [`SessionSummary`] carries a pane title: a
/// dialog that says "3 unsaved files" and cannot say *which* gives the user no basis to
/// choose between discarding and going back. `title` is the basename the tab strip already
/// shows, so the dialog and the strip name the same thing; `path` disambiguates the two
/// `mod.rs` the user has open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UnsavedTab {
    pub tab: TabId,
    pub project: ProjectId,
    pub project_name: String,
    #[ts(type = "string")]
    pub path: std::path::PathBuf,
    /// The tab strip's label — the file's basename.
    pub title: String,
}

/// Whether closing may proceed, and what it would cost.
///
/// Both lists are empty in the common case, and that is the case where no dialog should
/// appear at all. A confirmation that fires whenever a process exists is one users learn to
/// dismiss without reading, which is worse than none.
///
/// The two lists are separate because the two risks are not the same risk, and the
/// confirmation has to word them differently:
///
/// * `unsaved` is **destruction**. Those edits exist nowhere else; closing loses them.
///   Always reported, whatever the settings say — see `Settings::
///   confirm_close_with_live_session`, which is about sessions and has no authority here.
/// * `blocking` is **interruption**. A `Busy` or `AwaitingPermission` session loses its
///   turn, not its transcript: the conversation resumes with `claude --resume`. This list
///   is governed by that setting, and is empty when the user has turned it off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuitDecision {
    pub blocking: Vec<SessionSummary>,
    pub unsaved: Vec<UnsavedTab>,
}

impl QuitDecision {
    /// Whether anything at all is at risk. `false` means close without asking.
    pub fn is_clear(&self) -> bool {
        self.blocking.is_empty() && self.unsaved.is_empty()
    }
}

/// What a split produced.
///
/// The resolved intent travels back with the pane id because the caller may not know it: a
/// `None` intent asks the domain for the tab's default, and the frontend still has to spawn
/// the right thing afterwards. Returning only the id would leave it guessing — and guessing
/// `NewClaude` where the domain said `ForkPrimary` starts a fresh conversation where the
/// user asked to branch an existing one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SplitOutcome {
    pub pane: PaneId,
    pub intent: SplitIntent,
}

/// What a Back or Forward into a file actually did. (M16)
///
/// Tagged rather than a bare `TabId`, and an `Ok` rather than an `Err`, because the three
/// outcomes are three different things to *say* and only one of them is a failure — which none
/// of them is. `cmd::file::tab_reopen_file` argues the split at length; the short version is
/// that "that file is no longer there" belongs in an informational notice, exactly where
/// `tab_reopen_closed` puts the same fact, and putting it on the failure toast would make an
/// ordinary consequence of deleting a file look like a bug in the app.
///
/// [`Self::Restored`] and [`Self::Opened`] are distinguished for the user's sake and not the
/// caller's: both end in a hydrate. The distinction is kept because it is the observable
/// difference between Back putting a *split* tab back — panes, ids, live Claude sessions — and
/// Back minting a fresh single-pane editor, and a wire type that could not express it would make
/// that regression invisible to every check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ReopenedFile {
    /// The closed-tab record was spent: the tab is back with its original pane tree.
    Restored { tab: TabId },
    /// An ordinary open — there was no record for this path, or the tab was open already and
    /// was activated. The record, if there was one, is deliberately still on the stack.
    Opened { tab: TabId },
    /// Nothing is at that path any more. No tab was opened and nothing was consumed.
    Gone { path: String },
}

/// What rides beside a file's bytes on the way **to** the webview. (M63)
///
/// The head of the frame `file_read_bytes` answers with (`cide_ipc::frame`): the two facts
/// [`FileDoc`] carries besides its text, from the same `metadata` call that read the bytes.
/// The drawing pane's autosave compares against `stamp` exactly as the editor's does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileBytesHead {
    /// See [`FileDoc::writable`] — the mode bits, and the dependency-cache rule applied.
    pub writable: bool,
    /// See [`FileDoc::stamp`].
    pub stamp: Option<FileStamp>,
}

/// What rides beside a file's bytes on the way **from** the webview. (M63)
///
/// The head of the frame `file_write_bytes` is handed. The path is in the head and not in a
/// request header because a header value is visible ASCII and a path is not — see
/// `cide_ipc::frame`'s module comment. `if_unchanged` has `file_write`'s meaning exactly:
/// `None` writes unconditionally (Ctrl+S), `Some` is the autosave precondition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileBytesWrite {
    pub path: String,
    pub if_unchanged: Option<FileStamp>,
}

/// A text file as the editor loads it. (M9)
///
/// The text is the file's bytes verbatim, line endings included. Normalising here would be
/// the convenient thing and the wrong one: CodeMirror normalises again on `EditorState`
/// creation, so the only place that can tell a CRLF file from an LF one is the side holding
/// the original — and if that is not the editor, every save rewrites every line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileDoc {
    pub path: std::path::PathBuf,
    pub text: String,
    /// False when the file's mode bits carry no write permission at all.
    ///
    /// This is exactly `Permissions::readonly()` inverted, and that is a narrower question
    /// than "can this process write it": it reads the mode and nothing else, so a file
    /// owned by another user with mode 0644, or any file on a read-only mount, still comes
    /// back `true`. Purely advisory — the write is attempted regardless and reports its own
    /// failure — but it catches the common `chmod -w` case early rather than letting the
    /// user type for ten minutes into something that will not save.
    pub writable: bool,
    /// What the file was when this buffer was read. See [`FileStamp`].
    ///
    /// `None` when the metadata could not be read as a stamp — a filesystem with no mtime, a
    /// platform that reports one before the epoch. A `None` here means the precondition below
    /// simply cannot be checked, and the write proceeds unconditionally: refusing to save
    /// because a `stat` was unhelpful would be worse than the race it is guarding against.
    pub stamp: Option<FileStamp>,
}

/// `u64` nanoseconds as a decimal string on the wire. See [`FileStamp::mtime_nanos`].
mod nanos_as_string {
    pub fn serialize<S: serde::Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        use serde::Deserialize as _;
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A cheap "is this still the file I read" token: modification time and length.
///
/// # Why autosave needed this and Ctrl+S did not
///
/// `EditorPane` raises its conflict bar from `cide://session-tool`, which arrives as a Claude
/// Code tool call completes. That is the fast path and it is deliberately not the whole story:
/// a `sed -i`, a `cargo fmt` or a `git checkout` in a shell pane goes through no tool and raises
/// nothing. Before autosave, clobbering such a change took a deliberate Ctrl+S. With
/// autosave-on-blur it takes *switching to the terminal, running `cargo fmt`, and clicking back*
/// — three things a person does without deciding anything.
///
/// So the buffer carries what the file was when it was read, and hands it back on the write. A
/// mismatch is refused with [`crate::FileWriteRefusal::Changed`], which `EditorPane` turns into
/// the same conflict bar the tool path raises. An explicit Ctrl+S passes no token and forces,
/// because that is the user deciding.
///
/// # Why not subscribe the editor to `cide://fs-changed`
///
/// Because our own write emits one. A watcher subscription needs a self-write suppression
/// window, and a suppression window is a race with a timer in it — precisely the shape that
/// ships broken. A precondition compared inside the same `write` call has no window at all.
///
/// Mtime **and** length, not mtime alone: coarse-grained filesystems and fast tools can produce
/// two writes inside one mtime tick, and the length is free (`metadata` already has it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileStamp {
    /// Nanoseconds since the epoch. A `u64`, which runs out in the year 2554.
    ///
    /// **Carried as a decimal string, because it does not fit in a JavaScript number.** The
    /// comment here used to claim it was "far inside JavaScript's exact-integer range", and that
    /// arithmetic was simply wrong: nanoseconds since the epoch is around 1.79e18 today, while
    /// `Number.MAX_SAFE_INTEGER` is 9.007e15 — about two hundred times smaller. The reasoning
    /// held for seconds, or even milliseconds, and was carried over to a unit it is false for.
    ///
    /// The consequence was total rather than marginal. Tauri's transport is JSON in both
    /// directions, so the stamp handed to the webview was silently rounded to a multiple of
    /// 256 ns, and the token the webview passed back therefore never equalled the file's real
    /// stamp. `write_if_unchanged` refused **every** autosave on any filesystem with
    /// nanosecond mtimes — ext4, btrfs, xfs — and the refusal is indistinguishable from a real
    /// conflict, so the user was shown "this file changed on disk" about a file nothing had
    /// touched, on a timer, for ever.
    ///
    /// A string round-trips exactly and stays opaque: the frontend only ever hands it back.
    /// Reducing the precision instead would have made the check *miss* a real edit inside the
    /// rounding window, which is the failure this stamp exists to prevent.
    #[serde(with = "nanos_as_string")]
    #[ts(type = "string")]
    pub mtime_nanos: u64,
    #[ts(type = "number")]
    pub len: u64,
}

#[cfg(test)]
mod file_stamp_tests {
    use super::FileStamp;

    /// The stamp survives a JSON round trip **exactly**, at a real present-day value.
    ///
    /// This is the regression test for a total failure, not a rounding nicety. `mtime_nanos` was
    /// declared to TypeScript as a `number`, on a comment claiming the value sat "far inside
    /// JavaScript's exact-integer range". Nanoseconds since the epoch is ~1.79e18 and
    /// `Number.MAX_SAFE_INTEGER` is 9.007e15, so every stamp that crossed the IPC boundary came
    /// back rounded — and `write_if_unchanged`, which compares the returned token against the
    /// file's real stamp, refused **every** autosave on ext4, btrfs and xfs while telling the user
    /// their file had changed on disk.
    ///
    /// The literal is a genuine ext4 nanosecond mtime rather than a round number, because a value
    /// ending in zeros is exactly the one that would survive the rounding and pass a broken
    /// implementation.
    #[test]
    fn a_nanosecond_stamp_survives_the_wire_exactly() {
        let stamp = FileStamp {
            mtime_nanos: 1_786_998_123_456_789_123,
            len: 4096,
        };
        let wire = serde_json::to_string(&stamp).expect("serialises");

        // A string, and asserted as one: this is the property, not an implementation detail. A
        // future "tidy" back to a bare integer is precisely the regression.
        assert!(
            wire.contains("\"1786998123456789123\""),
            "the stamp travels as a decimal string, not a JSON number: {wire}"
        );

        let back: FileStamp = serde_json::from_str(&wire).expect("deserialises");
        assert_eq!(
            back.mtime_nanos, stamp.mtime_nanos,
            "not one nanosecond lost"
        );

        // And the value really is past what a double can hold, so the test is testing something.
        const JS_MAX_SAFE: u64 = 9_007_199_254_740_991;
        assert!(stamp.mtime_nanos > JS_MAX_SAFE);
        assert_ne!(
            stamp.mtime_nanos as f64 as u64, stamp.mtime_nanos,
            "a double genuinely cannot hold this — if this ever passes, the premise has changed"
        );
    }
}
