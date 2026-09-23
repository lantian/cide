// GENERATED FILE — DO NOT EDIT.
//
// cide's remote wire, written by `cargo xtask codegen` from `#[derive(TS)]` types in
// `crates/cide-ipc`. This is the **whole** vocabulary and nothing but it: the transitive
// closure, in name order, of
//     ClientFrame, ServerFrame
// A type that is not reachable from a frame is not on this wire and is not in this file, which
// is what makes reading this file the same thing as reading the protocol.
//
// Notably absent, and structurally so: `Workspace` and everything under it. It carries
// `Settings`, which carries provider API keys and proxy passwords in plaintext. Nothing on this
// wire can name it. See `crates/cide-ipc/src/remote.rs`.
//
// The companion application vendors this file by copy and compares PROTOCOL_VERSION against what
// a server announces in its `welcome` frame. The rules are in `crates/cide-remote`: adding a
// variant or an optional field keeps this number, removing or renaming anything bumps it.
//
// CI runs `cargo xtask codegen --check`, so a Rust rename that never reached this file fails the
// build rather than surfacing as an `undefined` on somebody's phone.

export const PROTOCOL_VERSION = 1

/**
 * A subagent role name — `developer`, `qa`, `artist`. (M18)
 *
 * A string, and not a uuid, because **the user writes it themselves in a file in their own
 * repository**: it is the `name:` key of `.cide/agents/<name>.md`, it is what they type into a
 * dispatch, and it is what the orchestrator names when it hands work over. A uuid is
 * unwritable in every one of those positions.
 *
 * The generated-id alternative — cide mints an id and the file carries a display name — was
 * considered and lost for a sharper reason than ergonomics: it would mean the definition file
 * had to be **round-tripped through cide once before it meant anything**, because until cide
 * had read it and written an id back, nothing else could refer to that role. A committed
 * config file that does nothing until the app has rewritten it is not a file a team can
 * review in a pull request, which is the whole point of putting it in the repository.
 *
 * Deliberately not validated here. This crate is the wire shape; `cide-agents` is where a name
 * is checked against its file stem and where a duplicate across the project and global
 * directories is reported, because that is the layer that knows the path and the line number
 * to name in the error.
 */
export type AgentId = string;

/**
 * One dispatched execution, as the panel draws it.
 */
export type AgentRun = { run: RunId, agent: AgentId, 
/**
 * [`AgentDef::label`] **copied at dispatch, not looked up at render**.
 *
 * A history row must still read correctly after the role has been renamed or its definition
 * deleted, and a row that re-derives its label from the current roster does neither: it
 * renames itself when a config file changes, or empties when a role is removed. A row that
 * renames itself is a row that lies about what happened — the run really was dispatched
 * under the old name, and that is the name in the transcript, in `/resume` and in the
 * terminal title.
 */
agentLabel: string, 
/**
 * Copied for the same reason as [`Self::agent_label`]: the run happened on this harness
 * whatever the definition says now.
 */
harness: Harness, project: ProjectId, 
/**
 * The PTY session, once there is one.
 *
 * `None` while [`RunState::Queued`] — there is no child yet — which is exactly why the Open
 * action is *withheld* until there is, rather than drawn disabled. A queued row renders no
 * Open control at all: a control that cannot work in this state should not be on the screen
 * in it.
 *
 * It is also the join key for the phase dot. The Agents store subscribes to
 * `cide://session-state` keyed on this, so a row's colour moves at session speed with no
 * round trip and no second per-run event stream.
 */
session: SessionId | null, state: RunState, 
/**
 * The task this run was dispatched against.
 *
 * **This is the whole agent→task half of the cross-link, and it is the only stored half.**
 * The reverse — "which agent is on task t-17 right now" — is derived at render time by
 * scanning live runs for this field; see [`crate::Task::agent`], which is the role a task is
 * *for* and not a claim about the present.
 *
 * `None` is legal: an ad-hoc run — the orchestrator asking a role to check or do one small
 * thing that is not on the board. Such a run stands in the **project root**, not in a
 * worktree of its own (M40; `cide_agents::run_checkout` is the rule). The row draws
 * `no task` in dim rather than nothing, because a run nothing can account for is worth
 * seeing.
 */
task: TaskId | null, startedUnixMs: bigint, 
/**
 * Milliseconds this run has **worked**, over its closed working intervals only.
 *
 * Read together with [`Self::working_since_unix_ms`]: the figure to show is this plus, when
 * that stamp is set, the time since it. Splitting a closed total from an open stamp is what
 * lets a live row tick between broadcasts without the producer sending a new number every
 * second, and what makes a paused or finished row's figure *stop* rather than freeze at
 * whatever the last event carried.
 *
 * [`RunState::counts_as_work`] is the definition of "working", and its doc carries the
 * argument for where the line is drawn.
 *
 * **This does not replace [`Self::started_unix_ms`]**, which stays exactly what it was: the
 * moment the run was dispatched. That is a real fact, it is what the panel's History sorts
 * by, and it is what `cide_agents::tools`' run list means by `started … ago` — which was
 * never wrong, because it says "started". What was wrong was rendering it as the run's
 * duration.
 */
workedMs: bigint, 
/**
 * When the current working interval began, or `None` when this run is not working.
 *
 * `Some` in exactly the states [`RunState::counts_as_work`] admits, which is the invariant
 * `cide_app::agents::move_to` exists to keep — it is the only thing in the process that
 * assigns a run's state, and it opens and closes this stamp on the transition.
 *
 * Deliberately **not** persisted across a cide restart. Every restored run comes back
 * `Interrupted`, which is not working, so this is `None` by construction and the hours cide
 * spent shut add nothing to the figure — the offline case needs no snapshot timestamp and
 * no code of its own.
 */
workingSinceUnixMs: bigint | null, 
/**
 * Where cide announces this run's turn endings — the nudge `agent_rpc::note_run_over` types
 * into a Claude pane. Recorded at dispatch and carried on the wire so the run list can say
 * which runs will never announce themselves. (M40)
 */
notify: RunNotify, 
/**
 * The run was frozen long enough that its in-flight model request may have timed out.
 *
 * **A field on the run, not an event**, and that is the load-bearing part. A window opened
 * *after* the resume has no history to derive this from, so an event-only design shows
 * nothing in the new window where the old one shows a warning — the exact failure
 * `cide://session-awaiting`'s doc records and answers by broadcasting the whole set. State
 * that a late-joining window must agree about belongs in the snapshot.
 *
 * A *suspicion*, surfaced as an offer, never an automatic re-dispatch: cide cannot see the
 * model request, only a process that was stopped and continued, and a re-dispatch would
 * double-bill a turn that in fact survived. The user answers Retry or Leave it, and
 * `agents_ack_stale_turn` clears it.
 */
staleTurn: boolean, 
/**
 * One line of extra context for the row — what the queue is waiting on, which worktree this
 * run holds. `None` for the ordinary case, which is most of them.
 */
note: string | null, 
/**
 * Whether **Open** has anything to show for this run. (M42)
 *
 * The gate the panel reads, and it is a field rather than a rule the panel derives from
 * [`Self::session`] because `session` stopped being the whole answer: a finished `opencode`
 * run's child is gone and its cide session with it, yet its conversation (`ses_…`) can be
 * re-opened in the real harness, and a finished `claude` run restored after a restart has
 * no session at all while its transcript sits on disk. The registry computes this from what
 * it holds in memory — a session it still owns, or a conversation it has confirmed can be
 * re-opened — so a roster broadcast costs no filesystem read. `false` for a queued run,
 * which is what keeps Open *withheld* there rather than drawn disabled.
 *
 * What Open then does is a second question with three answers — see [`RunOpen`].
 */
openable: boolean, 
/**
 * The model this run is on, `provider/model` where the harness spells it that way. (M89)
 *
 * **The run's, never the role file's.** After a local override or a pool failover the file
 * names what the *next* dispatch would get; this names what the child was actually told,
 * resolved by the same ladder as [`LogRunInfo::model`] so the panel's row and the log card
 * never disagree. `None` where nothing chose one and the harness picked its own default —
 * the row then draws the harness alone rather than guessing a name.
 */
model: string | null, 
/**
 * The pool that chose [`Self::model`] and how far down it the run is — `fast entry 2 of 3`.
 * (M89) `None` for a run with no pool, which is every harness but opencode today.
 *
 * A field rather than the sentence it used to be inside [`Self::note`]: a pool is the
 * standing fact of what the run is on, and a note is for events. A row that has to parse
 * prose to learn its model is a row that breaks on the next rewording.
 */
poolPosition: string | null, };

