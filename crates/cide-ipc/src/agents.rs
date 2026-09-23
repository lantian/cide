//! Subagent orchestration's wire types: the roles a project defines, the runs cide has
//! dispatched, and the per-project switch that governs both. (M18)
//!
//! A *run* is one execution of one role. It is hosted as a real interactive PTY session — the
//! same `SpawnSpec` → `PtySession::spawn` → `SessionRegistry::insert` path a pane uses, headless
//! only in the sense that no pane is attached to it yet — so everything here describes a session
//! plus the queue position, task link and pause state a session has no vocabulary for.
//!
//! # Two properties everything in this module follows from
//!
//! **Subagents are off by default and enabled per project.** An IDE that starts spawning
//! unattended agents in somebody's repository because they upgraded is not acceptable, so
//! [`OrchestrationConfig::enabled`] is `false` in the absence of a config file and
//! [`AgentRoster::Disabled`] is the state most users will ever see. It is therefore a designed
//! state with prose in it, not an empty list.
//!
//! **The roster is derived, never persisted.** `.cide/config.json` and `.cide/agents/*.md` are
//! committed files that a teammate's commit or a `git checkout` can change under the running
//! app; they are read fresh and reach the frontend through `agents_roster` and
//! `cide://agents-changed`. Nothing here is mirrored into `Workspace`, which would make cide's
//! own state file hold a stale copy of something git owns.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{AgentId, ProjectId, RunId, SessionId, TaskId};

/// Which CLI actually runs a role.
///
/// An enum and not a string because it selects an implementation — `cide_agents::harness`
/// dispatches on it — and because the settings surface and the definition parser must agree on
/// the set. A role naming a harness this build does not have is a *parse* error with a line
/// number, which is only possible if the set is closed.
///
/// More are expected; adding one is a variant here, an `impl Harness` there and one insert into
/// the registry. That is the shape the trait was chosen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Harness {
    Claude,
    Opencode,
    /// Qwen Code (`qwen`). (M43) Claude-shaped on the outside — `--session-id`/`--resume`
    /// uuids, `--append-system-prompt`, an inline `--mcp-config`, tools named
    /// `mcp__<server>__<tool>` — and hosted like `claude`: an interactive TUI in the PTY, with
    /// the run's state read from the JSON event file the CLI writes beside it
    /// (`--json-file`), because its hooks can only be configured in settings files cide will
    /// not edit.
    Qwen,
    /// OpenAI's Codex CLI (`codex`). (M44) Hosted like `opencode`, not like `qwen`: one
    /// `codex exec --json` child per turn, the run's state read off the JSONL it prints, the
    /// thread id captured from its first event because the caller cannot choose one, and a
    /// follow-up a fresh `codex exec resume <id>` child. The role's brief travels as a
    /// `-c developer_instructions=` override and cide's tracker as a `-c mcp_servers.cide.*`
    /// one — with the run's environment spelled into it, because codex hands an MCP server a
    /// whitelisted environment rather than its own. `codex resume <id>` is the real TUI a
    /// person re-opens a finished run in.
    Codex,
    /// Xiaomi's MiMo Code (`mimo`). (M81) **An opencode fork**, measured on 0.1.15: `mimo run`
    /// takes opencode's flags, prints opencode's `--format json` events byte for byte in shape,
    /// names MCP tools `<server>_<tool>`, and reads every environment variable opencode does
    /// under a `MIMOCODE_` prefix. So it is not a fifth implementation but a second *flavour* of
    /// `cide_agents::harness::opencode` — one renderer, one failover classifier, one provider
    /// document — and it takes everything [`Harness::reads_provider_document`] grants.
    Mimo,
}

impl Harness {
    /// Whether this harness is fed cide's LLM **provider document** — and therefore takes a
    /// model pool, fails over, folds in its own default model, and is a different child when a
    /// provider changes. (M81)
    ///
    /// One producer for a question that was four `== Harness::Opencode` tests in
    /// `cide_agents::overrides` and two more in the app, which is four places for a second
    /// opencode-shaped CLI to be silently left out of: a pool named on a mimo role would have
    /// been reported *inert* by the roster and ignored by the fork, with nothing logged.
    #[must_use]
    pub const fn reads_provider_document(self) -> bool {
        match self {
            Harness::Opencode | Harness::Mimo => true,
            Harness::Claude | Harness::Qwen | Harness::Codex => false,
        }
    }
}

/// The colours a role may be drawn in, in the order a derived colour indexes them. (M75)
///
/// **Claude Code's own `color:` vocabulary**, deliberately, so a subagent that already declares
/// one needs no translation table and no second spelling. `ui/src/styles/tokens.css` declares
/// `--agent-<name>` for every entry in **both** palette blocks, and `AgentsPanel/model.ts`
/// restates this list; `check:agents` scrapes this constant and asserts the two are equal, and
/// `check:theme` asserts every name resolves to a token under both themes.
///
/// **The order is API.** A role with no `color:` of its own is drawn in the hue this list's
/// *index* answers for a hash of its name, so reordering this list silently recolours every
/// uncoloured role in every tracker. Append, never reorder — and appending is not free either,
/// since the modulus moves with the length.
///
/// Eight, and not nine, because the ninth colour is the one reserved for the orchestrator:
/// `--agent-orchestrator` is outside this list precisely so that no role can ever be given it,
/// which is a property this list's *absence* of an entry enforces and a rule beside it would not.
pub const AGENT_HUES: &[&str] = &[
    "blue", "green", "orange", "purple", "cyan", "red", "yellow", "pink",
];

/// One of [`AGENT_HUES`] for a value a definition wrote, or `None` for anything else.
///
/// Trimmed and lowercased first, because a person types `Blue` and Claude Code's own UI offers
/// `automatic` — which is not a colour and correctly answers `None`, meaning *derive one*. Every
/// other miss (a hex, a typo, a colour Claude adds next month) takes the same road for the same
/// reason: see [`AgentDef::color`] for why a bad value here is never an error.
#[must_use]
pub fn hue(raw: &str) -> Option<String> {
    let folded = raw.trim().to_ascii_lowercase();
    AGENT_HUES
        .iter()
        .find(|name| ***name == *folded)
        .map(|name| (*name).to_string())
}

