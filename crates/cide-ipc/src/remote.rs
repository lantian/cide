//! What a paired device is shown: projections of the workspace, never the workspace. (M72)
//!
//! > *"Need to develop an mobile app (android/ios) that will allowe me to connect to cide and
//! > see projects of opened cide and select the project and see it's activly running claude
//! > (and agents) sessions, agents and tasks. Need to show what is in progress right now and to
//! > be able to open session on phone."*
//!
//! # The rule this module exists to make unbreakable
//!
//! **[`crate::Workspace`] must never cross this wire, and the reason is not its size.** It
//! carries [`crate::Settings`], which carries [`crate::LlmSettings`] — provider **API keys, in
//! plaintext** (the whole reason [`crate::LlmProvider`] has a hand-written `Debug`) — and
//! [`crate::ProxySettings`], whose URLs carry passwords (the reason `workspace.json` is written
//! `0600`, asserted by `persist`'s own test). Forwarding the payload of
//! `cide://workspace-changed` to a network client would put every credential on the machine onto
//! the LAN, in a feature whose entire premise is that the listener is reachable from another
//! device.
//!
//! So the remote surface carries *projections*: [`RemoteProject`] and [`RemoteSession`] below,
//! built by `cide-app` from the tree, and nothing that has ever held a secret. The tree's own
//! revision is teed as a bare number so a client knows to re-ask, and re-asking gets it a
//! projection again. The protection is structural rather than careful — the types in this module
//! cannot name `Workspace`, so the mistake is not expressible — and `cide-app`'s
//! `a_projection_carries_no_credential` plants a key in a fixture and greps every frame the host
//! can produce, because "cannot name it" stops being true the first time somebody adds a field.
//!
//! # Why a session is reported per *pane*
//!
//! Two panes mirroring one child is a real gesture in cide (`claude.mirror`), so the same
//! `SessionId` legitimately appears twice with two titles. A list keyed by session would have to
//! pick one of them, and picking is guessing. The consumer that wants conversations rather than
//! panes de-duplicates on [`RemoteSession::session`], which is the only place that knows which
//! of the two it is counting.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agents::{AgentScope, DispatchRequest, Harness};
use crate::ids::{AgentId, PaneId, ProjectId, RunId, SessionId, TabId};
use crate::screen::{Cursor, ScreenInfo, ScreenLine, ScrollbackCapture};
use crate::tasks::{TaskEdit, TaskNew};
use crate::{PaneKind, PaneRole, SessionState, TaskId};

/// Which cide a device is talking to.
///
/// Sent once per connection, immediately after the handshake, and it is the **authority** for
/// the name a device displays — not the pairing code the device scanned. A profile renamed on
/// the desktop must reach the phone's list on the next connect, and a name baked into a QR at
/// pairing time never would.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstanceInfo {
    /// Stable for the life of this profile's state directory, minted once and remembered.
    ///
    /// It is what lets a device recognise a re-pair of a cide it already knows instead of
    /// listing the same machine twice — an address cannot do that job, because a laptop's
    /// address changes with the network and two profiles on one machine share it.
    pub id: String,
    /// What the device puts in its list: `profile::title_prefix()` followed by the hostname, so
    /// `thinkpad` and `[DEV] thinkpad` read exactly as the two OS window titles do.
    pub name: String,
    /// The profile, absent for the production instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile: Option<String>,
    /// cide's own version, for a device that wants to say which end is behind.
    pub version: String,
}

/// A project, as a device lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteProject {
    pub id: ProjectId,
    pub name: String,
    /// The primary root in display form, e.g. `~/work/cide`.
    pub display_path: String,
    /// The CSS colour of the header tab's dot.
    ///
    /// Carried so a device can colour a project the way cide does, which is the cheapest way to
    /// make two surfaces onto one workspace feel like one thing. A device that cannot parse
    /// `var(--accent)` ignores it; it is a hint, never a requirement.
    pub dot: String,
}

/// A live session, and the pane it is drawn in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteSession {
    pub session: SessionId,
    pub project: ProjectId,
    pub pane: PaneId,
    /// The tab that owns the pane, absent when the pane is **detached** — torn out into an OS
    /// window of its own, and therefore in no tab's tree.
    ///
    /// Absent is not an error and not a missing lookup. It is a state a pane is genuinely in,
    /// and the reason `cide_core::workspace::session_panes` exists: the walk it replaced went
    /// over tabs alone and reported a detached pane's running `claude` as nothing at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tab: Option<TabId>,
    /// The tab's own label, when the pane is in one. (M75)
    ///
    /// The name a person recognises. A pane's `title` is what the pane calls itself and is
    /// `claude` for every Claude console on the machine, so a device listing consoles drew a
    /// column of identical rows; the tab is what tells them apart, and it is the same string
    /// the workspace tab strip draws.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<String>,
    pub title: String,
    pub kind: PaneKind,
    pub role: PaneRole,
    pub state: SessionState,
    /// Whether this session is in the *finished and not yet looked at* set.
    ///
    /// Reported here as well as in the authoritative awaiting frame so a freshly connected
    /// device paints a correct list from its first snapshot, without having to join two answers
    /// that were taken at different instants.
    pub awaiting: bool,
    /// The agent run that owns this session, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run: Option<RunId>,
    /// The role a run is running under, e.g. `reviewer`. Absent for a session a person started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent: Option<AgentId>,
    /// The task a run is working on, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task: Option<TaskId>,
}