/**
 * Which of the two directories a definition lives in.
 *
 * The four directories `cide_agents::defs::load_from` merges, named on the wire because **a
 * form has to say which file it is about**. The panel draws one `developer` row where two files may declare it
 * — a project definition shadowing a global one, which `LoadedAgent::shadows` exists to make
 * visible — so a save that guessed would edit whichever the guess landed on, and the user would
 * watch their change have no effect for the same reason shadowing costs an afternoon today.
 *
 * It is also the only control the user has over that merge. Moving a role from `Project` to
 * `Global` is how it stops being this repository's and becomes theirs, on every project they
 * open; there is no key in the file for that, because it *is* which directory the file is in.
 * So scope is a field of the draft and changing it is a move — see `cide_agents::defs::save`.
 *
 * # Two families, four directories
 *
 * The first two are cide's own. The second two are **Claude Code's** (M30): a subagent is the
 * same idea in somebody else's format, documented at <https://code.claude.com/docs/en/sub-agents>,
 * and the directories are already populated on the machines of people who have never opened
 * cide. Reading them is not a translation layer bolted on — they merge into the one catalog
 * through the one merge, because a role's identity is its name and two lists keyed by name
 * would be two answers to "who is `reviewer`".
 *
 * What the families do *not* share is who owns the vocabulary. cide defines the keys in
 * `.cide/agents/` and may refuse one it does not know; it defines none of the keys in
 * `.claude/agents/` and must therefore preserve every one of them — see
 * [`AgentDraft::extras`], which exists for exactly that reason.
 */
export type AgentScope = "project" | "global" | "claudeProject" | "claudeGlobal";

/**
 * What an attachment's bytes are, coarsely: a picture the card can draw, or a file it names.
 *
 * Decided **once, at import, by sniffing the bytes** (`cide_core::image::sniff`), never by the
 * extension — a `.png` that is a text file would otherwise be handed to `<img>` and draw
 * nothing. It is a hint for the panel and for the list an agent reads, and *only* that: the
 * asset-protocol grant that lets a thumbnail load re-sniffs the file on disk every time, because
 * a record in a committed JSON file is a claim anybody can edit and the grant is a capability.
 */
export type AttachmentKind = "image" | "file";

/**
 * One entry of the *finished and not yet looked at* set.
 *
 * The stamp is what makes a **local** notification correct, and a bare set of ids cannot do it.
 * A device that was asleep when a turn ended reconnects to a set, and has to decide whether each
 * entry is news. Without a stamp it can only choose between announcing everything it sees (a
 * notification every time the socket drops and comes back) and announcing nothing it has seen
 * before (silence for a session that went busy and finished *again* while it was away). With
 * one, the rule is a comparison: newer than the last one announced for that session is news.
 *
 * It is written when the session *enters* the set and never touched while it stays there —
 * every window observes the same transitions and reports them, and a repeat report that moved
 * the stamp would make one session's single wait look like a stream of them.
 */
export type AwaitingEntry = { session: SessionId, 
/**
 * Milliseconds since the epoch, as a JavaScript `number` rather than a `bigint`.
 *
 * ~1.7e12 today against a `Number.MAX_SAFE_INTEGER` of ~9.0e15, so the range is not close;
 * `a_stamp_stays_inside_a_javascript_number` pins it. `bigint` would be technically
 * truthful and would make every consumer write a cast that can only succeed — and the first
 * one to forget it gets `NaN` in a date, which is [`crate::properties`]' argument in a
 * second place.
 */
sinceUnixMs: number, };

/**
 * One OpenSpec change, as its directory name under `openspec/changes/` — `add-dark-mode`. (M28)
 *
 * [`TaskId`]'s argument, arriving at the same answer from the other direction: this is a string
 * **a person types and a model quotes**, in a task body, in a comment, on a command line
 * (`openspec show add-dark-mode`), and as a directory name in a pull request. OpenSpec's own
 * grammar for it is `^[a-z0-9]+(?:-[a-z0-9]+)*$` with a 200-character ceiling, which is a
 * kebab-case identifier for exactly those reasons.
 *
 * Not validated here, for [`SpecId`]'s reason — and the check matters more than most, because
 * this value reaches `Path::join`. `cide-tasks` refuses one that is not kebab before it can be
 * written into `.cide/tasks.json`, and `cide-spec` refuses one before it can name a directory.
 */
export type ChangeName = string;

/**
 * What one run of a gate or a verify command answered.
 */
export type CheckResult = { 
/**
 * The command, as run.
 */
command: string, 
/**
 * Exit 0 within the timeout.
 */
passed: boolean, 
/**
 * `None` when the process was killed (a timeout, a signal) rather than exiting.
 */
exitCode?: number, timedOut: boolean, 
/**
 * The last lines of combined output, which is where every test runner prints its verdict.
 */
tail: string, startedUnixMs: number, durationMs: number, 
/**
 * The commit that was checked, when the directory is a git checkout.
 */
head?: string, };

/**
 * What a device may say.
 */
export type ClientBody = { "t": "pair", code: string, client: ClientInfo, } | { "t": "hello", protocol: number, client: ClientInfo, } | { "t": "subscribe", projects: Array<ProjectId>, } | { "t": "answerPrompt", session: SessionId, option: number, expectScreen: string, } | { "t": "input", session: SessionId, key: KeyEvent, seq: number, 
/**
 * The digest of the prompt on screen, **required while a session is awaiting
 * permission** and ignored otherwise.
 *
 * Free text is legitimate at a permission prompt — it accepts typed redirection, which
 * is what option three is for — so refusing it outright would take a capability away.
 * What must not happen is a stray Enter from a soft keyboard landing on a prompt the
 * user never saw, and the device *can* see the screen, so it is asked to say which one.
 */
expectScreen?: string, } | { "t": "paste", session: SessionId, text: string, seq: number, 
/**
 * See [`Self::Input`]'s field of the same name.
 */
expectScreen?: string, } | { "t": "scroll", session: SessionId, lines: number, seq: number, } | { "t": "acknowledge", session: SessionId, } | { "t": "watchScreen", session: SessionId, } | { "t": "unwatchScreen", session: SessionId, } | { "t": "scrollbackPage", session: SessionId, fromTop: number, rows: number, } | { "t": "runStop", project: ProjectId, run: RunId, reason?: string, force?: boolean, } | { "t": "runPause", project: ProjectId, run?: RunId, } | { "t": "runResume", project: ProjectId, run?: RunId, } | { "t": "dispatch", request: DispatchRequest, } | { "t": "taskNew", task: TaskNew, } | { "t": "taskEdit", project: ProjectId, task: TaskId, edit: TaskEdit, } | { "t": "taskGet", project: ProjectId, task: TaskId, } | { "t": "scrollView", session: SessionId, pages: number, seq: number, } | { "t": "milestonesGet", project: ProjectId, } | { "t": "gateRun", project: ProjectId, milestone: string, } | { "t": "milestoneAccept", project: ProjectId, milestone: string, } | { "t": "checkLog", project: ProjectId, kind: string, key: string, } | { "t": "ping" };

/**
 * One frame from a device.
 *
 * Envelopes carry an optional `id` so an answer can be matched to its question. A frame with no
 * `id` expects no reply and gets none — the subscription frames are like that, because their
 * answer is a stream rather than a response.
 */
export type ClientFrame = { id?: number, body: ClientBody, };

/**
 * Who is connecting. Shown in cide's device list, and never trusted for anything else.
 */
export type ClientInfo = { 
/**
 * What the user called the device, e.g. `Pixel 9`.
 */
name: string, 
/**
 * `android`, `ios`, or whatever else turns up. A free string on purpose: an enum here
 * would refuse a client cide has not heard of, and the value is drawn in a list and used
 * for nothing.
 */
platform: string, appVersion: string, };

/**
 * One comment's identity within a task. (M21)
 *
 * # Not shaped like [`TaskId`], deliberately
 *
 * A task id is `t-17` because a *model* has to quote it in prose and a human has to hold it in
 * their head while reading a diff. Nothing quotes a comment id: it is minted by the panel's own
 * edit gesture and travels straight back to Rust, so the properties that shaped `t-17` — short,
 * memorable, survives paraphrase — buy nothing, and the property that matters instead is that
 * two writers never collide.
 *
 * A **string** rather than a `Uuid`, because two kinds of value live here and only one of them
 * is a uuid. A comment written by this build gets `Uuid::new_v4`. A comment already in a
 * `.cide/tasks.json` gets a derived `legacy-<hash>` from `TaskComment::legacy_id`, which is what
 * lets the union rule change from structural equality to id equality without touching a single
 * existing file. Making this a `Uuid` would mean either rewriting every task file on first read
 * or minting a fresh id per read — and a fresh id per read is a merge that duplicates every
 * comment it touches.
 *
 * `Default` is the empty string, which is what `#[serde(default)]` yields for a comment loaded
 * from a file written before this existed. `TasksStore`'s repairing loader fills those in; an
 * empty id must never reach the merge, and `repair` is the seam that guarantees it.
 */
export type CommentId = string;

/**
 * Where the cursor is. Absent from a capture means hidden, not at the origin.
 */
export type Cursor = { row: number, col: number, };

/**
 * Dispatch one run. Inbound.
 */