/// One role, as the panel draws it.
///
/// The wire projection of `.cide/agents/<name>.md` (or its global twin beside `keymap.json`) —
/// not that file's parse tree. The fields the definition carries that only a spawn cares about
/// (`tools`, `permission-mode`, `effort`, the origin path) stay in `cide-agents`, because a
/// roster row cannot act on them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentDef {
    /// The role name, which is also the file stem — see [`AgentId`].
    pub id: AgentId,
    /// What the row says. Title-cased from the id unless the file gives one, so a roster is
    /// readable before anybody has thought about presentation.
    pub label: String,
    /// Which of the four directories this definition was read from. (M30)
    ///
    /// # Why a roster row carries it when `origin`/`shadows` deliberately do not
    ///
    /// `LoadedAgent`'s doc has flagged for two milestones that the panel genuinely wants those
    /// two paths and that adding them "would be a reasonable addition to `AgentDef` when someone
    /// is next in there". This is the half that earned its way across, and a *path* is still not
    /// what crosses: a scope is a closed enum the frontend can switch on, where a path is a
    /// string it would have to parse to learn anything.
    ///
    /// What reads it is the roster's badge: a [`AgentScope::is_claude_code`] role is drawn
    /// **Claude Code**, because a definition cide did not author, cannot fully model, and applies
    /// through `--agent` rather than through its own flags is a fact the user has to be able to
    /// see without opening anything.
    ///
    /// **It does not replace the Settings screen's per-scope probing**, and that is worth saying
    /// because it looks as though it should. That screen issues one `agents_draft` per scope per
    /// name, which is not a lookup of something this field already knows: a roster row is the
    /// *merged* answer, one entry per name, so it names the scope that **won** and is silent
    /// about the file it shadowed. Listing both is the whole point of that screen — a shadowed
    /// definition must never be invisible — so the probe answers a question this field cannot.
    pub scope: AgentScope,
    pub harness: Harness,
    /// One line, from the front matter's `description`. What the role is *for*, in the author's
    /// own words — the orchestrator is given this verbatim when it asks what agents it has, so
    /// it is prompt text as much as it is UI text.
    pub description: String,
    /// The role's system prompt: the body of the markdown file, verbatim.
    ///
    /// **Carried on the wire although nothing in the frontend executes it**, because the panel
    /// shows it folded and read-only. It is the only way a user can tell two roles apart once
    /// they have edited the file — `description` is one line the author wrote early and rarely
    /// revisits, while the prompt is the thing they actually tune. A roster that lists names
    /// without them is a list of words.
    ///
    /// Read-only in the panel, deliberately: an editor here would be a second way to write a
    /// committed file, racing `git` and the user's own editor for it.
    pub system_prompt: String,
    /// A model alias or full name from the definition. `None` means the harness's own default,
    /// which is the right answer for a role whose author did not care.
    pub model: Option<String>,
    /// The role's colour, as one of the eight hue names in `AgentsPanel/model.ts`'s `AGENT_HUES`
    /// — or `None`, which means *derive one from the name*. (M75)
    ///
    /// # A presentation hint that cide reads and never writes
    ///
    /// `color` is **Claude Code's own front-matter key**, and [`AgentExtra`] already carries it
    /// through a save untouched — `claude_corpus` asserts it by name among the keys cide does not
    /// model. That does not change here, in either dialect: the key stays an extra, `render` goes
    /// on emitting it verbatim from the source lines it occupied, and this field is a *read* of
    /// it taken on the way past. Modelling it properly would mean cide re-spelling a key with its
    /// own quoting rules in a committed file it did not author, to gain nothing a colour needs.
    ///
    /// So the value is normalised (trimmed, lowercased) and matched against the eight names; a
    /// value that is not one of them — `automatic`, a hex, a typo, a colour Claude adds next
    /// month — is `None` and the name's hash answers instead. Never a
    /// [`Self::unavailable`] sentence: a role that could not be dispatched over a colour would be
    /// absurd, and a role with no colour is exactly the ordinary case.
    ///
    /// It is deliberately **not** in `cide_agents::overrides::ChildSettings`. That type's
    /// equality decides whether a paused run must be re-forked, and recolouring a role must not
    /// cost somebody an in-flight turn.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Why this role cannot be dispatched, as a sentence — or `None` when it can.
    ///
    /// **This is [`crate::Command::unavailable`] one layer down, and for the identical reason.**
    /// A role whose harness binary is not on `PATH`, whose system prompt is empty, or which two
    /// config files declared under one name, is drawn greyed **with this sentence** and gets no
    /// Dispatch button at all — never a button that is offered and then refused, and never a
    /// greyed row with nothing saying why.
    ///
    /// This project has paid for the listed-and-silently-inert state twenty-four times over
    /// (`cide_core::commands`' note records the count), and the answer there is the answer here:
    /// make it unrepresentable. `AgentsPanel/model.ts`'s `canDispatch` returns exactly one of a
    /// green light or a sentence, never neither, and `check-agents.mjs` fails if it can return
    /// both.
    ///
    /// A role is never *hidden* for being unavailable. "We could not tell" and "it is not there"
    /// must stay distinguishable, and a role that vanished when its CLI was uninstalled would
    /// look like a role the user had deleted.
    pub unavailable: Option<String>,
    /// How many runs of this role may be live at once.
    ///
    /// Per role rather than only globally, because roles differ: a `qa` that reads and reports
    /// parallelises more safely than a `developer` rewriting files. The declared number is the
    /// enforced number under both isolations — worktrees are per (role, task), so parallel
    /// tasks are parallel checkouts and the old clamp-to-1 is gone with its premise; what
    /// cannot run twice is two children in *one* checkout, which the registry's admission gate
    /// holds in the queue.
    pub max_concurrent: u16,
    /// `worktree:` — whether this role's runs take a worktree under the project's worktree
    /// isolation, default `true`.
    ///
    /// `false` opts the role out: its runs stand in the **project root**, on the user's own
    /// checked-out branch, beside the user's own uncommitted changes. That is the right shape
    /// for a role that only reads — a reviewer, a reporter — and the wrong one for anything
    /// that edits, which is why it is per role and opt-*out*: the safe posture stays the
    /// default and the file has to say otherwise. An opted-out role has no `cide/<role>`
    /// branches and nothing to integrate — its work, if any, lands directly where the user is.
    /// Under `isolation: shared` the flag is meaningless and ignored.
    #[serde(default = "default_worktree")]
    pub worktree: bool,
}

/// `AgentDef::worktree`'s default, for files and wire payloads from before the field existed.
fn default_worktree() -> bool {
    true
}

/// Where one run is.
///
/// # Deliberately shaped like [`crate::SessionState`], and deliberately not that type
///
/// A run *is* a session plus a queue position, so the resemblance is not accidental and the
/// vocabulary is kept aligned on purpose — `Running` here is `Busy` there, `AwaitingPermission`
/// is the same fact under the same name, and `Finished { code }` is `Exited { code }`. Both are
/// driven by the same hook frames through `cide_claude::next_state`.
///
/// The one place the vocabularies deliberately *diverge* is [`Self::Idle`], which collapses
/// `SessionState`'s `Idle` and `AwaitingInput` into a single arm. Its doc says why, at the
/// length the question deserves, because it is the kind of asymmetry that gets tidied away.
///
/// They are still two enums, because **three of these states exist before or outside a child
/// process**: `Queued` has no process yet, `Paused` describes a process that is frozen rather
/// than doing anything, and `Failed` covers the run that never started — no worktree, no binary,
/// a spawn that returned an error. `SessionState` has no vocabulary for any of them, and it
/// should not: it answers "what is this terminal doing".
///
/// Two enums describing overlapping facts is a real cost and it is taken knowingly. The
/// alternative was adding `Queued`/`Paused` to `SessionState`, and *every* existing consumer
/// would then have to grow an arm for a state it can never see — `awaitingRule.ts`,
/// `exitMarker.ts`, `restartRule.ts`, `paneHosts::setHostBusy` — which is four places to get a
/// subagent-only concept wrong in a pane that has nothing to do with subagents. (`Paused` does
/// reach `SessionState`, because a paused *session* is a fact a pane must draw; what does not
/// reach it is the queue.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
#[ts(export)]
pub enum RunState {
    /// Dispatched, waiting on a concurrency slot. No child, no session, no Open action.
    Queued,
    /// Spawning: the worktree is being prepared and the child has not reported in.
    Starting,
    /// A turn is in flight.
    Running,
    /// The harness handed the turn back and is waiting for input. **The child is alive.**
    ///
    /// # Why this is not `Finished { code: 0 }`, which is what it used to be
    ///
    /// The mapping this variant replaces wrote `Finished { code: 0 }` for a `claude` sitting at
    /// its prompt, and that put an **exit status on the wire for a process that had not exited**.
    /// Anything downstream reasoning about `code` — the row that prints it, a "did this run
    /// succeed" test somebody writes later, a log line — was reasoning about a number no child
    /// ever produced. A zero nobody computed is worse than no zero at all: it is byte-identical
    /// to a clean exit, which is the one thing it is not.
    ///
    /// The two facts are different and slicing them apart is the point:
    ///
    /// * **the turn is over and the child is alive** — this state. It is the moment the
    ///   orchestrator is nudged, the moment a follow-up prompt can be written to the child's
    ///   stdin (`cide_agents::Delivery::Stdin`, which needs a live process to write into), and
    ///   the moment the run stops occupying a *working* slot.
    /// * **the process exited** — [`Self::Finished`], and nothing else. It is the moment the
    ///   worktree is releasable and the transcript is final.
    ///
    /// # The queue consequence, which is the reason the old mapping existed
    ///
    /// That reason was real and it survives here unchanged. A run left `Running` while the CLI
    /// sits idle at a prompt holds its concurrency slot — and, under worktree isolation, its
    /// role's *only* worktree — for as long as the child lives, and the next queued task never
    /// starts. That is not a display detail; it is the queue stalling with nothing on screen to
    /// explain it.
    ///
    /// So `Idle` sits outside the live set exactly where `Finished` sat, and the registry
    /// releases the slot on entering it — **without** claiming the process is gone. What the old
    /// mapping could not express is that releasing the *slot* and releasing the *worktree* are
    /// two decisions: an idle child is still sitting in that checkout, so the queue either
    /// delivers the next turn into it or winds it down first, and it now has a state in which to
    /// tell the difference.
    ///
    /// # One variant where [`crate::SessionState`] deliberately has two
    ///
    /// `SessionState` distinguishes `Idle` — not working — from `AwaitingInput` — the turn ended
    /// and the conversation was handed back. That distinction earns its keep there: it is what
    /// raises the finished-work marker in a pane, and `cide_claude::next_state`'s own comment
    /// records the measurement that made it necessary.
    ///
    /// A run has **no human at its keyboard**. Nobody is going to notice the prompt and type into
    /// it; the only thing that can move an idle run is cide. So "waiting for the user" and
    /// "waiting for anything" are the same state to a queue, and two arms with identical
    /// handling in every match is the shape that rots — the next reader cannot tell which one to
    /// write, and picks by coin toss.
    ///
    /// Written down because the asymmetry reads like an oversight from either end, and somebody
    /// will otherwise "align" the two enums by adding an `AwaitingInput` here. It is not an
    /// oversight.
    Idle,
    /// The harness asked for permission and nothing is happening until somebody answers.
    ///
    /// The one run state that is a call to action, which is why the activity-rail badge draws it
    /// in the error tone rather than the ordinary count tone.
    AwaitingPermission,
    /// SIGSTOPped. `since_unix_ms` is what the elapsed figure counts from, and what decides
    /// whether the thaw is long enough to suspect a timed-out turn.
    Paused { since_unix_ms: u64 },
    /// The run outlived its process: cide restarted (or quit) while this run was open, the
    /// shutdown ladder ended the child, and the registry's snapshot brought the row back.
    ///
    /// # Not `Failed`, and not `Paused`, and both distinctions are load-bearing
    ///
    /// `Failed` closes a run — it may be dispatched again but never continued — and this run's
    /// conversation is intact: a `claude` child was given the ladder's grace precisely so it
    /// could finish writing the transcript `--resume` depends on, and an `opencode` conversation
    /// lives in that CLI's own store. `Paused` claims a frozen **live** child (`SIGCONT` is its
    /// resume), and there is no process here at all. What resuming this state means is a *new
    /// child continuing the old conversation*, through the same admission gate as any start —
    /// an interrupted run holds no slot, and its role's worktree may have been given to a newer
    /// run in the meantime, so it queues like anything else.
    ///
    /// The child is gone, so there is no session to open, nothing to pause, and no exit code —
    /// the ladder's kill is cide's own act, not a fact about the work.
    Interrupted,
    /// The child exited. `code` is the real status, kept for the reason
    /// [`crate::SessionExit`]'s doc gives: a bare "exited" with no number is the one thing a
    /// finished row cannot be read from.
    Finished { code: i32 },
    /// The run never got to a child, or died in a way that is not an exit code: no git worktree,
    /// a harness binary that disappeared between the roster and the dispatch, a spawn error.
    /// `reason` is shown on the row.
    Failed { reason: String },
}