/// One entry of the *finished and not yet looked at* set.
///
/// The stamp is what makes a **local** notification correct, and a bare set of ids cannot do it.
/// A device that was asleep when a turn ended reconnects to a set, and has to decide whether each
/// entry is news. Without a stamp it can only choose between announcing everything it sees (a
/// notification every time the socket drops and comes back) and announcing nothing it has seen
/// before (silence for a session that went busy and finished *again* while it was away). With
/// one, the rule is a comparison: newer than the last one announced for that session is news.
///
/// It is written when the session *enters* the set and never touched while it stays there —
/// every window observes the same transitions and reports them, and a repeat report that moved
/// the stamp would make one session's single wait look like a stream of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AwaitingEntry {
    pub session: SessionId,
    /// Milliseconds since the epoch, as a JavaScript `number` rather than a `bigint`.
    ///
    /// ~1.7e12 today against a `Number.MAX_SAFE_INTEGER` of ~9.0e15, so the range is not close;
    /// `a_stamp_stays_inside_a_javascript_number` pins it. `bigint` would be technically
    /// truthful and would make every consumer write a cast that can only succeed — and the first
    /// one to forget it gets `NaN` in a date, which is [`crate::properties`]' argument in a
    /// second place.
    #[ts(type = "number")]
    pub since_unix_ms: u64,
}

/// A repaint of a watched session, whole or partial.
///
/// **Only the lines that changed**, unless `full`. A device holds the grid it was last sent and
/// applies each update onto it; that is what makes an active terminal a few hundred bytes a tick
/// instead of a screenful, and it is the reason the sender keeps a digest per line rather than
/// asking the device what it has.
///
/// `epoch` is the grid's identity. It changes when [`ScreenInfo`] does — a resize, or the
/// alternate screen going in or out — and a change of epoch always arrives with `full`, because
/// a diff across two different grids is not a diff. A device that receives an unfamiliar epoch
/// throws its cache away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScreenUpdate {
    pub session: SessionId,
    #[ts(type = "number")]
    pub epoch: u64,
    /// `lines` is the whole grid rather than the part of it that moved.
    pub full: bool,
    pub info: ScreenInfo,
    /// `None` when the cursor is hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cursor: Option<Cursor>,
    pub lines: Vec<ScreenLine>,
}

/// A keystroke, as a device describes it.
///
/// **Semantic, never bytes**, and that is not a convenience — it is the only shape that can be
/// correct. Whether an arrow key is `CSI A` or `SS3 A` depends on DECCKM, and whether pasted text
/// must be wrapped in `CSI 200~` depends on bracketed-paste mode; both are modes of the *child*,
/// known only to the screen mirror. A device that encoded its own bytes would be guessing at
/// state it cannot see, and the failure is an arrow key arriving in a TUI as a literal `A`.
///
/// It also means the device needs no terminal knowledge at all: a key bar sends `up`, and what a
/// program two machines away actually receives is cide's problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct KeyEvent {
    pub key: KeyName,
    /// The character for [`KeyName::Char`], ignored otherwise.
    ///
    /// A `String` rather than a `char` because a soft keyboard emits graphemes — an emoji, an
    /// accented letter composed from two code points — and a terminal takes the UTF-8 either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub shift: bool,
}

/// Which key. A closed set, because an open one would be a device inventing keys cide has no
/// encoding for and no way to refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "k")]
#[ts(export)]
pub enum KeyName {
    /// Ordinary typing. The character is in [`KeyEvent::text`].
    Char,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 through F12. A number outside that range is refused rather than guessed at.
    Function {
        n: u8,
    },
}

/// A permission prompt, as a device is shown it.
///
/// Produced by `cide_claude::permission::parse` and carried here, rather than declared in that
/// crate and converted: `cide-ipc` is where a wire type lives, and a domain struct plus a wire
/// struct plus a `From` between them is three places for a field to go missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PermissionPrompt {
    /// The lines above the options, in order, with the box drawing taken off.
    pub question: Vec<String>,
    pub options: Vec<PromptOption>,
    /// Which option the TUI has highlighted, when it marks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub selected: Option<u8>,
    /// A hash of the normalised block, and **the guard on every answer**.
    ///
    /// Over a network the prompt a person tapped can already have been replaced by the *next*
    /// one. Without this, a tap on *"No, tell Claude what to do differently"* lands as *"1. Yes"*
    /// on a prompt that arrived 300 ms later, and approves whatever it was asking about. The
    /// answer is refused when this has moved.
    pub digest: String,
}