export type DispatchRequest = { project: ProjectId, agent: AgentId, 
/**
 * The task this run is for.
 *
 * `None` is legal and means an ad-hoc run — the orchestrator asking a role to check or do
 * one small thing that is not on the board. Such a run **stands in the project root**: no
 * worktree, no `cide/<role>-…` branch, nothing to integrate (M40 — before that a task-less
 * run took the role's base worktree, and the user asked for exactly not that: a quick check
 * or a small piece of direct work has nowhere to be merged back *from*). The panel's Dispatch
 * button nonetheless **always** fills it, because a run with no task is a run the Tasks
 * panel cannot account for: it draws no chip on any row, it leaves no trace once it has
 * exited, and the record of what the agents did that afternoon has a hole in it. The MCP
 * road (`cide_agent_dispatch`) may omit it, and its own prose says when that is right.
 */
task?: TaskId, 
/**
 * Extra instruction for this run only, appended after the task body — or, for a run with no
 * task, the whole of its brief.
 *
 * `None` means the task speaks for itself, which is the case the orchestrator should be
 * aiming for: instruction that lives only in a prompt is instruction the next run of the
 * same task never sees.
 */
prompt?: string, 
/**
 * Where the run's turn endings are announced. (M40)
 *
 * `None` reads as [`RunNotify::Primary`], which is what a request that carries no session
 * of its own — the panel's button, an assignment made in the Tasks panel — can honestly
 * ask for. A request made over the agent socket carries the connection's session instead
 * (`agent_rpc::RegistrySink` fills this from the `Scope`), so the pane that asked is the
 * pane that hears.
 */
notify?: RunNotify, };

/**
 * A milestone's gate, as last seen. Outbound.
 */
export type GateState = { milestone: string, 
/**
 * `None` when it has never run on this machine.
 */
last?: CheckResult, 
/**
 * A run is in progress now.
 */
running: boolean, 
/**
 * The full output of the last (or current) run, as a path — for a model that can read a
 * file; the panel asks `milestones_check_log`. Absent until it has run here.
 */
log?: string, };

/**
 * Which CLI actually runs a role.
 *
 * An enum and not a string because it selects an implementation — `cide_agents::harness`
 * dispatches on it — and because the settings surface and the definition parser must agree on
 * the set. A role naming a harness this build does not have is a *parse* error with a line
 * number, which is only possible if the set is closed.
 *
 * More are expected; adding one is a variant here, an `impl Harness` there and one insert into
 * the registry. That is the shape the trait was chosen for.
 */
export type Harness = "claude" | "opencode" | "qwen" | "codex" | "mimo";

/**
 * Which cide a device is talking to.
 *
 * Sent once per connection, immediately after the handshake, and it is the **authority** for
 * the name a device displays — not the pairing code the device scanned. A profile renamed on
 * the desktop must reach the phone's list on the next connect, and a name baked into a QR at
 * pairing time never would.
 */
export type InstanceInfo = { 
/**
 * Stable for the life of this profile's state directory, minted once and remembered.
 *
 * It is what lets a device recognise a re-pair of a cide it already knows instead of
 * listing the same machine twice — an address cannot do that job, because a laptop's
 * address changes with the network and two profiles on one machine share it.
 */
id: string, 
/**
 * What the device puts in its list: `profile::title_prefix()` followed by the hostname, so
 * `thinkpad` and `[DEV] thinkpad` read exactly as the two OS window titles do.
 */
name: string, 
/**
 * The profile, absent for the production instance.
 */
profile?: string, 
/**
 * cide's own version, for a device that wants to say which end is behind.
 */
version: string, };

/**
 * A keystroke, as a device describes it.
 *
 * **Semantic, never bytes**, and that is not a convenience — it is the only shape that can be
 * correct. Whether an arrow key is `CSI A` or `SS3 A` depends on DECCKM, and whether pasted text
 * must be wrapped in `CSI 200~` depends on bracketed-paste mode; both are modes of the *child*,
 * known only to the screen mirror. A device that encoded its own bytes would be guessing at
 * state it cannot see, and the failure is an arrow key arriving in a TUI as a literal `A`.
 *
 * It also means the device needs no terminal knowledge at all: a key bar sends `up`, and what a
 * program two machines away actually receives is cide's problem.
 */
export type KeyEvent = { key: KeyName, 
/**
 * The character for [`KeyName::Char`], ignored otherwise.
 *
 * A `String` rather than a `char` because a soft keyboard emits graphemes — an emoji, an
 * accented letter composed from two code points — and a terminal takes the UTF-8 either way.
 */
text?: string, ctrl?: boolean, alt?: boolean, shift?: boolean, };

/**
 * Which key. A closed set, because an open one would be a device inventing keys cide has no
 * encoding for and no way to refuse.
 */
export type KeyName = { "k": "char" } | { "k": "enter" } | { "k": "escape" } | { "k": "tab" } | { "k": "backspace" } | { "k": "delete" } | { "k": "insert" } | { "k": "up" } | { "k": "down" } | { "k": "left" } | { "k": "right" } | { "k": "home" } | { "k": "end" } | { "k": "pageUp" } | { "k": "pageDown" } | { "k": "function", n: number, };

/**
 * One kind of edge between two tasks. (M30)
 *
 * Each directed kind is **stored in one canonical direction** and read the other way round at
 * render time — [`Task::agent`]'s one-writer rule applied to edges. Storing both directions
 * would mean two rows for one fact, and the first merge with a stale file where only one row
 * survived would be a link that exists from one task and not from the other.
 */
export type LinkType = "related" | "blockedBy" | "subtaskOf";

/**
 * One goal.
 */
export type Milestone = { 
/**
 * A short handle — `slice`, `p2` — that the gate command and the prompts name it by.
 */
id: string, title: string, 
/**
 * The task that holds this goal on the board. Work towards it is `subtaskOf` it.
 */
task?: TaskId, 
/**
 * Shell command, run with `sh -c` in the project root. Exit 0 is met.
 */
gate: string, 
/**
 * Seconds. `number` on the wire, not ts-rs's `bigint`: a day is 86,400 and a JavaScript
 * number holds that exactly, while a `bigint` would make every reader cast.
 */
timeoutSecs?: number, };

/**
 * A project's milestones, as `.cide/config.json` holds them and the Milestones tab edits them.
 *
 * Every field defaults, so a hand-written `{"milestones": {"items": [...]}}` is a complete file —
 * and, more importantly, so a shape this build does not recognise costs only this key: the loader
 * reads `milestones` separately from `agents`, because a parse failure in `agents` disables
 * subagents for the whole project, and a typo in a gate command must not do that.
 */
export type MilestonePlan = { 
/**
 * In order. The first one not yet accepted is where the project is, unless [`Self::active`]
 * says otherwise.
 */
items: Array<Milestone>, 
/**
 * The milestone being worked on, by id. `None` with items present means the first one.
 */
active?: string, 
/**
 * Run in a run's worktree before its work is integrated; exit 0 is *done*. Empty is none.
 *
 * A role saying it ran the checks is a report; this is the checks. It is what makes *review*
 * mean the work passes, for every harness alike — no harness hook is involved, because codex
 * and opencode have none cide may write.
 */
verify: string, 
/**
 * How many open tasks (todo, doing, review) the active milestone may hold before a new one
 * the orchestrator creates goes to the inbox instead. `None` is [`DEFAULT_MAX_OPEN`].
 */
maxOpen?: number, 
/**
 * Paths (a file, or a directory with a trailing `/`) relative to the project root that a
 * gate reads. A branch that touches one is refused at integration: the work may not move the
 * goal it is measured against.
 */
guardPaths: Array<string>, };

/**
 * One task under a milestone: enough to draw a row and open the card. Outbound.
 */
export type MilestoneTask = { id: TaskId, title: string, status: TaskStatus, agent?: AgentId, 
/**
 * 0 for a direct subtask of the milestone's task, 1 for its subtasks, and so on.
 */
depth: number, };

/**
 * One milestone's tasks, as a tree flattened in board order. Outbound.
 */
export type MilestoneTasks = { milestone: string, tasks: Array<MilestoneTask>, };

/**
 * Everything the Milestones tab of the Tasks panel draws about a project's milestones. Outbound.
 */
export type MilestonesView = { project: ProjectId, plan: MilestonePlan, 
/**
 * One per milestone that has ever been run here, in [`MilestonePlan::items`] order.
 */
gates: Array<GateState>, 
/**
 * Ids of milestones the user accepted — their task is done.
 */
accepted: Array<string>, 
/**
 * The work under each milestone, one entry per milestone in [`MilestonePlan::items`] order —
 * every task that is `subtaskOf` its task at any depth, done ones included so the tab can
 * count them. (M83)
 */
tasks: Array<MilestoneTasks>, 
/**
 * Verify on each task's branch that this cide has run or is running, newest first. (M83) The
 * board and the card draw it on the task, so a run's work being checked is visible where the
 * work is rather than only as a merge that has not happened yet.
 */
verifies: Array<VerifyState>, 
/**
 * Changes agents have proposed to the milestones or to guarded files, oldest first, waiting
 * for the user to accept or reject them. (M83)
 */
proposals: Array<Proposal>, };

/**
 * What a program has asked for by way of mouse reports. Mirrors `vt100`'s two modes, flattened
 * to what a sender of a **wheel** needs: whether to send anything, and in which encoding.
 */
export type MouseReporting = "off" | "legacy" | "sgr";