impl RunState {
    /// Whether a run in this state is doing work that its **worked clock** should count.
    ///
    /// The one definition, and it is exhaustive so a ninth variant is a compile error here
    /// rather than a state silently counted as one thing or the other. Read by
    /// `cide_app::agents::move_to`, which is the only thing that assigns a run's state, and by
    /// the two surfaces that render the figure.
    ///
    /// # Why a run needs a second clock at all
    ///
    /// [`AgentRun::started_unix_ms`] is stamped once, at enqueue, and never moves. Every
    /// duration cide showed for a run used to be `now - started`, which is *age since dispatch*
    /// and answers a different question from *how long did this agent work*. The two diverge
    /// without bound in three ways, all of them ordinary use: a run paused overnight accrued
    /// the night; a run that outlived a cide restart accrued the hours cide was shut; and a
    /// finished run's figure went on growing for ever, so a run that took ninety seconds and
    /// ended four hours ago read `4h 00m` on the row whose own doc said "how long it took".
    ///
    /// # Where the line is drawn, and why it is not drawn further out
    ///
    /// `Starting` and `Running` are the states in which a child of this run's own exists and is
    /// progressing. Everything else is waiting, and each kind of waiting was considered:
    ///
    /// * `Queued` — waiting on a concurrency slot. There is no child. A run that sat behind two
    ///   others for twenty minutes did no work in them, and counting the queue would make a
    ///   role's figures depend on how busy the project was rather than on what the role did.
    /// * `AwaitingPermission` — blocked on a human answering a prompt. The child is alive and
    ///   its turn is open, which is the argument for counting it, but the interval is bounded by
    ///   when somebody next looks at the screen: it is the same class as a pause and behaves
    ///   like one, in that a run left overnight at a permission prompt would accrue the night.
    /// * `Idle` — the turn was handed back. The run is holding its checkout waiting for the
    ///   queue to give it another turn, which is waiting, not working.
    /// * `Paused` — SIGSTOPped. The reported bug.
    /// * `Interrupted` — no process at all; cide restarted out from under it.
    /// * `Finished` / `Failed` — over. Answering `false` here is what makes a history row's
    ///   figure **final** with no end stamp anywhere in the workspace: the clock simply stops,
    ///   and the two bugs above it are fixed by the same absence rather than by three fixes.
    pub fn counts_as_work(&self) -> bool {
        match self {
            Self::Starting | Self::Running => true,
            Self::Queued
            | Self::Idle
            | Self::AwaitingPermission
            | Self::Paused { .. }
            | Self::Interrupted
            | Self::Finished { .. }
            | Self::Failed { .. } => false,
        }
    }
}

/// One dispatched execution, as the panel draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentRun {
    pub run: RunId,
    pub agent: AgentId,
    /// [`AgentDef::label`] **copied at dispatch, not looked up at render**.
    ///
    /// A history row must still read correctly after the role has been renamed or its definition
    /// deleted, and a row that re-derives its label from the current roster does neither: it
    /// renames itself when a config file changes, or empties when a role is removed. A row that
    /// renames itself is a row that lies about what happened — the run really was dispatched
    /// under the old name, and that is the name in the transcript, in `/resume` and in the
    /// terminal title.
    pub agent_label: String,
    /// Copied for the same reason as [`Self::agent_label`]: the run happened on this harness
    /// whatever the definition says now.
    pub harness: Harness,
    pub project: ProjectId,
    /// The PTY session, once there is one.
    ///
    /// `None` while [`RunState::Queued`] — there is no child yet — which is exactly why the Open
    /// action is *withheld* until there is, rather than drawn disabled. A queued row renders no
    /// Open control at all: a control that cannot work in this state should not be on the screen
    /// in it.
    ///
    /// It is also the join key for the phase dot. The Agents store subscribes to
    /// `cide://session-state` keyed on this, so a row's colour moves at session speed with no
    /// round trip and no second per-run event stream.
    pub session: Option<SessionId>,
    pub state: RunState,
    /// The task this run was dispatched against.
    ///
    /// **This is the whole agent→task half of the cross-link, and it is the only stored half.**
    /// The reverse — "which agent is on task t-17 right now" — is derived at render time by
    /// scanning live runs for this field; see [`crate::Task::agent`], which is the role a task is
    /// *for* and not a claim about the present.
    ///
    /// `None` is legal: an ad-hoc run — the orchestrator asking a role to check or do one small
    /// thing that is not on the board. Such a run stands in the **project root**, not in a
    /// worktree of its own (M40; `cide_agents::run_checkout` is the rule). The row draws
    /// `no task` in dim rather than nothing, because a run nothing can account for is worth
    /// seeing.
    pub task: Option<TaskId>,
    pub started_unix_ms: u64,
    /// Milliseconds this run has **worked**, over its closed working intervals only.
    ///
    /// Read together with [`Self::working_since_unix_ms`]: the figure to show is this plus, when
    /// that stamp is set, the time since it. Splitting a closed total from an open stamp is what
    /// lets a live row tick between broadcasts without the producer sending a new number every
    /// second, and what makes a paused or finished row's figure *stop* rather than freeze at
    /// whatever the last event carried.
    ///
    /// [`RunState::counts_as_work`] is the definition of "working", and its doc carries the
    /// argument for where the line is drawn.
    ///
    /// **This does not replace [`Self::started_unix_ms`]**, which stays exactly what it was: the
    /// moment the run was dispatched. That is a real fact, it is what the panel's History sorts
    /// by, and it is what `cide_agents::tools`' run list means by `started … ago` — which was
    /// never wrong, because it says "started". What was wrong was rendering it as the run's
    /// duration.
    pub worked_ms: u64,
    /// When the current working interval began, or `None` when this run is not working.
    ///
    /// `Some` in exactly the states [`RunState::counts_as_work`] admits, which is the invariant
    /// `cide_app::agents::move_to` exists to keep — it is the only thing in the process that
    /// assigns a run's state, and it opens and closes this stamp on the transition.
    ///
    /// Deliberately **not** persisted across a cide restart. Every restored run comes back
    /// `Interrupted`, which is not working, so this is `None` by construction and the hours cide
    /// spent shut add nothing to the figure — the offline case needs no snapshot timestamp and
    /// no code of its own.
    pub working_since_unix_ms: Option<u64>,
    /// Where cide announces this run's turn endings — the nudge `agent_rpc::note_run_over` types
    /// into a Claude pane. Recorded at dispatch and carried on the wire so the run list can say
    /// which runs will never announce themselves. (M40)
    pub notify: RunNotify,
    /// The run was frozen long enough that its in-flight model request may have timed out.
    ///
    /// **A field on the run, not an event**, and that is the load-bearing part. A window opened
    /// *after* the resume has no history to derive this from, so an event-only design shows
    /// nothing in the new window where the old one shows a warning — the exact failure
    /// `cide://session-awaiting`'s doc records and answers by broadcasting the whole set. State
    /// that a late-joining window must agree about belongs in the snapshot.
    ///
    /// A *suspicion*, surfaced as an offer, never an automatic re-dispatch: cide cannot see the
    /// model request, only a process that was stopped and continued, and a re-dispatch would
    /// double-bill a turn that in fact survived. The user answers Retry or Leave it, and
    /// `agents_ack_stale_turn` clears it.
    pub stale_turn: bool,
    /// One line of extra context for the row — what the queue is waiting on, which worktree this
    /// run holds. `None` for the ordinary case, which is most of them.
    pub note: Option<String>,
    /// Whether **Open** has anything to show for this run. (M42)
    ///
    /// The gate the panel reads, and it is a field rather than a rule the panel derives from
    /// [`Self::session`] because `session` stopped being the whole answer: a finished `opencode`
    /// run's child is gone and its cide session with it, yet its conversation (`ses_…`) can be
    /// re-opened in the real harness, and a finished `claude` run restored after a restart has
    /// no session at all while its transcript sits on disk. The registry computes this from what
    /// it holds in memory — a session it still owns, or a conversation it has confirmed can be
    /// re-opened — so a roster broadcast costs no filesystem read. `false` for a queued run,
    /// which is what keeps Open *withheld* there rather than drawn disabled.
    ///
    /// What Open then does is a second question with three answers — see [`RunOpen`].
    pub openable: bool,
}