/// One numbered choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PromptOption {
    pub number: u8,
    pub label: String,
}

// --- the wire ----------------------------------------------------------------------------

/// The protocol a device and a cide must agree on.
///
/// The compatibility rule, and it is deliberately blunt in one direction:
///
/// * Adding a [`ServerBody`] variant, or an optional field to any frame, **keeps** the number.
///   A device ignores what it does not know, which is what lets a newer cide serve an older app.
/// * Removing or renaming anything, or changing what a field means, **bumps** it.
/// * A server refuses a [`ClientBody::Hello`] whose `protocol` is greater than its own, by name
///   — and a device that is refused stops, rather than retrying. A reconnect loop against an
///   incompatible peer is the worst failure available here: it drains a battery, fills a log and
///   tells the user nothing.
///
/// Finer-grained branching belongs on [`ServerBody::Welcome`]'s `features`, not on this number.
/// A device asking *can you do X* gets a better answer than one asking *are you new enough*, and
/// the feature list is what makes a partial rollout (a build with the screen but not the prompt
/// parser, say) describable at all.
pub const PROTOCOL_VERSION: u32 = 1;

/// Who is connecting. Shown in cide's device list, and never trusted for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ClientInfo {
    /// What the user called the device, e.g. `Pixel 9`.
    pub name: String,
    /// `android`, `ios`, or whatever else turns up. A free string on purpose: an enum here
    /// would refuse a client cide has not heard of, and the value is drawn in a list and used
    /// for nothing.
    pub platform: String,
    pub app_version: String,
}

/// One frame from a device.
///
/// Envelopes carry an optional `id` so an answer can be matched to its question. A frame with no
/// `id` expects no reply and gets none — the subscription frames are like that, because their
/// answer is a stream rather than a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ClientFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<u32>,
    pub body: ClientBody,
}