/**
 * One leaf in a tab's pane tree.
 */
export type PaneId = string;

/**
 * What a pane is showing.
 */
export type PaneKind = "claude" | "shell" | "diff" | "editor";

/**
 * Whether a pane may be closed.
 *
 * The pinned Claude tab's first pane is `Primary`: closing *or detaching* it returns
 * `Err(PanePrimary)` however many panes the tab holds, so the project console can never
 * lose its conversation — not by accident, and not by splitting first.
 */
export type PaneRole = "primary" | "auxiliary";

/**
 * A permission prompt, as a device is shown it.
 *
 * Produced by `cide_claude::permission::parse` and carried here, rather than declared in that
 * crate and converted: `cide-ipc` is where a wire type lives, and a domain struct plus a wire
 * struct plus a `From` between them is three places for a field to go missing.
 */
export type PermissionPrompt = { 
/**
 * The lines above the options, in order, with the box drawing taken off.
 */
question: Array<string>, options: Array<PromptOption>, 
/**
 * Which option the TUI has highlighted, when it marks one.
 */
selected?: number, 
/**
 * A hash of the normalised block, and **the guard on every answer**.
 *
 * Over a network the prompt a person tapped can already have been replaced by the *next*
 * one. Without this, a tap on *"No, tell Claude what to do differently"* lands as *"1. Yes"*
 * on a prompt that arrived 300 ms later, and approves whatever it was asking about. The
 * answer is refused when this has moved.
 */
digest: string, };

/**
 * One opened project (a header tab, or a window in `PerProject` mode).
 */
export type ProjectId = string;

/**
 * One numbered choice.
 */
export type PromptOption = { number: number, label: string, };

/**
 * One proposed change, waiting for the user. Outbound.
 */
export type Proposal = { 
/**
 * `p-1`, `p-2`, … — per project, never reused.
 */
id: string, title: string, 
/**
 * Why, in markdown, as the agent wrote it.
 */
rationale: string, by: TaskAuthor, createdUnixMs: number, 
/**
 * The task it came out of, if any; accepting or rejecting it is noted there.
 */
task?: TaskId, change: ProposalChange, };

/**
 * What accepting a proposal does.
 */
export type ProposalChange = { "kind": "plan", plan: MilestonePlan, before: MilestonePlan, } | { "kind": "files", files: Array<ProposedFile>, } | { "kind": "note" };

/**
 * One file in a [`ProposalChange::Files`].
 */
export type ProposedFile = { 
/**
 * Relative to the project root, `/`-separated.
 */
path: string, 
/**
 * The new content; `None` deletes the file.
 */
content?: string, 
/**
 * The content when it was proposed (`None`: the file did not exist). Accepting refuses if
 * the file no longer says this — the proposal was written against something else.
 */
before?: string, 
/**
 * A unified diff of `before` → `content`, for reading.
 */
diff: string, };

/**
 * A role, as a device lists it. (M75)
 *
 * A projection of [`crate::AgentDef`] rather than the thing itself, and the field it leaves out
 * is the reason it exists: `system_prompt` is the role's entire body, kilobytes apiece, and a
 * roster of a dozen roles would put the lot on a phone to render a list of names. That is the
 * board's lesson (`TaskRow` against the tracker's whole contents) arriving a second time, and
 * the cheapest moment to apply it is before anything ships.
 *
 * It carries no model **credentials** — only the model *name*, which is what the row says. The
 * keys live in `LlmProvider`, which has a hand-written `Debug` so a log cannot print one, and
 * nothing in `cide-remote` may name that type.
 */
export type RemoteAgent = { id: AgentId, label: string, scope: AgentScope, harness: Harness, description: string, 
/**
 * The model a run of this role would use, when the definition names one.
 */
model?: string, 
/**
 * The hue the definition **declared**, when it declared one. (M76)
 *
 * The name (`blue`, `cyan`, …) and never a resolved colour, because the two ends paint from
 * different palettes: cide reads `var(--agent-blue)` out of whichever theme is on, and a
 * phone has no such thing. A hex here would be one theme's answer shipped to a device that
 * is always dark.
 *
 * `None` is not *no colour*: a role without a declared one is coloured from a hash of its
 * **id**, which both ends compute identically and neither has to store. So this carries the
 * one fact a device cannot derive, and the derivation stays where it was.
 */
color?: string, 
/**
 * Why this role cannot be dispatched, when it cannot. A sentence, already written for a
 * person — the panel shows it and so should a device.
 */
unavailable?: string, 
/**
 * How many runs of this role may be in flight at once, after local overrides.
 */
maxConcurrent: number, worktree: boolean, 
/**
 * How many runs of this role are alive, and how many are waiting for a slot.
 *
 * Counted **here**, by the one function that already answers it for cide's own panel, and
 * never re-derived on the device from the runs list. A count is only ever visibly wrong
 * beside the list it counts, and two producers give two plausible answers — `TaskRow`'s
 * rule, which is in this file for the same reason.
 */
running: number, queued: number, };

/**
 * A project, as a device lists it.
 */
export type RemoteProject = { id: ProjectId, name: string, 
/**
 * The primary root in display form, e.g. `~/work/cide`.
 */
displayPath: string, 
/**
 * The CSS colour of the header tab's dot.
 *
 * Carried so a device can colour a project the way cide does, which is the cheapest way to
 * make two surfaces onto one workspace feel like one thing. A device that cannot parse
 * `var(--accent)` ignores it; it is a hint, never a requirement.
 */
dot: string, };

/**
 * A live session, and the pane it is drawn in.
 */
export type RemoteSession = { session: SessionId, project: ProjectId, pane: PaneId, 
/**
 * The tab that owns the pane, absent when the pane is **detached** — torn out into an OS
 * window of its own, and therefore in no tab's tree.
 *
 * Absent is not an error and not a missing lookup. It is a state a pane is genuinely in,
 * and the reason `cide_core::workspace::session_panes` exists: the walk it replaced went
 * over tabs alone and reported a detached pane's running `claude` as nothing at all.
 */
tab?: TabId, 
/**
 * The tab's own label, when the pane is in one. (M75)
 *
 * The name a person recognises. A pane's `title` is what the pane calls itself and is
 * `claude` for every Claude console on the machine, so a device listing consoles drew a
 * column of identical rows; the tab is what tells them apart, and it is the same string
 * the workspace tab strip draws.
 */
tabTitle?: string, title: string, kind: PaneKind, role: PaneRole, state: SessionState, 
/**
 * Whether this session is in the *finished and not yet looked at* set.
 *
 * Reported here as well as in the authoritative awaiting frame so a freshly connected
 * device paints a correct list from its first snapshot, without having to join two answers
 * that were taken at different instants.
 */
awaiting: boolean, 
/**
 * The agent run that owns this session, when one does.
 */
run?: RunId, 
/**
 * The role a run is running under, e.g. `reviewer`. Absent for a session a person started.
 */
agent?: AgentId, 
/**
 * The task a run is working on, when it has one.
 */
task?: TaskId, };

/**
 * One dispatched subagent execution. (M18)
 *
 * A uuid because **cide mints it and no human ever types it** — the exact opposite of
 * [`AgentId`] and [`TaskId`] below, which are short strings precisely because people and
 * models do.
 *
 * # Why this is not a [`SessionId`]
 *
 * A run reuses the whole session machinery — `SpawnSpec` → `PtySession::spawn` →
 * `SessionRegistry::insert` — so the temptation to key it by the session it will own is
 * real, and it is wrong at both ends of the run's life.
 *
 * A run exists **before** its child does: it is dispatched into a queue, where it already
 * has to be listable, cancellable and attributable to a task. Keyed by its session, a
 * queued run would have no identity at all — nothing for the panel to draw a row for and
 * nothing for a cancel to name.
 *
 * And a run goes on meaning something **after** the child dies: its exit code, the task it
 * was dispatched against, whether its turn was frozen mid-flight. The session registry
 * reaps a dead child, so a session-keyed run would vanish the moment it finished, taking
 * the one row a user actually wants to read — the one that just failed — with it.
 *
 * So `AgentRun` carries both: this id for the run's whole life, and
 * `session: Option<SessionId>` for the part of it that has a process.
 */
export type RunId = string;

/**
 * Where cide announces a run's turn endings — handed back, finished, failed. (M40)
 *
 * # Why this is recorded on the run and not looked up at the edge
 *
 * The nudge is a line typed into somebody's conversation (`agent_rpc::note_run_over`'s header
 * says why it is a PTY write at all), and *whose* conversation used to be a fixed answer: the
 * project's primary pane. That was wrong twice over once a second Claude pane could start a run —
 * by assigning a role, and since M40 by dispatching — because the pane that asked never heard,
 * and the pane that did hear had not asked. The choice belongs to the moment of dispatch, so it
 * is stamped there, rides the run through the snapshot, and is read at the edge.
 *
 * # Why `Primary` is resolved late
 *
 * `Project::primary_session` moves whenever the console pane binds a new conversation
 * (`workspace::bind_session` rewrites it on every restart), so a *session id* captured at
 * dispatch would name a conversation that no longer exists by the time a long run ends.
 * `Primary` is therefore a role, resolved when the line is typed; [`Self::Session`] is the
 * specific pane that asked, and falls back to `Primary` if that pane is gone.
 */