/// What pressing **Open** on a run should do, as `agents_run_open` answers it. (M42)
///
/// One question to Rust, because the three answers depend on facts only Rust has — whether the
/// registry still holds a live child for the run, and whether the harness can re-open the
/// conversation from its directory — and the frontend then builds the pane through the split
/// machinery with the intent this names. **Never a pane built by the command**: the argument
/// beside `agent_open_pane` in `cide_app::cmd::agents` is why, and it is unchanged by this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum RunOpen {
    /// The run's child is alive: attach a second sink to its session, spawn nothing. For a
    /// `claude` run that is the real TUI, mid-turn; for `opencode` it is the rendered event
    /// stream of a turn in flight. `continues` is handed to the pane for later — see
    /// [`crate::SplitIntent::Mirror`].
    Mirror {
        session: SessionId,
        continues: Option<crate::HarnessSession>,
    },
    /// The child is gone and the harness can re-open the conversation: spawn it, in the pane,
    /// which owns it. See [`crate::SplitIntent::Continue`].
    Continue { conversation: crate::HarnessSession },
    /// Nothing to show, and the sentence saying why: a queued run has no child yet, an
    /// `opencode` run that died before naming its conversation has nothing to continue, a
    /// `claude` transcript can be gone with its worktree, or the `--resume` injection is off.
    Unavailable { reason: String },
}

/// What cide knows about a project's subagents right now.
///
/// A tagged union with a first-class `Disabled`, on [`crate::DiagnosticsSnapshot`]'s argument:
/// an empty `runs` array cannot say whether orchestration is **off**, **on with no roles
/// defined**, or **on with roles and nothing running**, and those want three different screens.
/// The first of them is not an edge case either — **subagents are off by default, so the state
/// most users see first is precisely the one an array cannot express**, and it is the state that
/// most needs prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum AgentRoster {
    /// Orchestration is off for this project — usually because nothing has ever turned it on.
    ///
    /// `hint` is the explanation the panel prints and `config_path` is named **in full and
    /// before the button**, because turning this on writes a file the user's repository will
    /// contain, and a feature toggle that quietly adds a committed file is a surprise commit.
    Disabled {
        hint: String,
        #[ts(type = "string")]
        config_path: PathBuf,
    },
    /// On, and no role is defined. The panel says where roles come from and offers no
    /// "create three roles for me" button: inventing a `qa` the user did not ask for is the
    /// same class of lie as a confident empty list.
    Empty {
        #[ts(type = "string")]
        config_path: PathBuf,
    },
    /// On, with roles. `runs: []` is a real zero — nothing is running.
    Ready {
        agents: Vec<AgentDef>,
        /// Live runs first, then finished ones, in the order the panel's five groups read
        /// (Running, Idle, Queued, Roles, Recent).
        runs: Vec<AgentRun>,
        /// Whether the queue will start anything new.
        ///
        /// **Distinct from "every run is paused", and the difference is what the user is about
        /// to act on.** Pausing does two things — it closes the dispatch queue *and* it freezes
        /// the children — and a project with the queue shut but nothing running looks identical
        /// to an idle one unless this is on the wire. A user who reaches for Resume needs to
        /// know which of the two they are looking at, and the panel header is the only control
        /// that is reachable when the console pane itself is frozen.
        dispatching: bool,
    },
}

/// The `agents` block of `.cide/config.json`.
///
/// # Why this is not part of [`crate::Settings`]
///
/// `Settings` is **global**: it lives at `Workspace.settings`, it is written to
/// `$XDG_STATE_HOME/cide/workspace.json`, and it rides every `cide://workspace-changed` to every
/// window of every open project. None of that is true of this.
///
/// "This project runs subagents, under these roles, at this concurrency" is a property of the
/// **checkout**. It belongs beside `.cide/tasks.json` and `.cide/agents/*.md`; it is reviewable
/// in a pull request, where a teammate can see that a repository has started dispatching agents;
/// and above all it must not follow the user into an unrelated project. A global switch would
/// mean enabling subagents once, for a repository the user trusts, silently arms every other
/// checkout they open afterwards.
///
/// `Project` has no settings field, and this change does not add one — the config is read fresh
/// from disk on demand rather than mirrored, for the reason the module header gives.
/// Correspondingly this config gets **no row** on the Settings screen: `SECTIONS` is global
/// settings and `useSettings.patch` writes a [`crate::SettingsPatch`], so a row there would have
/// to explain that it means something different from every other row on the screen.
///
/// [`crate::SettingsSection::Agents`] is not that row and does not contradict this, which is
/// worth stating because the two read like opposites. That section is not a page of settings; it
/// is an **editor for files** — `.cide/agents/*.md` and their global twins — reached through the
/// Settings shell because that is where a person looks for *configure the thing*, and writing
/// through `agents_save`/`agents_delete` rather than through a patch. It is where the switches
/// in *this* struct will eventually be drawn too, and when they are they will still be a
/// per-project file write and still not a [`crate::SettingsPatch`]. The rule the paragraph above
/// is really stating survives intact: nothing about a project rides `Workspace.settings`.
///
/// # Why this is `Clone` and no longer `Copy` (M79)
///
/// It was `Copy` until [`Self::auto_spin_prompt`] arrived, and a prompt is a `String` a person
/// wrote. The alternative — keeping `Copy` by leaving the prompt off the wire — would have put
/// the one field on this screen that a user actually composes behind a hand edit of a JSON file,
/// which is the state `AgentDraft` exists to get *out* of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct OrchestrationConfig {
    /// Off in the absence of the file, so a project that has never heard of this feature can
    /// never spawn anything. The single most important default in the module.
    pub enabled: bool,
    /// How many runs may be live across the whole project, whatever the per-role ceilings say.
    ///
    /// 2 rather than "the number of roles": every run is a full `claude` with its own context
    /// window and its own bill, and a default that started six of them the first time somebody
    /// pressed the button would be a default nobody forgives.
    pub max_concurrent: u16,
    /// The harness a role gets when its definition does not name one.
    pub harness: Harness,
    /// A finished run's note opens a fresh Claude tab instead of typing into the console. (M79)
    ///
    /// `cide_agents::config::AgentsConfig::finish_in_new_tab` carries the argument; this is the
    /// same key, on the wire because the user asked for it in Settings. Note it is **under**
    /// `nudgeOrchestrator`, which stays disk-only: that key answers *announce my subagents at
    /// all*, this one answers *where*.
    pub finish_in_new_tab: bool,
    /// cide wakes this project when it goes quiet with work still open. (M79)
    pub auto_spin: bool,
    /// How long that quiet must last, in seconds. The **stored** number, not the clamped one —
    /// `AgentsConfig::to_wire` says why.
    pub auto_spin_after_secs: u32,
    /// What the spun run is told. Empty means *use the one cide ships*, decided in
    /// `AgentsConfig::spin_prompt` and never here.
    pub auto_spin_prompt: String,
    /// cide reads the spun run's plan approval off its screen and answers it. (M79)
    ///
    /// `cide_agents::config::AgentsConfig::auto_spin_accept_plan` carries the argument, including
    /// why this is not the blind write the stop code refuses.
    pub auto_spin_accept_plan: bool,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        // Mirrors `cide_agents::config::AgentsConfig::default`, which is the real one: this is
        // what a build with no such handler draws, and the two disagreeing would show a panel
        // that describes a file it is not reading.
        Self {
            enabled: false,
            max_concurrent: 2,
            harness: Harness::Claude,
            finish_in_new_tab: true,
            auto_spin: false,
            auto_spin_after_secs: 900,
            auto_spin_prompt: String::new(),
            auto_spin_accept_plan: true,
        }
    }
}