/// What a device may say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "t")]
#[ts(export)]
pub enum ClientBody {
    /// Redeem a pairing code. **The only frame an unauthenticated connection may send**, and it
    /// may send it once: anything else, or a second attempt, closes the socket. A short code is
    /// only strong against a single guess, so a retry budget is exactly what would make it
    /// guessable.
    Pair { code: String, client: ClientInfo },
    /// Say which protocol this device speaks. The first frame of a resumed connection.
    ///
    /// **It carries no credential**, and that is the point of the sealed transport: the device
    /// was identified in the handshake and authenticated by being able to seal a frame this cide
    /// could open. There is nothing here for anybody watching the network to take, because after
    /// pairing nothing is ever sent.
    Hello { protocol: u32, client: ClientInfo },
    /// Declare, in full, which projects this connection wants news about.
    ///
    /// **A replace and never a delta.** An array that says what the set *is* can be re-sent
    /// after a reconnect and leave the server in a known state; one that says what the caller
    /// *did* cannot, because the server has no way to tell a repeat from a change. The same rule
    /// `cide_task_link` states for a task's links.
    Subscribe { projects: Vec<ProjectId> },
    /// Answer a permission prompt by tapping one of its options.
    ///
    /// `expect_screen` is the digest of the prompt the device was *shown*. The server re-reads
    /// the screen, re-parses it, and refuses when that has moved — see
    /// [`PermissionPrompt::digest`] for the 300 ms this exists to close.
    AnswerPrompt {
        session: SessionId,
        option: u8,
        expect_screen: String,
    },
    /// One keystroke, into a session.
    ///
    /// `seq` rides `SessionRegistry::accept_write`'s existing watermark — the same at-least-once
    /// guard the webview uses, which already exists precisely for a client that counts
    /// independently. A reconnecting device resets its own epoch and does not have its first
    /// keystrokes swallowed as duplicates.
    Input {
        session: SessionId,
        key: KeyEvent,
        #[ts(type = "number")]
        seq: u64,
        /// The digest of the prompt on screen, **required while a session is awaiting
        /// permission** and ignored otherwise.
        ///
        /// Free text is legitimate at a permission prompt — it accepts typed redirection, which
        /// is what option three is for — so refusing it outright would take a capability away.
        /// What must not happen is a stray Enter from a soft keyboard landing on a prompt the
        /// user never saw, and the device *can* see the screen, so it is asked to say which one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        expect_screen: Option<String>,
    },
    /// Pasted text, wrapped or not according to the child's own bracketed-paste mode.
    ///
    /// A separate frame from a run of [`Self::Input`]s because a paste **is** a different thing
    /// to a terminal: a program that asked for bracketing reads the markers to tell typed text
    /// from pasted, and a TUI's own paste detection eats a submit that arrives in the same chunk.
    /// Sending a paste as N keystrokes would take that distinction away from the child.
    Paste {
        session: SessionId,
        text: String,
        #[ts(type = "number")]
        seq: u64,
        /// See [`Self::Input`]'s field of the same name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        expect_screen: Option<String>,
    },
    /// Scroll the **program**, by sending it a wheel. (M76)
    ///
    /// Not a variant of [`Self::Input`], because a wheel is not a key and — unlike every key —
    /// it is *refused* rather than encoded when the child has not asked for mouse reports. To a
    /// program that never asked, the bytes are typed characters, so a wheel aimed at a shell
    /// would put `[<64;40;12M` on its command line. `cide_remote::keys::wheel` is the one place
    /// that decides, and it reads the mode off the mirror rather than trusting the device.
    ///
    /// It exists because a full-screen program keeps no scrollback for the terminal to page
    /// through: `claude` takes the alternate screen, so its transcript lives in the program and
    /// the only way to see earlier output is to ask the program to redraw. A device holds no
    /// history of its own for such a session — [`Self::ScrollbackPage`] answers `depth: 0`,
    /// correctly — and this is what it sends instead.
    ///
    /// `lines` is signed: negative is **up**, towards older output.
    Scroll {
        session: SessionId,
        lines: i16,
        #[ts(type = "number")]
        seq: u64,
    },
    /// This session has been looked at; clear its *finished and not yet seen* mark.
    ///
    /// **One session, never a set.** `ui/src/panes/awaiting.ts` states the rule and the reason: a
    /// surface that has observed nothing yet would report its empty set and clear every marker in
    /// the application. A device consumes the whole set and reports one entry at a time.
    Acknowledge { session: SessionId },
    /// Start receiving repaints of this session's screen.
    ///
    /// Watching is what costs cide anything: with nothing watched, no grid is read and no timer
    /// runs. A device that is looking at a list rather than a terminal should watch nothing, and
    /// that is the difference between a companion app somebody keeps and one they delete for
    /// eating the battery.
    WatchScreen { session: SessionId },
    /// Stop. Implied by the connection ending.
    UnwatchScreen { session: SessionId },
    /// A page of retained scrollback, counted in lines from the **oldest**.
    ///
    /// From the top because the scrollback grows from the bottom: an offset from the end names a
    /// different line every time the child prints anything, so a reader paging backwards through
    /// one would see rows repeat and rows vanish.
    ScrollbackPage {
        session: SessionId,
        from_top: u32,
        rows: u16,
    },
    /// Stop an agent run.
    ///
    /// `force` is the outright kill; without it cide asks the run to wind down and waits out the
    /// project's grace. Note that cide refuses to *ask* a run that is awaiting permission and
    /// kills it instead, whatever this field says — the wind-down is typed text ending in a
    /// carriage return, and at a permission prompt that return **is an approval**.
    RunStop {
        project: ProjectId,
        run: RunId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        force: bool,
    },
    /// Freeze a run, or this project's whole queue when `run` is absent.
    RunPause {
        project: ProjectId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        run: Option<RunId>,
    },
    /// Thaw what a pause froze.
    RunResume {
        project: ProjectId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        run: Option<RunId>,
    },
    /// Put a role onto a task.
    Dispatch { request: DispatchRequest },
    /// Create a task.
    TaskNew { task: TaskNew },
    /// Edit one.
    ///
    /// Answered with nothing on success: the new board arrives on its own, because a device that
    /// can edit a task is subscribed to that project. Two answers to one gesture would be two
    /// versions of the same rows, one of them always staler.
    TaskEdit {
        project: ProjectId,
        task: TaskId,
        edit: TaskEdit,
    },
    /// Fetch one task's contents. (M75)
    ///
    /// A device holds rows and asks for a body only when somebody opens one — the board carries
    /// no content for the reason `TaskRow` exists at all.
    TaskGet { project: ProjectId, task: TaskId },
    /// Page the **view** of a session, up (negative) or down: PgUp/PgDn on a phone. (M91)
    ///
    /// Not [`Self::Input`]'s `PageUp`, which is `ESC[5~` handed to the program — ignored by a
    /// shell's readline, since in a real terminal paging is the *terminal's* job, and so a phone's
    /// PgUp scrolled nothing anywhere unless a full-screen program happened to bind the key. And
    /// not [`Self::Scroll`], which only ever talks to the program. This asks cide to do what a
    /// person's Shift+PgUp at the desk would: on the normal screen, scroll the desktop pane's own
    /// scrollback (the device pages its own copy of the same history, so the two move together);
    /// on an alternate screen with mouse reports, a wheel of `pages` screenfuls to the program,
    /// whose redraw both ends then show. Refused — as an `Error`, never silently — on an
    /// alternate screen that did not ask for the mouse, where there is nothing to scroll.
    ///
    /// Offered when [`ServerBody::Welcome`]'s `features` carries `"scrollView"`.
    ScrollView {
        session: SessionId,
        pages: i8,
        #[ts(type = "number")]
        seq: u64,
    },
    /// A project's milestones, gates and proposals, as the desk's Milestones tab draws them. (M91)
    ///
    /// Answered with [`ServerBody::Milestones`]; a subscribed project is also pushed one whenever
    /// a gate starts or finishes, so a device learns "running" and the verdict without polling.
    /// Offered when `features` carries `"milestones"`.
    MilestonesGet { project: ProjectId },
    /// Run a milestone's gate now, in the background. The verdict arrives as a pushed
    /// [`ServerBody::Milestones`] — twice, as on the desk: once running, once finished.
    GateRun {
        project: ProjectId,
        milestone: String,
    },
    /// The user accepts a milestone whose gate passed: its task is done and the next one is
    /// active. Answered with the new [`ServerBody::Milestones`].
    MilestoneAccept {
        project: ProjectId,
        milestone: String,
    },
    /// The full output of a gate (`kind: "gate"`, `key` a milestone id) or of a verify
    /// (`"verify"`, `key` a task id) — the last MiB of it. Answered with [`ServerBody::CheckLog`].
    CheckLog {
        project: ProjectId,
        kind: String,
        key: String,
    },
    /// Keep the connection honest. RN's `WebSocket` exposes no protocol-level ping, and a NAT
    /// mapping that has gone away is otherwise indistinguishable from a quiet server.
    Ping,
}