export type RunNotify = { "kind": "primary" } | { "kind": "session", session: SessionId, } | { "kind": "silent" };

/**
 * Where one run is.
 *
 * # Deliberately shaped like [`crate::SessionState`], and deliberately not that type
 *
 * A run *is* a session plus a queue position, so the resemblance is not accidental and the
 * vocabulary is kept aligned on purpose — `Running` here is `Busy` there, `AwaitingPermission`
 * is the same fact under the same name, and `Finished { code }` is `Exited { code }`. Both are
 * driven by the same hook frames through `cide_claude::next_state`.
 *
 * The one place the vocabularies deliberately *diverge* is [`Self::Idle`], which collapses
 * `SessionState`'s `Idle` and `AwaitingInput` into a single arm. Its doc says why, at the
 * length the question deserves, because it is the kind of asymmetry that gets tidied away.
 *
 * They are still two enums, because **three of these states exist before or outside a child
 * process**: `Queued` has no process yet, `Paused` describes a process that is frozen rather
 * than doing anything, and `Failed` covers the run that never started — no worktree, no binary,
 * a spawn that returned an error. `SessionState` has no vocabulary for any of them, and it
 * should not: it answers "what is this terminal doing".
 *
 * Two enums describing overlapping facts is a real cost and it is taken knowingly. The
 * alternative was adding `Queued`/`Paused` to `SessionState`, and *every* existing consumer
 * would then have to grow an arm for a state it can never see — `awaitingRule.ts`,
 * `exitMarker.ts`, `restartRule.ts`, `paneHosts::setHostBusy` — which is four places to get a
 * subagent-only concept wrong in a pane that has nothing to do with subagents. (`Paused` does
 * reach `SessionState`, because a paused *session* is a fact a pane must draw; what does not
 * reach it is the queue.)
 */
export type RunState = { "state": "queued" } | { "state": "starting" } | { "state": "running" } | { "state": "idle" } | { "state": "awaitingPermission" } | { "state": "paused", sinceUnixMs: bigint, } | { "state": "interrupted" } | { "state": "finished", code: number, } | { "state": "failed", reason: string, };

/**
 * One cell colour, or the terminal's default when absent.
 *
 * An index is kept as an index and never resolved to RGB here. The palette belongs to whoever
 * paints: cide's own editor colours come from `crate::theme`, and a phone reading this over a
 * network is entitled to a different, higher-contrast palette for a five-inch screen. Resolving
 * on this side would take that choice away and make every capture carry one machine's theme.
 */
export type ScreenColor = { "kind": "idx", index: number, } | { "kind": "rgb", r: number, g: number, b: number, };

/**
 * Everything about a grid that is not its contents.
 *
 * Split out of [`ScreenCapture`] because an incremental update carries it too — a consumer must
 * be able to tell a repaint of the same grid from a *different* grid — and two structs each
 * spelling out five fields is two places for them to disagree about what a screen is.
 */
export type ScreenInfo = { cols: number, rows: number, 
/**
 * The alternate screen is engaged — a fullscreen TUI is drawing.
 *
 * Consumers need it for three separate decisions: there is no scrollback to page through,
 * re-wrapping would destroy a layout drawn in columns, and a transition either way is a
 * change of grid rather than of contents, so a diff across one is meaningless.
 */
alt: boolean, 
/**
 * DECCKM: the cursor keys must be encoded `SS3 A` rather than `CSI A`.
 *
 * On the capture because it is the only thing that knows, and because a caller encoding a
 * keystroke from a phone has no other way to find out. Guessing it is how an arrow key
 * arrives in a TUI as a literal `A`.
 */
appCursor: boolean, 
/**
 * The program asked for bracketed paste, so pasted text must be wrapped in `CSI 200~` /
 * `CSI 201~` and a caller must not wrap it when it did not.
 */
bracketedPaste: boolean, 
/**
 * How the program wants the mouse reported, `None` when it has not asked at all. (M76)
 *
 * On the capture for [`Self::app_cursor`]'s reason, and the consequence of guessing it is
 * worse here than there. A program that never enabled mouse reporting reads whatever a
 * wheel would have encoded as **typed input**: scrolling a phone would put
 * `[<64;40;12M` on a shell's command line. So a caller with a wheel to send has to be told,
 * and `cide_remote::keys::wheel` refuses rather than guessing.
 *
 * The *encoding* rides with the mode because the two are set by separate escape sequences
 * and a program may enable tracking without asking for SGR — in which case the report is
 * the original single-byte form, which cannot describe a column past 223.
 */
mouse: MouseReporting, };

/**
 * One row of the grid.
 */
export type ScreenLine = { row: number, 
/**
 * The line continues into the next one because the text ran off the edge, rather than
 * because the program asked for a new line.
 *
 * A consumer narrower than the capture needs this to re-wrap without inventing breaks the
 * program never wrote: a soft-wrapped paragraph may be re-flowed and two separate lines
 * may not.
 */
wrapped?: boolean, runs: Array<StyleRun>, };

/**
 * A repaint of a watched session, whole or partial.
 *
 * **Only the lines that changed**, unless `full`. A device holds the grid it was last sent and
 * applies each update onto it; that is what makes an active terminal a few hundred bytes a tick
 * instead of a screenful, and it is the reason the sender keeps a digest per line rather than
 * asking the device what it has.
 *
 * `epoch` is the grid's identity. It changes when [`ScreenInfo`] does — a resize, or the
 * alternate screen going in or out — and a change of epoch always arrives with `full`, because
 * a diff across two different grids is not a diff. A device that receives an unfamiliar epoch
 * throws its cache away.
 */
export type ScreenUpdate = { session: SessionId, epoch: number, 
/**
 * `lines` is the whole grid rather than the part of it that moved.
 */
full: boolean, info: ScreenInfo, 
/**
 * `None` when the cursor is hidden.
 */
cursor?: Cursor, lines: Array<ScreenLine>, };

/**
 * A window onto the retained scrollback, counted from the oldest line.
 *
 * `from_top` and `depth` rather than "the last N lines" because the scrollback grows from the
 * bottom while a page is being read: an offset from the end names a different line every time
 * the child prints anything, and a reader paging backwards would see rows repeat and rows
 * vanish with nothing wrong anywhere.
 */
export type ScrollbackCapture = { fromTop: number, 
/**
 * How many lines are retained in total, so a reader can size a scrollbar and know when it
 * has reached the beginning.
 */
depth: number, lines: Array<ScreenLine>, };

/**
 * What cide may say.
 */
export type ServerBody = { "t": "welcome", protocol: number, instance: InstanceInfo, 
/**
 * Capability names, for a device that wants to ask *can you* rather than *are you new
 * enough*. See [`PROTOCOL_VERSION`].
 */
features: Array<string>, } | { "t": "paired", device: string, key: string, 
/**
 * Which cide this is, and what it calls itself.
 *
 * Sent here because a device that paired by a **typed address** has never been told
 * either: the QR carries both, and the typed road has only an address and eight
 * characters. Without this the app would have to invent an instance id, and a
 * placeholder shared by every typed pairing makes two machines look like one — so the
 * list would show one entry that connects to whichever answered last.
 */
instance: string, label: string, } | { "t": "projects", rev: number, projects: Array<RemoteProject>, } | { "t": "sessions", sessions: Array<RemoteSession>, } | { "t": "sessionState", session: SessionId, state: SessionState, } | { "t": "awaiting", entries: Array<AwaitingEntry>, } | { "t": "error", kind: string, detail: string, } | { "t": "prompt", session: SessionId, prompt: PermissionPrompt, } | { "t": "promptGone", session: SessionId, } | { "t": "dispatched", run: RunId, } | { "t": "screen", update: ScreenUpdate, } | { "t": "scrollback", session: SessionId, page: ScrollbackCapture, } | { "t": "screenGone", session: SessionId, } | { "t": "runs", project: ProjectId, runs: Array<AgentRun>, } | { "t": "roster", project: ProjectId, agents: Array<RemoteAgent>, 
/**
 * Whether the queue will start anything new.
 *
 * **Distinct from "every run is paused", and carrying only the second was a bug.**
 * Pausing a project does two things — it shuts the dispatch queue *and* freezes the
 * children — so a project paused while nothing happened to be running looked, on a
 * device, exactly like an idle one. `AgentRoster::Ready`'s own field says this at
 * length; a device is owed the same fact for the same reason, because it has the same
 * Resume button and no way to tell which of the two it is about to undo.
 */
dispatching: boolean, } | { "t": "task", project: ProjectId, id: TaskId, 
/**
 * Boxed, because a `TaskDetail` carries a body, every comment, every attachment
 * record and the whole status history — and an enum is as large as its largest
 * variant, so inlining it would make every `Pong` and every `SessionState` carry that
 * much stack. Serde and ts-rs both see straight through a `Box`, so the wire and the
 * TypeScript are unchanged.
 */
task?: TaskDetail, } | { "t": "board", project: ProjectId, tasks: Array<TaskRow>, } | { "t": "milestones", project: ProjectId, view?: MilestonesView, } | { "t": "checkLog", project: ProjectId, kind: string, key: string, text?: string, } | { "t": "desync", why: string, } | { "t": "pong" } | { "t": "goingAway", why: string, };