/// A partial update to [`OrchestrationConfig`]. `None` means "leave this alone".
///
/// Mirrors [`crate::SettingsPatch`]'s shape deliberately — per-field `Option` with
/// `#[ts(optional)]`, so a caller flipping one switch need not restate the other two — and it is
/// allowed to, because none of these three fields is itself nullable. That is the exact test
/// [`crate::TaskEdit`] fails and explains at length.
///
/// `Clone` rather than `Copy` since M79, for [`OrchestrationConfig`]'s reason.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
#[ts(export)]
pub struct OrchestrationPatch {
    /// Turning this on writes `.cide/config.json`, and `agents_config_set` refuses it with a
    /// sentence when the project is not a git repository — worktree isolation has nothing to
    /// build on there, and a silent fallback to a shared tree is how two agents clobber one file
    /// with nobody told.
    #[ts(optional)]
    pub enabled: Option<bool>,
    #[ts(optional)]
    pub max_concurrent: Option<u16>,
    #[ts(optional)]
    pub harness: Option<Harness>,
    #[ts(optional)]
    pub finish_in_new_tab: Option<bool>,
    #[ts(optional)]
    pub auto_spin: Option<bool>,
    /// Clamped by `AgentsConfig::apply` rather than refused, `max_concurrent`'s trade.
    #[ts(optional)]
    pub auto_spin_after_secs: Option<u32>,
    /// Flattened to one line by `AgentsConfig::apply` before it reaches the file: this string is
    /// typed into a terminal, where a newline is another Enter.
    #[ts(optional)]
    pub auto_spin_prompt: Option<String>,
    #[ts(optional)]
    pub auto_spin_accept_plan: Option<bool>,
}

/// Dispatch one run. Inbound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct DispatchRequest {
    pub project: ProjectId,
    pub agent: AgentId,
    /// The task this run is for.
    ///
    /// `None` is legal and means an ad-hoc run — the orchestrator asking a role to check or do
    /// one small thing that is not on the board. Such a run **stands in the project root**: no
    /// worktree, no `cide/<role>-…` branch, nothing to integrate (M40 — before that a task-less
    /// run took the role's base worktree, and the user asked for exactly not that: a quick check
    /// or a small piece of direct work has nowhere to be merged back *from*). The panel's Dispatch
    /// button nonetheless **always** fills it, because a run with no task is a run the Tasks
    /// panel cannot account for: it draws no chip on any row, it leaves no trace once it has
    /// exited, and the record of what the agents did that afternoon has a hole in it. The MCP
    /// road (`cide_agent_dispatch`) may omit it, and its own prose says when that is right.
    #[ts(optional)]
    pub task: Option<TaskId>,
    /// Extra instruction for this run only, appended after the task body — or, for a run with no
    /// task, the whole of its brief.
    ///
    /// `None` means the task speaks for itself, which is the case the orchestrator should be
    /// aiming for: instruction that lives only in a prompt is instruction the next run of the
    /// same task never sees.
    #[ts(optional)]
    pub prompt: Option<String>,
    /// Where the run's turn endings are announced. (M40)
    ///
    /// `None` reads as [`RunNotify::Primary`], which is what a request that carries no session
    /// of its own — the panel's button, an assignment made in the Tasks panel — can honestly
    /// ask for. A request made over the agent socket carries the connection's session instead
    /// (`agent_rpc::RegistrySink` fills this from the `Scope`), so the pane that asked is the
    /// pane that hears.
    #[serde(default)]
    #[ts(optional)]
    pub notify: Option<RunNotify>,
}

/// Where cide announces a run's turn endings — handed back, finished, failed. (M40)
///
/// # Why this is recorded on the run and not looked up at the edge
///
/// The nudge is a line typed into somebody's conversation (`agent_rpc::note_run_over`'s header
/// says why it is a PTY write at all), and *whose* conversation used to be a fixed answer: the
/// project's primary pane. That was wrong twice over once a second Claude pane could start a run —
/// by assigning a role, and since M40 by dispatching — because the pane that asked never heard,
/// and the pane that did hear had not asked. The choice belongs to the moment of dispatch, so it
/// is stamped there, rides the run through the snapshot, and is read at the edge.
///
/// # Why `Primary` is resolved late
///
/// `Project::primary_session` moves whenever the console pane binds a new conversation
/// (`workspace::bind_session` rewrites it on every restart), so a *session id* captured at
/// dispatch would name a conversation that no longer exists by the time a long run ends.
/// `Primary` is therefore a role, resolved when the line is typed; [`Self::Session`] is the
/// specific pane that asked, and falls back to `Primary` if that pane is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum RunNotify {
    /// The project's primary Claude pane, whichever conversation it holds when the run ends.
    #[default]
    Primary,
    /// One particular Claude pane of the project — the one whose connection dispatched or
    /// assigned. If that session is gone or no longer a Claude pane of the project at delivery,
    /// the primary pane hears instead: the fact is not dropped for want of its first address.
    Session { session: SessionId },
    /// Nothing is typed anywhere. The orchestrator that chose this polls `cide_agent_runs`
    /// (`includeFinished: true`) and reads the task's comments; the death comment a failed run
    /// leaves on its task is written regardless, because that is board record, not a knock.
    Silent,
}

// ==========================================================================================
// Editing a role from a form, rather than by hand. (M18)
// ==========================================================================================
//
// Everything above this line reads `.cide/agents/*.md`. Everything below it is the write path,
// and it exists because the answer to "how do I add a role" was *open an editor and get the
// front matter right*, which is a fair thing to ask of the person who designed the format and
// nobody else.
//
// The shape of it follows from one fact: **the file stays the source of truth.** The form is a
// second door onto a committed markdown file that a teammate, a `git pull` and the user's own
// editor also write. So a draft is a projection of one file, a save re-renders that whole file,
// and the roster everything else reads is still derived from disk afterwards. Nothing here is
// cached, mirrored into `Workspace`, or authoritative.

/// Which of the two directories a definition lives in.
///
/// The four directories `cide_agents::defs::load_from` merges, named on the wire because **a
/// form has to say which file it is about**. The panel draws one `developer` row where two files may declare it
/// — a project definition shadowing a global one, which `LoadedAgent::shadows` exists to make
/// visible — so a save that guessed would edit whichever the guess landed on, and the user would
/// watch their change have no effect for the same reason shadowing costs an afternoon today.
///
/// It is also the only control the user has over that merge. Moving a role from `Project` to
/// `Global` is how it stops being this repository's and becomes theirs, on every project they
/// open; there is no key in the file for that, because it *is* which directory the file is in.
/// So scope is a field of the draft and changing it is a move — see `cide_agents::defs::save`.
///
/// # Two families, four directories
///
/// The first two are cide's own. The second two are **Claude Code's** (M30): a subagent is the
/// same idea in somebody else's format, documented at <https://code.claude.com/docs/en/sub-agents>,
/// and the directories are already populated on the machines of people who have never opened
/// cide. Reading them is not a translation layer bolted on — they merge into the one catalog
/// through the one merge, because a role's identity is its name and two lists keyed by name
/// would be two answers to "who is `reviewer`".
///
/// What the families do *not* share is who owns the vocabulary. cide defines the keys in
/// `.cide/agents/` and may refuse one it does not know; it defines none of the keys in
/// `.claude/agents/` and must therefore preserve every one of them — see
/// [`AgentDraft::extras`], which exists for exactly that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AgentScope {
    /// `<root>/.cide/agents/<name>.md` — committed with the project, reviewed in its pull
    /// requests, the same for everyone who checks it out.
    Project,
    /// `$XDG_CONFIG_HOME/cide/agents/<name>.md` — beside `keymap.json`, the user's own, and
    /// applied to every project they open unless that project defines the same name.
    Global,
    /// `<root>/.claude/agents/*.md` — a **Claude Code subagent** committed with the project.
    ///
    /// The file stem need not equal the `name:` key here, which is Claude's rule and not cide's,
    /// so this scope is addressed by declared name and the file behind it is found by reading
    /// the directory. See `cide_agents::defs::claude_project_dir`.
    ClaudeProject,
    /// `~/.claude/agents/*.md` — the user's own Claude Code subagents, on every project.
    ///
    /// `CLAUDE_CONFIG_DIR` relocates it; `cide_claude::claude_dir` is the one place that rule
    /// lives.
    ClaudeGlobal,
}