/// One frame to a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerFrame {
    /// The `id` of the frame this answers, absent for anything the server said on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<u32>,
    pub body: ServerBody,
}

/// What cide may say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "t")]
#[ts(export)]
pub enum ServerBody {
    /// The answer to a `Hello`: which cide this is, and what it can do.
    Welcome {
        protocol: u32,
        instance: InstanceInfo,
        /// Capability names, for a device that wants to ask *can you* rather than *are you new
        /// enough*. See [`PROTOCOL_VERSION`].
        features: Vec<String>,
    },
    /// The answer to a `Pair`: the credentials to keep.
    Paired {
        device: String,
        key: String,
        /// Which cide this is, and what it calls itself.
        ///
        /// Sent here because a device that paired by a **typed address** has never been told
        /// either: the QR carries both, and the typed road has only an address and eight
        /// characters. Without this the app would have to invent an instance id, and a
        /// placeholder shared by every typed pairing makes two machines look like one — so the
        /// list would show one entry that connects to whichever answered last.
        instance: String,
        label: String,
    },
    /// The open projects. `rev` is the workspace revision they were read at, so a device can
    /// drop a snapshot older than one it already has — `ui/src/store/workspace.ts`' rule, for
    /// the same reason: two readers of one tree can be answered out of order.
    Projects {
        #[ts(type = "number")]
        rev: u64,
        projects: Vec<RemoteProject>,
    },
    /// Every pane holding a session, in the projects this connection asked about.
    Sessions {
        sessions: Vec<RemoteSession>,
    },
    /// One session moved between idle, busy, awaiting permission or exited.
    ///
    /// Not narrowed by subscription, unlike the lists. A device's *what is happening right now*
    /// screen reads across every project it can see, and so does its notification rule, so
    /// withholding a transition because the device had not opened that project would make the
    /// first screen it lands on the one screen that is wrong. A transition is two fields.
    SessionState {
        session: SessionId,
        state: SessionState,
    },
    /// The whole *finished and not yet looked at* set, as cide currently believes it.
    ///
    /// Whole, in this direction only. A device reports an acknowledgement **one session at a
    /// time and never a set** — `ui/src/panes/awaiting.ts`'s rule, which exists because a
    /// surface that has observed nothing yet would otherwise report its empty set and clear
    /// every marker in the application.
    Awaiting {
        entries: Vec<AwaitingEntry>,
    },
    /// A refusal, in [`crate::CoreError`]'s shape.
    ///
    /// `kind` is for branching and `detail` is for reading. Two fields rather than one string
    /// because the client that shows this to a person has the same problem
    /// `ui/src/ipc/errorText.ts` was written for: a rejection rendered with the equivalent of
    /// `String(e)` is how a user is shown `[object Object]` instead of a sentence.
    Error {
        kind: String,
        detail: String,
    },
    /// A watched session is asking a permission question, and these are its options.
    Prompt {
        session: SessionId,
        prompt: PermissionPrompt,
    },
    /// The prompt that was on a watched session's screen is no longer there.
    ///
    /// Said out loud for [`Self::ScreenGone`]'s reason: on a phone, a card that has stopped
    /// changing and a card about a question that was answered elsewhere are the same picture.
    PromptGone {
        session: SessionId,
    },
    /// A dispatch started. The run id, so a device can open what it just started.
    Dispatched {
        run: RunId,
    },
    /// A watched session repainted.
    Screen {
        update: ScreenUpdate,
    },
    /// A page of retained scrollback.
    Scrollback {
        session: SessionId,
        page: ScrollbackCapture,
    },
    /// A watched session no longer exists, so no repaint is coming.
    ///
    /// Said rather than left to silence, because on a phone the two are the same picture: a
    /// terminal that has stopped changing. A device that is not told draws a live-looking screen
    /// of a child that exited an hour ago.
    ScreenGone {
        session: SessionId,
    },
    /// This connection missed something and must re-read what it is subscribed to.
    ///
    /// The server's queue towards a device is **bounded**, and a device that cannot keep up is
    /// told to resynchronise rather than allowed to back-pressure the thing producing the news.
    /// That is `cide-pty`'s credit protocol's argument in a second place: the producers here are
    /// the hook-apply thread and the GTK main loop, and a queue that grew behind a phone on a
    /// train would grow on those.
    /// The agent runs of a subscribed project, newest first. (M75)
    ///
    /// [`crate::AgentRun`] itself rather than a projection: it is already a small row built for
    /// cide's own panel, it names no credential, and the labels it carries are *copied at
    /// dispatch* — so a device showing a finished run still reads correctly after the role was
    /// renamed, which is the property that struct's own doc argues for at length.
    Runs {
        project: ProjectId,
        runs: Vec<crate::AgentRun>,
    },
    /// The roles a subscribed project can dispatch, and whether it will. (M75)
    Roster {
        project: ProjectId,
        agents: Vec<RemoteAgent>,
        /// Whether the queue will start anything new.
        ///
        /// **Distinct from "every run is paused", and carrying only the second was a bug.**
        /// Pausing a project does two things — it shuts the dispatch queue *and* freezes the
        /// children — so a project paused while nothing happened to be running looked, on a
        /// device, exactly like an idle one. `AgentRoster::Ready`'s own field says this at
        /// length; a device is owed the same fact for the same reason, because it has the same
        /// Resume button and no way to tell which of the two it is about to undo.
        dispatching: bool,
    },
    /// One task, whole. (M75)
    ///
    /// `task` is `None` when there is no such task — a board a device is holding can name one
    /// somebody has since deleted, and that is an answer rather than an error.
    Task {
        project: ProjectId,
        id: TaskId,
        /// Boxed, because a `TaskDetail` carries a body, every comment, every attachment
        /// record and the whole status history — and an enum is as large as its largest
        /// variant, so inlining it would make every `Pong` and every `SessionState` carry that
        /// much stack. Serde and ts-rs both see straight through a `Box`, so the wire and the
        /// TypeScript are unchanged.
        #[ts(optional)]
        #[serde(skip_serializing_if = "Option::is_none")]
        task: Option<Box<crate::TaskDetail>>,
    },
    /// The task board of a subscribed project. (M75)
    ///
    /// Rows, never content: `TaskRow` exists because the board used to carry every word ever
    /// written into a tracker. A device that wants a task's body asks for it.
    Board {
        project: ProjectId,
        tasks: Vec<crate::TaskRow>,
    },
    /// A project's milestones. (M91) `view` is absent when the project defines none, or is not
    /// open here. Boxed for [`Self::Task`]'s reason.
    Milestones {
        project: ProjectId,
        #[ts(optional)]
        #[serde(skip_serializing_if = "Option::is_none")]
        view: Option<Box<crate::MilestonesView>>,
    },
    /// A check's log, the answer to [`ClientBody::CheckLog`]. `text` is absent when that check
    /// has never run here.
    CheckLog {
        project: ProjectId,
        kind: String,
        key: String,
        #[ts(optional)]
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Desync {
        why: String,
    },
    Pong,
    /// cide is shutting down. Sent before the socket closes, so a device can say *the machine
    /// went away* rather than *something went wrong*.
    GoingAway {
        why: String,
    },
}

/// The `kind` values [`ServerBody::Error`] uses, so both ends spell them once.
pub mod error_kind {
    /// The `Hello` named a protocol this cide does not speak.
    pub const PROTOCOL: &str = "protocol";
    /// The device or token was not recognised, or the device has been revoked.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// A frame arrived that this connection is not allowed to send yet.
    pub const UNEXPECTED: &str = "unexpected";
    /// A frame arrived that this cide does not know. **Answered, never fatal**: a device built
    /// against a newer cide must degrade, not disconnect.
    pub const UNKNOWN_FRAME: &str = "unknownFrame";
    /// The pairing code was wrong, expired, or already used.
    pub const PAIRING: &str = "pairing";
    /// The thing named — a project, a session, a task — is not here.
    pub const NO_SUCH: &str = "noSuch";
    /// The request was understood and refused. `detail` says why, in a sentence.
    pub const REFUSED: &str = "refused";
}

// --- what the Settings panel draws --------------------------------------------------------

/// What the remote listener is doing, for the screen that turns it on.
///
/// Three arms rather than a struct with a nullable error, because they are three different
/// things to *say*: nothing is running, this is where to point a phone, or here is why it could
/// not start. A panel that had to work out which of those it was holding would get it wrong on
/// the arm that matters — a refusal rendered as an empty address list reads as "starting…".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
#[ts(export)]
pub enum RemoteStatus {
    /// The listener is off, which is the default.
    Off,
    /// Listening. `addresses` are the ones a device could plausibly use, most useful first —
    /// loopback and link-local are left out, because neither helps a phone.
    Listening {
        addresses: Vec<String>,
        port: u16,
        /// Devices that have connected at least once since cide started.
        connected: u32,
    },
    /// It could not start, and this is the sentence to show.
    ///
    /// A configured port that is taken lands here rather than sliding to another one: a number
    /// the user typed is a promise to a device that saved it, and quietly moving would point
    /// that device at whatever else is listening.
    Refused { why: String },
}

/// A paired device, as the panel lists it.
///
/// No token and no hash. `cide-remote`'s `Device` has a hand-written `Debug` so a log cannot
/// print one; this type is the same rule applied to the screen, and there is nothing to mask
/// because there is nothing here to mask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    #[ts(type = "number")]
    pub created_unix_ms: u64,
    /// Zero means it has never connected — paired and then never used, which is worth showing
    /// as its own state rather than as a date in 1970.
    #[ts(type = "number")]
    pub last_seen_unix_ms: u64,
    pub last_addr: String,
}