/**
 * One frame to a device.
 */
export type ServerFrame = { 
/**
 * The `id` of the frame this answers, absent for anything the server said on its own.
 */
id?: number, body: ServerBody, };

/**
 * A PTY-backed session. For Claude panes this is the `--session-id` uuid.
 */
export type SessionId = string;

/**
 * Lifecycle of a session, driven by Claude Code hooks with a PTY-quiet fallback.
 *
 * "Live session" for the close-confirm setting means `Busy | AwaitingPermission | Paused` —
 * not "the process exists", which would warn constantly. See [`SessionState::is_live`].
 */
export type SessionState = { "state": "spawning" } | { "state": "splash" } | { "state": "idle" } | { "state": "busy" } | { "state": "awaitingPermission" } | { "state": "awaitingInput" } | { "state": "paused" } | { "state": "exited", code: number, };

/**
 * A span of adjacent cells sharing one appearance.
 */
export type StyleRun = { text: string, 
/**
 * `None` is the terminal's default foreground.
 */
fg?: ScreenColor, 
/**
 * `None` is the terminal's default background.
 */
bg?: ScreenColor, 
/**
 * A mask of [`flags`]. Absent means none of them.
 */
flags?: number, };

/**
 * One workspace tab inside a project.
 */
export type TabId = string;

/**
 * One file attached to a task's body or to a comment. (M39)
 *
 * **Metadata only.** The bytes are a file on disk at [`Self::relative_path`], copied there by
 * `cide_tasks::attachments::import`; this record is what travels in `.cide/tasks.json`, on the
 * wire and in an agent's `cide_task_get`. A record that carried bytes would put a screenshot
 * through Tauri's JSON IPC as an array of decimal numbers (`crate::image`'s header) and into a
 * file whose diffs people read.
 *
 * Immutable once written, except for [`Self::deleted`]. There is no rename and no replace: a
 * person who wants a different file attaches a different file, and that is what keeps the merge
 * rule to one line — see `cide_tasks::union_attachments`.
 */
export type TaskAttachment = { id: TaskAttachmentId, 
/**
 * The file's name as a person sees it, and the last component of its path on disk.
 *
 * The original name, sanitised to one path component (no separators, no NUL, not empty),
 * kept verbatim otherwise — the OS opener picks an application by the real extension, and
 * a person recognises `design-v3.png`, not a uuid. Each attachment has a directory of its own
 * named by its id, which is what lets two attachments on one task share a name.
 */
name: string, bytes: bigint, kind: AttachmentKind, 
/**
 * Who attached it. From the connection, never from the payload — [`TaskComment`]'s rule.
 */
addedBy: TaskAuthor, addedUnixMs: bigint, 
/**
 * A tombstone, on [`TaskComment::deleted`]'s exact terms: one-way, kept in the vector so a
 * merge with a stale file cannot resurrect the record, and the merge rule is "either side
 * deleted ⇒ deleted". The bytes are removed when this is set; the record stays.
 */
deleted: boolean, };

/**
 * A file attached to a task or to a comment. (M39)
 *
 * A **string** like [`CommentId`], and a fresh v4 uuid for every record, but with none of the
 * `legacy-` story: no task file written before attachments existed carries one, so there is
 * nothing to derive and no `Default` for a repairing loader to fill. Hence no `Default` and no
 * `is_empty` — a `TaskAttachmentId` that reached the store is a real uuid or the record is dropped.
 *
 * It is also a **path component**: the bytes live at
 * `.cide/attachments/<task>/<attachment>/<name>`, and that is why `cide_tasks::repair` must never
 * re-mint one the way it re-mints a malformed task or comment id. A comment id is only a merge
 * key; re-minting an attachment id would leave the file on disk under the old one, orphaned with
 * no symptom but a broken thumbnail.
 */
export type TaskAttachmentId = string;

/**
 * Who wrote a comment.
 *
 * A **tagged enum rather than a bare author string**, so the panel can style the user's own
 * lines without string-matching a name that a config file is free to choose. With a string,
 * "is this mine" would be `author === 'you'` against a value an agent definition could legally
 * claim, and the styling would be wrong in the one case it matters: a user reading back a
 * conversation to work out which half of it they said.
 *
 * [`Self::Agent`] carries `label` beside `agent` for the reason [`crate::AgentRun::agent_label`]
 * states one layer up: the label is copied at write time, so a comment written by `developer`
 * still reads correctly after the role has been renamed. A comment that renamed its own author
 * when a config file changed would be a record that lies about what happened.
 */
export type TaskAuthor = { "kind": "user" } | { "kind": "orchestrator" } | { "kind": "agent", agent: AgentId, label: string, };

/**
 * One line of a task's log.
 *
 * # Append-only for **agents**. The user may edit and delete. (M21)
 *
 * It was append-only for everyone, and that argument is still the right one where it bites: an
 * agent that can quietly rewrite a comment after the fact leaves neither the user reading the
 * panel nor the next agent reading the task any way to tell that what they are acting on is not
 * what was written.
 *
 * What it never justified was stopping the person at the keyboard from fixing their own typo or
 * removing a comment that should not be there. **The boundary is the caller, not the comment's
 * author**: `TasksStore::edit` refuses [`TaskEdit::EditComment`] and [`TaskEdit::DeleteComment`]
 * for every author but [`TaskAuthor::User`], and no MCP tool exposes either — the arrangement
 * `TasksStore::remove` already had, for the same reason.
 *
 * Three fields carry that, and each is load-bearing rather than decorative — see their own
 * notes: [`Self::id`], [`Self::edited_at_unix_ms`] and [`Self::deleted`].
 */
export type TaskComment = { 
/**
 * Stable identity, and the reason a delete survives a merge.
 *
 * The file is merged with a stale copy of itself by unioning comments, and that union used
 * to be *structural* — `(author, at_unix_ms, text)` were the only three fields there were,
 * so `==` said exactly what the rule said. Under editing that rule is wrong twice: an edit
 * changes `text`, so the stale copy's original re-appears beside the new one, and a delete
 * removes a comment the stale copy still has, so the next merge puts it back.
 *
 * A comment written by this build gets a fresh v4 uuid. A comment already in a file gets one
 * **derived from the old merge key** by [`Self::legacy_id`] — deterministic, so two readers
 * of the same file agree, and identical under exactly the conditions structural equality was
 * identical under. That is what makes the change invisible to every task file that already
 * exists rather than a migration.
 */
id: CommentId, author: TaskAuthor, 
/**
 * The text, verbatim. **Markdown**, since M27.
 *
 * This said "never rendered as markup" until M31, and by then it had been wrong for a
 * milestone: `TaskMarkdown.tsx` renders it. The refusal it recorded was about a *road*, not
 * about markup — `string -> HTML string` into `dangerouslySetInnerHTML`, model-authored, in
 * the IDE's own chrome. That road is still not taken. The parser produces no HTML node of
 * any kind, every string reaches the DOM as a React child through React's escaping, and a
 * link is drawn as accented text that activates nothing; `TaskMarkdown.tsx`'s header carries
 * the whole argument and `check:markdown` asserts the parser can never invent such a node.
 *
 * Leaving the stale sentence here was not free. It was mirrored into the MCP input schema
 * (`cide_agents::tools`), so every agent was told in the tool it was about to call that its
 * report would not be formatted — and wrote one flat paragraph, which is exactly what the
 * card then drew. A comment is read by a person; the wording an agent is handed here is the
 * thing that decides whether it is readable.
 *
 * One dialect note: the card parses with `softBreak: 'break'`, so a lone newline is a line.
 * A comment is a message, not a `.md` file — the markdown *preview* keeps CommonMark.
 */
text: string, 
/**
 * Milliseconds since the Unix epoch, stamped by Rust when the comment lands.
 *
 * Written where the mutation is applied rather than taken from the caller, for
 * [`crate::ViewPosition::touched_at`]'s reason: a timestamp supplied by whoever is writing
 * is a timestamp an agent can get wrong, and this one orders the log.
 */
atUnixMs: bigint, 
/**
 * When the user last edited it, or `None` for a comment that stands as written.
 *
 * Drawn by the panel, and that is the point rather than a nicety. The user editing an
 * *agent's* comment is the one case left that could mislead a later reader — the whole
 * hazard the append-only rule existed to prevent, with the user in the agent's place — and a
 * visible "edited" mark is what keeps the log honest about it.
 *
 * It also decides the merge: for one id, the copy with the newer edit wins. A comment that
 * was never edited sorts as its `at_unix_ms`, so an edited copy always beats an untouched
 * one no matter which file it came from.
 */
editedAtUnixMs: bigint | null, 
/**
 * A tombstone. The comment is gone from the panel and from every agent's view of the task.
 *
 * Kept as a flag rather than removed from the vector, because a removed comment is one the
 * next merge with a stale file puts straight back. `deleted` only ever goes false → true, so
 * the merge rule is "either side deleted ⇒ deleted" and no ordering question arises.
 *
 * The text is **cleared** when this is set. The tombstone has to persist; the words do not,
 * and a deleted comment whose content sat in `.cide/tasks.json` for ever would be a delete
 * that did not delete.
 */
deleted: boolean, 
/**
 * Files attached to this comment, oldest first. (M39)
 *
 * `#[serde(default)]` for [`Task::links`]' reason and with the same non-bump of
 * [`TaskFile::CURRENT_SCHEMA`]. Records are tombstoned, never removed — see
 * [`TaskAttachment::deleted`]. A deleted comment keeps its attachment *records* (the
 * tombstones have to survive the merge) but `cide_tasks` removes their bytes.
 */
attachments: Array<TaskAttachment>, };