impl AgentScope {
    /// Whether this scope is a directory **Claude Code** owns rather than one cide does.
    ///
    /// The question three separate decisions turn on, which is why it is a method and not three
    /// `matches!` written out: the roster's badge, the harness's `--agent` road, and whether an
    /// unknown front-matter key is a defect in cide's format or a key in somebody else's.
    pub fn is_claude_code(self) -> bool {
        matches!(self, Self::ClaudeProject | Self::ClaudeGlobal)
    }
}

/// Which definition file a role occupies: a scope and a name, which is a directory and a stem.
///
/// A pair rather than a path, because a path is not the frontend's to construct: `global_dir()`
/// is an XDG lookup and `.cide/agents/` hangs off a project root the webview does not hold. It
/// is also the pair the *user* changed — a form that came back with `/home/x/.config/...` would
/// have to be parsed to find out which of the two things moved.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AgentLocation {
    pub scope: AgentScope,
    pub name: AgentId,
}

/// One front-matter key cide does not model, carried through a save untouched. (M30)
///
/// # The whole point is that cide does not understand it
///
/// A Claude Code subagent's front matter is **Claude's vocabulary**: `hooks`, `mcpServers`,
/// `skills`, `maxTurns`, `disallowedTools`, `color`, `background`, `initialPrompt`, `memory`, and
/// whatever a release adds next month. cide has no model for any of them, will never have a model
/// for all of them, and is nonetheless being asked to write the file back after a user edits the
/// description. Re-rendering only the keys cide knows would **delete somebody's `hooks:` block**,
/// silently, in a committed file, as a side effect of fixing a typo.
///
/// So an unmodelled key is neither dropped nor guessed at: it is kept, in the order the file had
/// it, and written back verbatim. [`Self::value`] may therefore be **multi-line** — a nested
/// block or a block sequence is carried as the raw source lines it occupied, because the one
/// thing cide can honestly promise about YAML it does not parse is that it did not touch it.
///
/// The form renders these as editable key/value rows, which is the other half of the decision:
/// preserving a key the user cannot see would make the form lie about what the file contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AgentExtra {
    /// The key, without its colon.
    pub key: String,
    /// Everything after the colon, and — when the key opened a nested block or a block sequence —
    /// the lines under it too, joined with `\n` and with their original indentation intact.
    pub value: String,
}

/// One role as a form edits it: every key the definition file may carry, plus where it lives.
///
/// Inbound, and therefore `deny_unknown_fields` — a field the form invented that this build
/// silently dropped would be a switch the user set and the file never got, which is precisely
/// the class of failure `cide_agents::defs`' parser refuses rather than ignores.
///
/// # Why this is not [`AgentDef`] with a few more fields
///
/// [`AgentDef`] is the *roster row*: what the panel draws, for every role, on every
/// `cide://agents-changed`. It carries `unavailable`, which no file contains and no form may
/// set, and it is missing `tools`, `permission-mode` and `effort` for the reason its own doc
/// gives — a row cannot act on them, and a panel handed them would have to be trusted not to
/// draw them as editable. This is the other projection of the same file: everything a *file*
/// can say, nothing a *reader* computed. Merging the two would put a settable `unavailable` on
/// the wire and a whole definition's worth of bytes on a per-run event stream.
///
/// # What a save through this loses, and why that is the design
///
/// A save re-renders the whole file from these fields. What that costs, and what it no longer
/// costs, changed in M30 and the distinction is worth stating precisely.
///
/// **Still lost:** comments in the front matter, and a value cide's own vocabulary cannot hold —
/// `harness: bogus`, `max-concurrent: lots` — which comes back as "not set" and is written out as
/// absent. Both are lossy on purpose. A value cide could not read is one it cannot vouch for, and
/// re-emitting it would be the form claiming a switch is set when nothing will act on it.
///
/// **No longer lost: an unknown key.** It used to go, on the argument that "a save that
/// re-emitted an unread key would be cide vouching for a line it does not understand". That
/// argument holds for a directory cide *owns* and collapses for one it does not. Claude Code's
/// `.claude/agents/` is now a scope ([`AgentScope::ClaudeProject`]), its whole vocabulary is
/// outside `cide_agents::defs::KNOWN_KEYS`, and deleting a user's `hooks:` block because they
/// edited a description is not a defensible thing to do to somebody else's file. So an unmodelled
/// key rides [`Self::extras`], is shown in the form as an editable row — which is what keeps it
/// from being text the user cannot see — and is written back verbatim.
///
/// The reporting did not move with it. A key outside `KNOWN_KEYS` in a `.cide/agents/` file is
/// still a problem against that file's line, exactly as before; it is now *reported and kept*
/// rather than reported and deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AgentDraft {
    /// Which directory this role will live in after the save.
    pub scope: AgentScope,
    /// The role's name, which is also its file stem — the parser enforces that they agree.
    ///
    /// So editing this field is a **rename of a file**, not an edit inside one. See
    /// [`Self::original`].
    pub name: AgentId,
    /// Which file the form was populated from, or `None` for a role being created.
    ///
    /// **Not a key in the file and never written to one.** It is the form's memory of what it
    /// opened, and it is on the draft rather than a second argument to the save because it is
    /// the answer to a question only the form knows: the user has typed a new name into a box,
    /// and cide cannot tell from the draft alone whether that is a rename of `qa.md` or a brand
    /// new role that happens to be called `qa`. Those two have different failure modes — one
    /// must move a file, the other must refuse to overwrite one — and guessing between them
    /// either loses a definition or refuses a legitimate rename.
    ///
    /// `None` therefore means *create*, and a create over an existing file is refused rather
    /// than merged: the file it would replace is somebody's system prompt.
    #[ts(optional)]
    pub original: Option<AgentLocation>,
    /// The `label:` key. `None` means the file does not set one and the roster title-cases the
    /// name, which is what [`AgentDef::label`] promises.
    #[ts(optional)]
    pub label: Option<String>,
    /// The `harness:` key. `None` means the file is silent and the project's
    /// [`OrchestrationConfig::harness`] applies — a real and common state, and distinct from
    /// naming the same harness explicitly, because the project default can change underneath a
    /// role that never named one.
    #[ts(optional)]
    pub harness: Option<Harness>,
    /// The `description:` key, which is prompt text as much as UI text — see
    /// [`AgentDef::description`]. Empty is legal and warned about, never refused.
    pub description: String,
    #[ts(optional)]
    pub model: Option<String>,
    /// A harness-specific reasoning-effort knob, carried verbatim and validated against nothing:
    /// the set differs per harness and per release, exactly as `cide_agents::defs::KNOWN_TOOLS`
    /// does, and a list cide checked against would be wrong within a month.
    #[ts(optional)]
    pub effort: Option<String>,
    /// `--allowedTools`, as a list. Empty means the definition does not restrict them, which is
    /// the harness's own default and **not** "no tools".
    pub tools: Vec<String>,
    /// `--permission-mode`. A string and not an enum, because it is somebody else's vocabulary
    /// and the CLI updates itself underneath a running cide — see
    /// `cide_agents::defs::PERMISSION_MODES`, which is checked against and is allowed to go
    /// stale. A value outside it is refused with the list in the sentence, on this field.
    #[ts(optional)]
    pub permission_mode: Option<String>,
    /// `max-concurrent:`. `None` means the file does not say, which the loader reads as 1.
    ///
    /// `Option<u16>` where [`AgentDef::max_concurrent`] is a bare `u16`, and the difference is
    /// the point: the roster carries the *effective* number, and a form carries what the file
    /// says. Writing `max-concurrent: 1` into every definition the form has ever touched would
    /// freeze a default that a later cide may want to change.
    #[ts(optional)]
    pub max_concurrent: Option<u16>,
    /// `worktree:`. `None` means the file does not say, which the loader reads as `true`.
    ///
    /// An `Option` for `max_concurrent`'s exact reason one field up: the form carries what the
    /// file says, and `Some` is written back only when the file (or the user) actually said
    /// it. This field existing on the draft at all is load-bearing — `defs::render` writes
    /// only the fields the draft models, so a switch the draft did not carry would be
    /// silently *deleted* from a hand-written definition by the next panel save.
    #[ts(optional)]
    pub worktree: Option<bool>,
    /// The body of the file: the role's system prompt, verbatim.
    ///
    /// The one field here that is a document rather than a switch, and the reason the format is
    /// markdown at all — `cide_agents::defs`' module header makes that argument in full. It is
    /// **editable here** where [`AgentDef::system_prompt`] is explicitly read-only in the roster
    /// panel: that field's doc refuses an editor *in a row*, on the grounds that a second writer
    /// racing the user's editor is a bad idea in a surface nobody opened for editing. A form the
    /// user deliberately opened to change this file is the surface that was missing.
    pub system_prompt: String,
    /// Every front-matter key cide does not model, in the order the file had them. (M30)
    ///
    /// See [`AgentExtra`] for why they are carried at all. Empty is the ordinary state of a
    /// `.cide/agents/` definition — cide defines that format, so a key outside
    /// `cide_agents::defs::KNOWN_KEYS` there is a *defect* and is reported as one against the
    /// file's line. It is still preserved, which is the change M30 made to the paragraph above:
    /// reporting a key and deleting it are different services, and cide now performs only the
    /// first.
    #[serde(default)]
    pub extras: Vec<AgentExtra>,
}