/// An open pairing window.
///
/// Held in memory and never written down: a code on disk outlives the two minutes it is good
/// for. The panel shows [`Self::grouped`] and a device may type it back with or without the dash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PairingInvite {
    /// The code as typed: eight characters, no punctuation.
    pub code: String,
    /// The code as shown: `XXXX-XXXX`, because eight unbroken characters are read off a screen
    /// wrongly.
    pub grouped: String,
    /// Everything a device needs, for a scan or a paste.
    pub uri: String,
    /// `uri` as a QR module grid, when it could be encoded.
    ///
    /// An `Option` rather than a required field, because the QR is a convenience over a thing
    /// the user can always type: `code` and the address are the payload, and the panel draws
    /// them whatever happens here. An encoder that refused would otherwise be able to stop a
    /// pairing that has nothing wrong with it.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qr: Option<QrMatrix>,
    #[ts(type = "number")]
    pub expires_unix_ms: u64,
}

/// A project's roles and whether its queue is open. (M75)
///
/// One value because the two are always read together and always about the same project — the
/// host reads them from one `AgentRoster` and splitting them across two returns is how the
/// `dispatching` half got dropped the first time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRoster {
    pub agents: Vec<RemoteAgent>,
    pub dispatching: bool,
}

/// A role, as a device lists it. (M75)
///
/// A projection of [`crate::AgentDef`] rather than the thing itself, and the field it leaves out
/// is the reason it exists: `system_prompt` is the role's entire body, kilobytes apiece, and a
/// roster of a dozen roles would put the lot on a phone to render a list of names. That is the
/// board's lesson (`TaskRow` against the tracker's whole contents) arriving a second time, and
/// the cheapest moment to apply it is before anything ships.
///
/// It carries no model **credentials** — only the model *name*, which is what the row says. The
/// keys live in `LlmProvider`, which has a hand-written `Debug` so a log cannot print one, and
/// nothing in `cide-remote` may name that type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteAgent {
    pub id: AgentId,
    pub label: String,
    pub scope: AgentScope,
    pub harness: Harness,
    pub description: String,
    /// The model a run of this role would use, when the definition names one.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The hue the definition **declared**, when it declared one. (M76)
    ///
    /// The name (`blue`, `cyan`, …) and never a resolved colour, because the two ends paint from
    /// different palettes: cide reads `var(--agent-blue)` out of whichever theme is on, and a
    /// phone has no such thing. A hex here would be one theme's answer shipped to a device that
    /// is always dark.
    ///
    /// `None` is not *no colour*: a role without a declared one is coloured from a hash of its
    /// **id**, which both ends compute identically and neither has to store. So this carries the
    /// one fact a device cannot derive, and the derivation stays where it was.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Why this role cannot be dispatched, when it cannot. A sentence, already written for a
    /// person — the panel shows it and so should a device.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// How many runs of this role may be in flight at once, after local overrides.
    pub max_concurrent: u16,
    pub worktree: bool,
    /// How many runs of this role are alive, and how many are waiting for a slot.
    ///
    /// Counted **here**, by the one function that already answers it for cide's own panel, and
    /// never re-derived on the device from the runs list. A count is only ever visibly wrong
    /// beside the list it counts, and two producers give two plausible answers — `TaskRow`'s
    /// rule, which is in this file for the same reason.
    pub running: u32,
    pub queued: u32,
}