/**
 * One whole task, as `task_get` answers it: its [`TaskRow`] and everything the row leaves out.
 * (M68)
 *
 * # Why the row is nested rather than flattened in
 *
 * Because the two counts have exactly one producer, `cide_tasks::row_of`, and they must keep it.
 * A shape that inlined the row's fields beside the content ones would have to either carry the
 * counts again — a second place deriving a number, where both copies are plausible and a wrong one
 * looks exactly like a right one — or omit them, which pushes the derivation into the *frontend*
 * adapter and puts the same second copy one layer further away.
 *
 * Nesting keeps it to one: whoever builds this asks `row_of` for the row, and every consumer,
 * wire or webview, spreads that answer.
 *
 * It also happens to be the shape the storage split is heading for — a row from the index joined
 * to a content file — so this is the join, written down once while both halves still live in one
 * file.
 */
export type TaskDetail = { row: TaskRow, 
/**
 * The whole statement of the work, possibly empty — [`Task::body`]'s posture verbatim.
 */
body: string, comments: Array<TaskComment>, 
/**
 * The body's files. A comment's ride in its own [`TaskComment::attachments`].
 */
attachments: Array<TaskAttachment>, history: Array<TaskStatusChange>, };

/**
 * One change to one task. Inbound.
 *
 * # Why an enum and not a `TaskPatch` of options
 *
 * [`crate::SettingsPatch`] is a struct of `Option`s and works, so this shape needs a reason.
 * It is one field: **unassigning**.
 *
 * In a patch struct, `agent: None` means "not mentioned, leave it alone". Expressing "set it to
 * nobody" therefore needs `Option<Option<AgentId>>` — a shape that serialises as
 * absent-versus-null, that `deny_unknown_fields` cannot police (both are well-formed), that
 * reads identically in the TypeScript type, and that every caller gets wrong exactly once
 * before discovering the difference. `SettingsPatch` gets away with per-field `Option` because
 * none of its fields is *itself* nullable; [`Task::agent`] is.
 *
 * A variant per edit also makes the mutation the caller intended legible at the seam, which
 * matters more here than for settings: these calls come from MCP tools a language model fills
 * in, and a five-variant enum with one or two fields each is called correctly far more often
 * than one nine-field object.
 *
 * # There is deliberately no `Reorder`
 *
 * The array is the order and reordering it is a real gesture — but the panel has no drag
 * affordance in v1, so a `Reorder` variant would be a wire shape no caller reaches. That is the
 * dead-control failure wearing a different hat, and this repository has already deleted one
 * instance of it: [`crate::SplitIntent`]'s removed `Diff` variant, which existed, was reachable
 * from no code path, and whose handling arm would have minted a pane showing nothing. Add it
 * with the gesture, in the same commit.
 */
export type TaskEdit = { "kind": "setTitle", title: string, } | { "kind": "setBody", body: string, } | { "kind": "setStatus", status: TaskStatus, } | { "kind": "assign", agent: AgentId | null, } | { "kind": "setSession", session: SessionId | null, } | { "kind": "setChange", change: ChangeName | null, } | { "kind": "link", link: LinkType, target: TaskId, } | { "kind": "unlink", link: LinkType, target: TaskId, } | { "kind": "comment", text: string, } | { "kind": "editComment", id: CommentId, text: string, } | { "kind": "deleteComment", id: CommentId, } | { "kind": "detachAttachment", attachment: TaskAttachmentId, };

/**
 * One task in `.cide/tasks.json`, as a short string — `t-17`. (M18)
 *
 * The shortness is the design, not a saving. **Agents quote task ids inside prompts and
 * comments**, and that is a position where a uuid fails three separate ways: it costs tokens
 * on every mention; it gets truncated or a digit transposed by a model that is *paraphrasing*
 * a task rather than copying an identifier; and once mangled it cannot be matched back to
 * anything, so the reference is silently lost with no error raised anywhere. `t-17` survives
 * all three, and a human reading the file in a diff can hold it in their head while they read
 * the next hunk.
 *
 * Minted by Rust from the **file's own high-water mark**, never from the length of the list.
 * The two agree only until something is deleted, and after that a length-derived id recycles:
 * a second `t-17` would silently re-point every comment, every prompt and every commit message
 * that ever named the first one. So an id is stable for the life of the file and is never
 * reused, which is also what lets a task be quoted somewhere cide cannot see.
 */
export type TaskId = string;

/**
 * One stored edge between two tasks, on the source task only. (M30)
 *
 * Unlike a [`TaskStatusChange`], an edge is **mutable** — unlink exists — so it carries the two
 * fields that let it survive the out-of-process merge: a tombstone and a stamp. There is
 * deliberately **no `LinkId`**: a comment needed a uuid because edits change its text while its
 * identity must persist, but an edge *is* its `(link, target)` pair — there is nothing else to
 * it — so the pair is the merge identity and a minted id would be a second name for the same
 * fact, with all of [`TaskFile::tasks`]' two-identities hazard.
 */
export type TaskLink = { link: LinkType, 
/**
 * The other task. Never validated against existence *here* — `cide-tasks` refuses a target
 * that does not exist at the gesture, but a target deleted afterwards leaves the edge
 * dangling and legal, because ids are never reused (see [`TaskId`]) and so a dangling edge
 * can never silently come to mean new work. Renderers mark it; nothing prunes it.
 */
target: TaskId, 
/**
 * A tombstone — but unlike [`TaskComment::deleted`] it is a **toggle**, not one-way.
 * A deleted comment is replaced by writing a new comment under a new id; re-linking the
 * same `(link, target)` pair recreates the *same* key, so "deleted wins for ever" would
 * make an unlink permanent after any merge. The stamp below is what resolves the toggle;
 * `cide_tasks::union_links` carries the argument.
 */
deleted: boolean, 
/**
 * Stamped by Rust when the link or unlink lands, for [`TaskComment::at_unix_ms`]'s reason —
 * and load-bearing beyond display: per `(link, target)` key, the merge keeps the copy with
 * the newer stamp.
 */
atUnixMs: bigint, };

/**
 * One edge named at task creation. Inbound. (M30)
 *
 * No tombstone and no stamp — those are the store's to write, for the same reason a comment's
 * timestamp is: a stamp supplied by whoever is writing is a stamp an agent can get wrong, and
 * this one decides a merge.
 */
export type TaskLinkSpec = { link: LinkType, target: TaskId, };

/**
 * A new task. Inbound.
 */
export type TaskNew = { project: ProjectId, 
/**
 * The one required field. A task with no title is a row nobody can act on.
 */
title: string, 
/**
 * Absent means empty. [`Task::body`] is not nullable; this is, only so the common call —
 * the orchestrator creating five titles at once — need not send five empty strings.
 */
body?: string, 
/**
 * The role this is for, if it is known at creation. Absent means unassigned.
 */
agent?: AgentId, 
/**
 * Where it starts. Absent means [`TaskStatus::Todo`], which is what every caller but the
 * compose dialog sends. (M21)
 *
 * # This field is the user's, and structurally not an agent's
 *
 * [`TaskStore::create`](../../cide_tasks/struct.TaskStore.html#method.create) used to say
 * there was deliberately no wire shape for this, because *"a task created directly into
 * `Done` is a status nobody moved it to"*. That worry is about **provenance**, and it is
 * still right about the caller it was written for: an agent minting finished work would
 * leave a tracker whose statuses record nothing.
 *
 * It does not apply to a person filling in a dialog with the four states in front of them
 * and pressing Create — *I am starting this now* is an ordinary thing to say, and the old
 * shape made them say it in two writes instead of one. The restriction that matters is
 * kept, and kept **structurally**: `cide_agents::tools`' create tool has no such argument
 * and `TaskSink::create` has no such parameter, so the MCP path cannot express one. It is
 * not a check that could be forgotten; it is a signature.
 */
status?: TaskStatus, 
/**
 * The OpenSpec change this task implements, if it is known at creation. (M28)
 *
 * Set at creation and not in a follow-up edit, which matters more than it looks: the
 * auto-dispatch trigger reads the task the mutation *left behind*, so a create-then-link
 * would dispatch a run from a task that did not yet name its change, and the run would be
 * told nothing about the checklist it was started for.
 */
change?: ChangeName, 
/**
 * Edges known at creation. Absent means none. (M30)
 *
 * Here for [`Self::change`]'s exact reason, with a sharper edge: a creation naming an
 * assignee dispatches, and the trigger reads the task the mutation left behind — so
 * create-then-link would dispatch a run from a task whose `blockedBy` did not exist yet,
 * and the one interleaving the gate exists for is the one it could never see.
 */
links?: Array<TaskLinkSpec>, 
/**
 * Files to attach to the body the moment the task exists. Absent means none. (M39)
 *
 * **Source paths**, on this machine, which `cide_tasks::attachments::import` copies under
 * `.cide/attachments/<the new id>/`; a path under its staging directory is consumed. Here
 * rather than as a follow-up `task_attach` for [`Self::links`]' reason in miniature: the
 * board is broadcast once with the attachments in place instead of once without and once
 * with, and a dropped file in the New task dialog is one gesture, not two.
 */
attachments?: Array<string>, };