/// Which box on the form a refusal belongs to.
///
/// The whole reason [`AgentDraftProblem`] is not a string. A save that answers "invalid" leaves
/// the user to find which of eleven fields it meant, and the fields that actually get refused —
/// a name that cannot be a directory component, a permission mode from a different CLI's
/// vocabulary — are exactly the ones whose rule is not visible from the box.
///
/// Variants are the draft's field names, so a frontend can key an error map by them without a
/// translation table, and adding a field to [`AgentDraft`] without a variant here is a field
/// whose refusals have nowhere to land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AgentField {
    Scope,
    Name,
    Label,
    Harness,
    Description,
    Model,
    Effort,
    Tools,
    PermissionMode,
    MaxConcurrent,
    SystemPrompt,
    /// The unmodelled front-matter keys, as a whole. (M30)
    ///
    /// One variant for the whole list rather than one per key, because the keys are not cide's to
    /// enumerate — that is the entire premise of [`AgentExtra`]. What lands here is a refusal
    /// about the list's *shape*: a key that is not a key, a duplicate, a value that would open a
    /// line inside the front matter.
    Extras,
}

/// One reason a draft was not written, against the field that caused it.
///
/// `message` is a whole sentence for the person looking at the box, in the voice
/// `cide_agents::defs`' parse errors use: what is wrong, why the rule exists when the reason is
/// not obvious, and what to write instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentDraftProblem {
    pub field: AgentField,
    pub message: String,
}

/// What `agents_save` did, or refused to do.
///
/// # Why a refusal is an `Ok` arm rather than an error
///
/// It is the shape `cmd::agents`' `AgentIntegration` already uses one command over, and for the
/// same reason: a *refusal* is a described outcome with a next action in it, while an `Err` is
/// the surface for things that went wrong. Concretely, `CoreError` has one string per variant
/// and no room for eleven fields, so a rejection routed through it would arrive as one sentence
/// with the field names inlined into prose — the "says invalid without saying where" failure
/// this type exists to make unrepresentable. Adding a variant to `CoreError` for it is the other
/// option and would give every existing consumer of that enum an arm for a subagent form.
///
/// The `Err` channel is still used, and for what it is for: a disk that would not take the
/// write, a project with no roots. Those are not the user's form to fix.
///
/// # Why `Saved` carries the whole roster
///
/// `cmd::tasks` answers with the whole board on the same argument, and `agents_config_set` with
/// the whole roster: there is no `cide://workspace-changed` carrying a per-project file, so a
/// caller handed an acknowledgement has exactly one way to learn what it did — ask again — and
/// the frame in between shows a list that does not contain the role the user just created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum AgentSaveOutcome {
    /// Written. `path` is the file that now holds it, named because a save may have *moved* the
    /// definition — a rename, or a change of scope — and the file the user edits next is not
    /// necessarily the one they opened.
    Saved {
        #[ts(type = "string")]
        path: PathBuf,
        roster: AgentRoster,
    },
    /// **Nothing was written**, and here is every reason, against its field.
    ///
    /// Every reason rather than the first: a form that surfaces one error at a time makes the
    /// user submit four times to discover four problems, and three of those submissions are
    /// round trips through a file write that was never going to happen.
    Rejected { problems: Vec<AgentDraftProblem> },
}

/// What the role form may offer in its **Model** box, for one harness. (M43)
///
/// # Why a list at all, when [`AgentDraft::model`] is validated against nothing
///
/// Because "validated against nothing" is a rule about what cide *refuses*, and it says nothing
/// about what cide can *suggest*. The two are the same asymmetry [`AgentDraft::effort`] states:
/// a menu, not a rule. This is the strong form of it — Claude Code has aliases anybody can type
/// (`sonnet`), while opencode wants `provider/model` drawn from whichever providers this machine
/// has configured, which on a real one includes ids like `unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M`.
/// A field nobody can fill from memory is a field nobody fills, and the effect was that
/// selecting opencode left every role on the provider default with no way to say otherwise.
///
/// So the frontend must never treat this as a closed set. `check-settings-agents.mjs` asserts
/// `model` is not a closed vocabulary and the box stays typable beside the menu; this type only
/// Whether one provider/model actually answered, run for real. (M45)
///
/// A verdict and not a `Result` on the wire, for `AgentModels::problem`'s reason: the frontend
/// draws both outcomes as a sentence beside the row, and a rejected `invoke` would have to be
/// unwrapped through `errorText` into the same sentence anyway — with the difference that a
/// rejection reads as *cide* failing rather than the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LlmModelTest {
    /// The `provider/model` that was tried, echoed so a late answer cannot be drawn against a
    /// row the user has since edited.
    pub model: String,
    /// Did it answer?
    pub ok: bool,
    /// One whole sentence, either way. The failure half is the provider's own words where there
    /// were any — see `cide_agents::harness::opencode`'s `error_sentence`.
    pub detail: String,
}

/// makes the common case one click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentModels {
    /// Which harness this list describes, echoed back.
    ///
    /// Echoed for the reason `cmd::settings::ClaudeCliSupport::binary` is: the answer describes
    /// *this* harness, and a form that captioned it with whatever its dropdown currently says
    /// would label one CLI's models with the other's name for as long as a probe was in flight —
    /// which is exactly the moment the user is switching between them.
    pub harness: Harness,
    /// The ids, in the order the harness names them. Empty is a legal answer and not an error.
    pub models: Vec<String>,
    /// Why there is no list, as a sentence — or `None`.
    ///
    /// A sentence about the *machine* ("opencode is not on this app's PATH"), never a refusal
    /// about the draft: it is drawn as the field's hint and never as an
    /// [`AgentDraftProblem`], because nothing the user typed caused it and nothing they can type
    /// in this form fixes it. The box stays editable either way.
    pub problem: Option<String>,
}

/// What a harness reported it spent, as of its last completed step. (M80)
///
/// # Normalised, because the two CLIs count differently
///
/// opencode's `step_finish` carries `tokens: {input, output, reasoning, cache:{read,write}}`
/// whose own `total` is `input + output + reasoning` — cache reads are counted *beside* the
/// input, not inside it. codex's `turn.completed` carries `usage: {input_tokens,
/// cached_input_tokens, output_tokens}` where `cached_input_tokens` is a **subset** of
/// `input_tokens`. Carried raw, the same conversation would read as a third larger under one
/// harness than the other, and nothing on the card would say why. So each harness's `usage`
/// answers this shape with the cached half taken *out* of [`Self::input`], and this struct's
/// arithmetic is the same arithmetic for both.
///
/// It is the **last step's** figures and never a running sum: that is what makes
/// [`Self::context`] mean what the card says it means, and it is also all either CLI reports —
/// neither states a conversation total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TokenUsage {
    /// The prompt the model read, less whatever of it came from the cache.
    pub input: u64,
    /// What it wrote.
    pub output: u64,
    /// What it thought, where the harness counts that apart from the output. opencode does;
    /// codex reports no reasoning figure at all, so this is `0` for every codex run rather than
    /// a number invented from the output.
    pub reasoning: u64,
    /// The part of the prompt served from the provider's cache. Still context — it is in the
    /// window — and still worth seeing apart, because it is the part that was cheap.
    pub cache_read: u64,
    /// What was written *into* the cache on this step. Not part of the window.
    pub cache_write: u64,
}

impl TokenUsage {
    /// Roughly what was in the model's context window when this step ended.
    ///
    /// The prompt it read (cached or not) plus what it produced, which is what the *next*
    /// request's prompt will contain. An estimate, and the card says so in as many words: a
    /// harness that compacts, summarises or drops a turn between steps makes the next prompt
    /// smaller than this, and neither CLI reports the number directly. Deliberately not
    /// including [`Self::cache_write`] — those bytes went to the provider's cache, not into
    /// this conversation.
    #[must_use]
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.output + self.reasoning
    }
}