/// A device that has connected to redeem a code, and the number it should be showing.
///
/// This is the *typed* road's authentication and nothing else's. A device that scanned the QR
/// already holds cide's public key and could not have completed the exchange without it, so it
/// is authenticated by the key and this never appears. A device that was given only an address
/// had to take the key from the wire, which anybody in the middle could have written — so the
/// two ends each derive six digits from what they actually agreed, and a person compares them.
/// An attacker relaying holds two different conversations and therefore two different numbers.
///
/// `addr` is shown beside it because "a device is pairing" is worth being able to disbelieve: if
/// somebody is at the pairing screen and the address is not their phone's, that is the whole
/// warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PairingAttempt {
    /// Where it connected from, as an address and port.
    pub addr: String,
    /// Six digits, already grouped for reading: `418 302`. Grouped **here**, in one place, so
    /// two screens cannot space it differently and make a match look like a mismatch.
    pub sas: String,
}

/// How the pairing window the panel opened is getting on.
///
/// One answer rather than two commands, because the panel asks both halves on the same event and
/// two round trips could disagree about the same instant — a device redeeming the code between
/// them would be reported as *still pairing* against a window that had already closed.
///
/// `open` is the authority on whether the modal should still be on screen, and it is read from
/// the device store rather than inferred: the code is taken by a redemption, dropped by a wrong
/// guess and forgotten at the end of its two minutes, and all three are the same news to a panel
/// holding a QR nobody can use any more. Inferring it from the device list would have covered
/// only the first, and would have left a dead code on screen after an expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PairingProgress {
    /// Whether a code is still outstanding.
    pub open: bool,
    /// The device standing at the pairing step right now, if one is. See [`PairingAttempt`].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attempt: Option<PairingAttempt>,
}