/**
 * One task as the *board* carries it: everything except its body, its log and its files. (M68)
 *
 * # Why this is a separate type and not `Task` with three keys left out
 *
 * Two reasons, and the second is the one that decides it.
 *
 * `Task::body` and `Task::comments` carry no `#[serde(default)]` — deliberately, because for
 * those two an absent key is not a fact about the task but a truncated document. A `Task` with
 * them omitted is therefore a *hard* deserialize failure, and giving them defaults to make the
 * omission legal would re-open the argument [`TaskAuthor::user`] makes: once absent means
 * `User`, or means "no comments", nothing downstream can tell a task that has none from a
 * payload that did not carry any.
 *
 * And that distinction is exactly what this type exists to keep. The board is index-shaped
 * because `cide://tasks-changed` used to carry every word ever written into a project's tracker
 * — 2.19 MB on a four-month-old board, of which the comments alone were 77.5% — and Tauri
 * delivers an event by inlining the payload into a JavaScript **source string** and `eval`ing it
 * on the GTK main loop (`tauri`'s `event::emit_js_script`). Measured: 10.2 ms to parse that board
 * as JS source against 2.9 ms as JSON, per window, per comment an agent wrote. `TASKS_CHANGED`'s
 * own doc has the rest.
 *
 * So a row is what every consumer of the *list* already needed and nothing more: the panel's row
 * draws a glyph, an id, a title and an agent chip, and every off-panel reader
 * (`openCount`, the link pickers, `taskReveal`, the OpenSpec tab) reads status, id, title or
 * `change`. A whole task is fetched by id when something opens one.
 */
export type TaskRow = { id: TaskId, title: string, status: TaskStatus, 
/**
 * The role this task is **for** — see [`Task::agent`], which this mirrors exactly.
 */
agent: AgentId | null, 
/**
 * `#[ts(optional)]` **paired with `skip_serializing_if`**, unlike [`Task::session`].
 *
 * That pairing is not tidiness. `crate::docker`'s own header spells the failure out: on its
 * own, `#[ts(optional)]` changes the emitted *type* and not what serde writes, so the field
 * the TypeScript calls `undefined` arrives as `null`, `=== undefined` is false, and the next
 * property access throws. `Task` has carried that mismatch since M28 and `adapt.ts` absorbs
 * it with `== null`; a new type is a chance not to add a third instance of it.
 */
session?: SessionId, 
/**
 * `#[ts(optional)]` + `skip_serializing_if`, on [`Self::session`]'s argument.
 */
change?: ChangeName, 
/**
 * This task's own edges, tombstones included — [`Task::links`]' posture verbatim.
 *
 * Links stay on the row rather than moving to the content file with the log, because they
 * are read *across* tasks: `blocks` is derived by scanning every other task's list, and
 * auto-dispatch refuses a blocked task by reading its blockers' statuses. A reader that had
 * to open one file per task to answer "is this one blocked" would be loading the whole board
 * to render a list, which is the cost this type exists to avoid.
 */
links: Array<TaskLink>, createdBy: TaskAuthor, createdUnixMs: bigint, 
/**
 * See [`Task::updated_unix_ms`] — the merge tiebreak and the panel's in-group sort key.
 *
 * **A write to a task's content must bump this**, or a comment stops moving its task's merge
 * rank and its position in the panel. Nothing throws; the board just quietly stops reordering.
 */
updatedUnixMs: bigint, 
/**
 * How many live comments the task has. A **cache**; the content file is the truth.
 *
 * Named `…Count` rather than `comments`, and it is worth the extra word: a field called
 * `comments` that is a number where every other reader expects a vector turns `.length` into
 * `undefined` instead of a type error, and `undefined > 0` is false — so a task with forty
 * comments would render as one with none, in silence. The name makes that a compile error.
 *
 * It is a cache because the count lives in a committed, hand-editable file separate from the
 * comments it counts, so the two can disagree — after a hand edit, a partial merge, or a
 * crash between two writes. `cide_tasks` recomputes it whenever it loads content and takes
 * content's answer, on `repair`'s rule for an attachment record: the file on disk wins over
 * the record that describes it.
 */
commentCount: number, 
/**
 * Live attachments across the body and every live comment, as one number. (M39's count.)
 *
 * A cache, on [`Self::comment_count`]'s terms. `cide_task_list` prints it because "there is a
 * file on this one" changes whether an agent spends a `cide_task_get`.
 */
attachmentCount: number, };

/**
 * Where a task is.
 *
 * # The two variants that were proposed and lost
 *
 * **`Blocked`.** It is a *reason*, not a place. Every tracker that ships it accumulates tasks
 * parked there for weeks with nothing on the row saying what the block was or whether it still
 * holds, because the state itself carries no text and nobody is obliged to add any. A blocked
 * task here is `Todo` with a comment — which is strictly more informative, is a shape an agent
 * can write without being taught a new vocabulary, and is exactly what a failed worktree merge
 * produces (`cide_agent_integrate` comments the conflicting paths and leaves the task where it
 * was).
 *
 * **`Cancelled`.** This file is a working queue, not an archive: a cancelled task is one the
 * user deletes, and `git log -p .cide/tasks.json` already holds the record of it having
 * existed, in more detail than a tombstone row could. A `Cancelled` group would also compete
 * with `Done` for the bottom of the panel, where neither is being read.
 *
 * # The one that was added: `Inbox` (M83)
 *
 * `Inbox` is not `Blocked` under another name. It is not a reason attached to work; it is the
 * statement that something is **not work yet**. Before it existed, "anything noticed in passing
 * goes on the board" meant every defect a run tripped over became a `Todo` — a task autodispatch
 * would start, the spinner counted as open work, and the next planning turn read as part of the
 * plan. Measured on a real board (`~/work/selfcraft`, four months, 232 tasks): of 98 open tasks,
 * 36 were the plan and the rest were things somebody noticed. The board grew faster than it
 * closed, and the orchestrator read the growth as progress.
 *
 * So a thing noticed goes to the inbox, and nothing about the inbox is automatic: autodispatch
 * does not start it (`autodispatch` starts `Todo | Doing` only), the spinner does not count it as
 * open work, and a role assigned to it is not woken. It becomes work by being moved to `Todo` —
 * by the orchestrator when a milestone needs it, or by the user. It never expires: a project left
 * alone for a month has the same inbox when it is opened again, because time passing says nothing
 * about whether an observation was right.
 *
 * It is placed **first** so that the derived `Ord` keeps reading as the life of a task.
 *
 * `Copy` and `Hash` because the panel groups by this and the group order is a lookup table.
 */
export type TaskStatus = "inbox" | "todo" | "doing" | "review" | "done";

/**
 * One status transition, recorded where the mutation is applied. (M27)
 *
 * # Why a structured record rather than a seeded comment
 *
 * The alternative was `TasksStore::edit` appending "moved to Review" lines to the log, which
 * costs nothing on the wire — and it is the design [`Task::created_by`] already tried and
 * reversed, for reasons that all apply here: the panel would have to *parse prose* to draw the
 * transitions apart from conversation, the user can delete comments (so the history would be
 * editable in exactly the way provenance must not be), and every agent reading the task would
 * wade through bookkeeping lines to find the words. A separate vector is invisible to the log,
 * undeletable by design (no [`TaskEdit`] variant touches it), and renders collapsed.
 *
 * `by` is stamped from the connection's identity exactly as a comment's author is — see
 * [`TaskEdit::Comment`] — so an agent cannot record a move as the user. `from` is read off the
 * task at the moment of the change, not supplied: a caller-supplied `from` is one an agent can
 * get wrong, and the pair is what makes each row legible on its own.
 */
export type TaskStatusChange = { from: TaskStatus, to: TaskStatus, by: TaskAuthor, 
/**
 * Stamped by Rust when the change lands, for [`TaskComment::at_unix_ms`]'s reason.
 */
atUnixMs: bigint, };

/**
 * Verify on one task's branch. Outbound.
 */
export type VerifyState = { task: TaskId, agent: AgentId, 
/**
 * It is running now; `last` is then the previous result, if any.
 */
running: boolean, last?: CheckResult, 
/**
 * The full output, as [`GateState::log`].
 */
log?: string, };