/// Which harness and model are behind a log line, for the card that opens one. (M80)
///
/// # Why the card asks at all
///
/// A run's pane draws `● bash  cargo test  1.2s #7` and nothing about *who ran it*. On one
/// machine a role may be an `opencode` pool that failed over twice, a `codex` the project's
/// overrides redirected it to, or the `claude` its file names — and the three are read back
/// from the same pane, in the same colours, days later. The two facts that answer "why did it
/// do that" are the harness and the model, and until M80 neither was anywhere on screen once
/// the run had left the Agents panel's live rows.
///
/// `None` on a [`crate::LogLineDetail`] is the ordinary answer, not a failure: a shell pane's
/// structured log lines are the feature this card was built for and they belong to no run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogRunInfo {
    /// The harness the standing child was **forked with**, which is not always the one the
    /// role's file names — an override redirects it, and M78's whole lesson was that the two
    /// answers drift silently. `AgentRegistry` reads it off the run, never off the definition.
    pub harness: Harness,
    /// The model that child was given, spelled as the harness was told it: a pool candidate's
    /// `provider/model`, the override's or the role's single model, or opencode's own resolved
    /// default.
    ///
    /// `None` where cide named none and the CLI chose for itself — a `claude` run with no
    /// `model:` in its definition, which is the common case. Drawn as a sentence saying so,
    /// never as a blank: a blank beside a label reads as a broken dialog (`TaskDetailPending`'s
    /// rule).
    pub model: Option<String>,
    /// How far down its pool this run has fallen, as `2 of 3`, or `None` for a run with no
    /// pool. The model above is *this* candidate, so without the position a failover is
    /// invisible — the card would simply name a model nobody chose.
    pub pool_position: Option<String>,
    /// The last step's tokens, or `None` until the run has finished one.
    pub usage: Option<TokenUsage>,
    /// The model's context window in tokens, where **this machine's configuration** states one.
    ///
    /// Only ever a number a user typed: `LlmModel::context`, looked up in the provider document
    /// the child was forked with, by the pool candidate's provider **and** model id as two
    /// separate keys. A catalogued model's window is the provider's business and cide holds no
    /// table of them, and splitting a `provider/model` string to go looking would be inventing
    /// the parser [`crate::PoolEntry`]'s header refuses to own. So this is usually `None`, and
    /// the card then prints the token count with no percentage rather than a percentage of a
    /// guess.
    pub context_limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight are the only eight, and a value outside them derives rather than paints.
    ///
    /// The negative half is the specification, the way it is for the failover classifier and the
    /// JSON-log detector: this function's whole job is to be **unsure safely**, because the
    /// vocabulary it validates against belongs to Claude Code and will grow without asking.
    #[test]
    fn a_hue_is_one_of_the_eight_or_it_is_nothing() {
        assert_eq!(
            AGENT_HUES.len(),
            8,
            "eight, because the ninth colour is the orchestrator's"
        );
        assert!(
            !AGENT_HUES.contains(&"orchestrator"),
            "the reserved colour is not in the list a role indexes — that absence is the reservation"
        );
        for name in AGENT_HUES {
            assert_eq!(hue(name).as_deref(), Some(*name), "{name} names itself");
        }

        assert_eq!(
            hue("Cyan").as_deref(),
            Some("cyan"),
            "folded — a person types it"
        );
        assert_eq!(
            hue("  cyan  ").as_deref(),
            Some("cyan"),
            "and trimmed, for the same reason"
        );

        for outside in [
            // Claude Code's own UI offers this, and it is not a colour: it means derive one.
            "automatic",
            // A hex is a perfectly sensible thing to write and is still not one of the eight;
            // painting it would put an unvalidated colour on `--panel` with no contrast gate.
            "#ff0000",
            // The reserved colour, spelled by somebody who read the source. Refused here as well
            // as unreachable by hash, because a definition must not be able to claim it.
            "orchestrator",
            // A colour a future Claude release adds. A warning would be noise about a key cide
            // does not model; the name simply derives its hue, which is a perfectly good colour.
            "teal",
            "",
            "   ",
        ] {
            assert_eq!(hue(outside), None, "{outside:?} is not one of the eight");
        }
    }

    #[test]
    fn a_run_round_trips_under_its_wire_names() {
        let run = AgentRun {
            run: RunId::new(),
            agent: AgentId("developer".into()),
            agent_label: "Developer".into(),
            harness: Harness::Claude,
            project: ProjectId::new(),
            session: None,
            state: RunState::Paused {
                since_unix_ms: 1_700_000_000_000,
            },
            task: Some(TaskId("t-14".into())),
            started_unix_ms: 1_699_999_000_000,
            worked_ms: 125_000,
            // Paused, so no interval is open — the pair's invariant, asserted below.
            working_since_unix_ms: None,
            notify: RunNotify::Primary,
            stale_turn: true,
            note: None,
            openable: false,
        };
        let json = serde_json::to_string(&run).expect("serialize");
        // snake_case on both sides would round-trip happily and read `undefined` in the webview.
        for wire in [
            "agentLabel",
            "startedUnixMs",
            "workedMs",
            "workingSinceUnixMs",
            "staleTurn",
            "sinceUnixMs",
        ] {
            assert!(json.contains(wire), "missing {wire} in {json}");
        }
        assert_eq!(serde_json::from_str::<AgentRun>(&json).unwrap(), run);
    }

    #[test]
    fn only_a_run_with_a_child_of_its_own_is_working() {
        // The table in `counts_as_work`'s doc, as assertions. Every variant appears, so this
        // fails to compile-and-pass rather than silently omitting a new one — the match itself
        // is exhaustive, and this is what pins *which* answer each arm gives.
        for working in [RunState::Starting, RunState::Running] {
            assert!(working.counts_as_work(), "{working:?} is work");
        }
        for waiting in [
            RunState::Queued,
            RunState::Idle,
            RunState::AwaitingPermission,
            RunState::Paused {
                since_unix_ms: 1_700_000_000_000,
            },
            RunState::Interrupted,
            RunState::Finished { code: 0 },
            RunState::Failed {
                reason: "no worktree".into(),
            },
        ] {
            assert!(!waiting.counts_as_work(), "{waiting:?} is not work");
        }
    }

    #[test]
    fn a_finished_runs_figure_is_final_because_its_clock_is_shut() {
        // The history row's whole claim, and the reason this change needs no end timestamp:
        // a terminal state opens no interval, so `worked_ms` is the answer for ever and
        // `working_since_unix_ms` has nothing to add to it however far the clock moves.
        assert!(!RunState::Finished { code: 0 }.counts_as_work());
        assert!(
            !RunState::Failed {
                reason: "spawn failed".into()
            }
            .counts_as_work()
        );
    }

    #[test]
    fn run_state_is_tagged_like_session_state() {
        // `tag = "state"` matches `SessionState`'s, so the two read alike in the store that
        // joins them on `AgentRun::session`.
        let json = serde_json::to_string(&RunState::Finished { code: 2 }).unwrap();
        assert_eq!(json, r#"{"state":"finished","code":2}"#);
    }

    #[test]
    fn an_idle_run_carries_no_exit_code() {
        // The whole of the variant, in one assertion: a turn that ended writes **no `code`**,
        // because no child produced one. The state this replaced serialised as
        // `{"state":"finished","code":0}` — an exit status for a process that was still alive
        // and still resumable, and byte-identical to a clean exit.
        let json = serde_json::to_string(&RunState::Idle).unwrap();
        assert_eq!(json, r#"{"state":"idle"}"#);
        assert!(
            !json.contains("code"),
            "an idle run has no exit status: {json}"
        );
        assert_eq!(
            serde_json::from_str::<RunState>(&json).unwrap(),
            RunState::Idle
        );
        // And it is a different value from the one it replaced, which is the drift a frontend
        // restating this enum as a string union has to be pinned against.
        assert_ne!(RunState::Idle, RunState::Finished { code: 0 });
    }

    #[test]
    fn the_roster_names_off_and_idle_apart() {
        let disabled = AgentRoster::Disabled {
            hint: "Subagents are off for this project.".into(),
            config_path: PathBuf::from("/repo/.cide/config.json"),
        };
        let idle = AgentRoster::Ready {
            agents: vec![],
            runs: vec![],
            dispatching: true,
        };
        // The point of the union: these are two different screens, and an empty array is both.
        assert_ne!(
            serde_json::to_string(&disabled).unwrap(),
            serde_json::to_string(&idle).unwrap()
        );
        assert!(
            serde_json::to_string(&disabled)
                .unwrap()
                .contains("configPath")
        );
    }

    #[test]
    fn orchestration_is_off_by_default() {
        // The default that keeps an upgrade from spawning anything in somebody's repository.
        let cfg = OrchestrationConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.max_concurrent, 2);
        assert_eq!(cfg.harness, Harness::Claude);
    }

    #[test]
    fn an_inbound_patch_refuses_a_field_it_does_not_know() {
        assert!(serde_json::from_str::<OrchestrationPatch>(r#"{"enable":true}"#).is_err());
        assert_eq!(
            serde_json::from_str::<OrchestrationPatch>(r#"{"enabled":true}"#).unwrap(),
            OrchestrationPatch {
                enabled: Some(true),
                ..Default::default()
            }
        );
    }
}