/// A QR code as a grid of modules, for a caller that owns its own drawing.
///
/// Deliberately not an image, an SVG string or a data URI. The webview draws `<rect>`s into an
/// `<svg>` it sizes itself, so the code scales with the dialog, follows `--ui-scale`, takes its
/// two colours from the theme and costs no bytes beyond this array — and the phone, which
/// receives one of these from nothing, is not a consumer of it at all. It also keeps the
/// encoder testable as an encoder: a test asserts on modules, never on rendered markup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QrMatrix {
    /// Width and height in modules. A QR code is always square.
    pub size: u32,
    /// Row-major, `size * size` entries, `true` where a module is dark.
    ///
    /// A flat array and not `Vec<Vec<bool>>`: the renderer indexes `y * size + x`, and a
    /// nested array would cost a per-row object in the JSON for nothing.
    pub modules: Vec<bool>,
}

impl QrMatrix {
    /// Is a module dark? `false` for anything outside the grid, so a renderer cannot panic on
    /// an off-by-one at the quiet zone.
    #[must_use]
    pub fn dark(&self, x: u32, y: u32) -> bool {
        if x >= self.size || y >= self.size {
            return false;
        }
        self.modules
            .get((y * self.size + x) as usize)
            .copied()
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_stays_inside_a_javascript_number() {
        const MAX_SAFE: u64 = 9_007_199_254_740_991;
        // 2200-01-01, further out than any machine this will run on.
        let far_future_ms: u64 = 7_258_118_400_000;
        assert!(far_future_ms < MAX_SAFE);
        assert!(MAX_SAFE / far_future_ms > 1_000);
    }

    #[test]
    fn an_attached_pane_spends_no_bytes_saying_it_is_not_detached() {
        let session = RemoteSession {
            session: SessionId::new(),
            project: ProjectId::new(),
            pane: PaneId::new(),
            tab: None,
            tab_title: None,
            title: "claude".to_owned(),
            kind: PaneKind::Claude,
            role: PaneRole::Primary,
            state: SessionState::Idle,
            awaiting: false,
            run: None,
            agent: None,
            task: None,
        };
        let json = serde_json::to_string(&session).expect("serialises");
        for absent in ["tab", "run", "agent", "task"] {
            assert!(!json.contains(absent), "{absent} in {json}");
        }
        let back: RemoteSession = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, session);
    }
}
