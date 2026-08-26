/**
 * The Tasks panel's pure core: the status vocabulary, the grouping, the status filter, the
 * text search, the rev-drop rule, the gate on an armed delete, and the chip that says which
 * agent is on a task right now. (M18)
 *
 * # The Tasks panel does not depend on subagents being enabled
 *
 * Stated here so nobody "fixes" it. A shared, committed task list is useful on its own — it is
 * a tracker in the repository, readable in a pull request, with `git log -p .cide/tasks.json`
 * as its history — and orchestration is **off by default**, so gating this panel on it would
 * make the first thing a curious user clicks say "turn on a feature you have not read about".
 * Nothing below reads a roster; the one place the two meet is [`agentChip`], and it degrades to
 * "the assigned role, dim" when the runs array is empty, which is a correct rendering rather
 * than a stub.
 *
 * # Why this file imports nothing
 *
 * `ui/scripts/check-agents.mjs` compiles this module — and its twin in `AgentsPanel/` — alone
 * with the TypeScript already in `node_modules` and then imports the emitted JS under node.
 * That works only while the module has no imports at all, not even a type-only one through the
 * `@/*` alias, which a bare `tsc` cannot resolve. `settings/theme.ts`, `chrome/sidebarView.ts`,
 * `chrome/sidebarWidth.ts` and `ProblemsPanel/model.ts` are all import-free for exactly this
 * reason; this project has no JS test runner and the panel must never have to be launched to
 * find out whether it groups a status.
 *
 * So the shapes below are **structural restatements** of `TaskBoard`, `Task`, `TaskComment`,
 * `TaskAuthor` and `TaskStatus` from `ui/src/ipc/generated.ts` rather than those types.
 * `chrome/sidebarWidth.ts` states the same trade for `SidebarSettings` and it is the same one
 * here: `adapt.ts` is the seam allowed to import the generated DTOs, it calls these functions
 * with values built from the real ones, and a field renamed on the Rust side therefore still
 * fails the build — one module further out. What a restatement *cannot* catch on its own is a
 * renamed enum **variant**, so `check-agents.mjs` reads `pub enum TaskStatus` out of
 * `crates/cide-ipc/src/tasks.rs` and pins [`TASK_STATUSES`] against it as a set.
 *
 * It also cannot import `AgentsPanel/model.ts` — that module is import-free for the same reason
 * and importing it would break both. [`RunRef.phase`] is therefore an opaque `string` here and
 * [`WORKING_PHASES`] is a deliberate second copy, pinned against the original by the same
 * check — which is also why the two copies had to be renamed in one change.
 *
 * # `number`, not `bigint`
 *
 * `rev`, `createdUnixMs`, `updatedUnixMs` and `atUnixMs` are `u64` in Rust and therefore
 * **`bigint`** in `generated.ts`. Every one of them is a `number` here, and the `Number(…)`
 * conversion belongs to `adapt.ts` beside the rest of the wire translation. A `Number` holds an
 * exact integer to 2^53 — as Unix milliseconds, the year 287396 — so nothing is lost; and
 * mixing a `bigint` with a `number` in a comparison throws a `TypeError` rather than coercing,
 * which in [`newerBoard`] would mean the store handler throwing on every snapshot.
 */

/* ---------------------------------------------------------------------------- the statuses */

/**
 * Where a task is. Restates `TaskStatus` in `crates/cide-ipc/src/tasks.rs`.
 *
 * Four, and the two that were proposed and lost are recorded there: `Blocked` is a *reason*
 * rather than a place, and `Cancelled` belongs to `git log` rather than to a working queue.
 */
export type TaskStatus = 'todo' | 'doing' | 'review' | 'done'

/**
 * The four statuses, in Rust's declaration order.
 *
 * This is the *vocabulary*, not the display order — see [`GROUP_ORDER`]. `check-agents.mjs`
 * asserts it equals `pub enum TaskStatus`'s variants as a set, so adding one in Rust fails the
 * build until the tables below know it.
 */
export const TASK_STATUSES: readonly TaskStatus[] = ['todo', 'doing', 'review', 'done']

/**
 * Display order, and deliberately **not** [`TASK_STATUSES`]: active first, because the panel
 * answers "what is happening" and a human scanning it wants Doing and Review above the backlog.
 *
 * Done last for the same reason — it is the group that grows without bound and is read least.
 * `check-agents.mjs` asserts this is a *permutation* of `TASK_STATUSES` rather than merely a
 * subset of it: a status the panel never groups is a task the user cannot see, which is a task
 * that has silently left the tracker.
 */
export const GROUP_ORDER: readonly TaskStatus[] = ['doing', 'review', 'todo', 'done']

/**
 * Is this one of the four statuses?
 *
 * Takes `string` rather than `TaskStatus` on purpose: every caller has a value *annotated*
 * `TaskStatus` by a wire type, and the point of the guard is that the annotation is a promise
 * from a JSON file a human can hand-edit rather than a fact.
 *
 * `Array.includes` and never `value in SOME_TABLE`: `in` walks the prototype chain, so
 * `'constructor' in { todo: '○', … }` is `true` and the lookup hands back
 * `Object.prototype.constructor` — a function, which React refuses as a child and which
 * `className` stringifies into the whole source text of `Object`.
 */
export function isTaskStatus(value: string): value is TaskStatus {
  return TASK_STATUSES.includes(value as TaskStatus)
}

/**
 * Colour roles a status can take. Each maps to one token in `TasksPanel.module.css`.
 *
 * `review` gets `attention` rather than a fifth quiet tone because it is the one status that is
 * a request *to the user*: an agent has finished and is waiting to be told whether it was right.
 */
export type Tone = 'idle' | 'busy' | 'attention' | 'done'

/** Every tone a table below may return. Exported so the check can pin the tables to it. */
export const TONES: readonly Tone[] = ['idle', 'busy', 'attention', 'done']

/*
 * The three tables, precisely typed as `Record<TaskStatus, …>`. That precision is safe only
 * because every read below goes through `isTaskStatus` first; the looser
 * `Record<string, string | undefined>` is a trap, because an `undefined` check does not catch
 * `'constructor'`, whose lookup returns a function.
 */
/*
 * Icon names, kept as `string` for the reason this module is import-free — see `AgentsPanel`'s
 * table, which carries the full argument. `asIcon` narrows at the render site.
 *
 * `eye` for review rather than another circle: review is the one status that is a *request to a
 * person*, and it should not read as another automatic state in the same family.
 */
const STATUS_GLYPH: Record<TaskStatus, string> = {
  todo: 'circle',
  doing: 'circle-dot',
  review: 'eye',
  done: 'circle-check',
}

const STATUS_LABEL: Record<TaskStatus, string> = {
  todo: 'Todo',
  doing: 'Doing',
  review: 'Review',
  done: 'Done',
}

const STATUS_TONE: Record<TaskStatus, Tone> = {
  todo: 'idle',
  doing: 'busy',
  review: 'attention',
  done: 'done',
}

/*
 * What an unrecognised status gets. Constants rather than the raw value, because the raw value
 * may be the empty string, and an empty glyph is a row whose marker column silently collapses —
 * which reads as "this task has no state" rather than "cide does not know this state".
 */
const UNKNOWN_GLYPH = 'circle-slash'
const UNKNOWN_LABEL = 'Unknown'
const UNKNOWN_TONE: Tone = 'idle'

/** The marker at the head of a task row. Never empty, for any input. */
export function statusGlyph(status: TaskStatus): string {
  return isTaskStatus(status) ? STATUS_GLYPH[status] : UNKNOWN_GLYPH
}

/** The group heading, and the label on the four-way status segment. Never empty. */
export function statusLabel(status: TaskStatus): string {
  return isTaskStatus(status) ? STATUS_LABEL[status] : UNKNOWN_LABEL
}

/** Which colour role the marker takes. Never `undefined`, for any input. */
export function statusTone(status: TaskStatus): Tone {
  return isTaskStatus(status) ? STATUS_TONE[status] : UNKNOWN_TONE
}

/* ------------------------------------------------------------------------------- the views */

/** Who wrote a comment. Structural restatement of `TaskAuthor`. */
export type CommentAuthor =
  | { kind: 'user' }
  | { kind: 'orchestrator' }
  | { kind: 'agent'; agent: string; label: string }

/**
 * One line of a task's log. Structural restatement of `TaskComment`.
 *
 * # Append-only for agents; the user may edit and delete (M21)
 *
 * It was append-only for everyone here too. The argument still holds where it bites — an agent
 * that can rewrite a comment after the fact leaves nobody able to tell that what they are acting
 * on is not what was written — but it never justified stopping the person at the keyboard fixing
 * their own typo. The boundary is enforced in Rust, on the *caller* rather than on the comment's
 * author: `TasksStore::edit` refuses both variants for anything but `TaskAuthor::User`, and the
 * MCP tools cannot express them at all. Nothing in this module is load-bearing for that.
 *
 * `text` is drawn as **text with whitespace preserved and never as markup** — it is
 * model-authored, and rendering model-authored markup inside the IDE's own chrome is an
 * injection surface bought for nothing at a 320px panel width.
 */
export interface CommentView {
  /** `TaskComment::id`. What an edit or a delete names — never the index, which regroups. */
  id: string
  author: CommentAuthor
  text: string
  /** `TaskComment::atUnixMs`, converted from `bigint` by `adapt.ts`. See the module header. */
  atMs: number
  /**
   * When the user last edited it, or `null`.
   *
   * Drawn, and that is the point. The user editing an *agent's* comment is the one case that
   * could still mislead a later reader — the hazard the append-only rule existed to prevent,
   * with the user in the agent's place — so the mark is what keeps the log honest about it.
   */
  editedMs: number | null
}


/**
 * One status transition. Structural restatement of `TaskStatusChange`. (M27)
 *
 * `from`/`to` are `TaskStatus` by annotation and a hand-editable file by fact, the same promise
 * every wire status carries — which is why the card renders them through `statusLabel`, whose
 * guard answers `Unknown` rather than throwing on a value a future cide wrote.
 */
export interface StatusChangeView {
  from: TaskStatus
  to: TaskStatus
  by: CommentAuthor
  /** `TaskStatusChange::atUnixMs`, converted from `bigint` by `adapt.ts`. */
  atMs: number
}

/** One task, as the panel draws it. Structural restatement of `Task`. */
export interface TaskView {
  /** `t-17`. Short on purpose: agents quote task ids inside prompts and comments. */
  id: string
  title: string
  /** The whole statement of the work, possibly empty. Never `null` — the wire is not nullable. */
  body: string
  status: TaskStatus
  /**
   * The role this task is **for**, never "who is working on it now".
   *
   * A task outlives the run that failed at it. "Which agent is on it now" is [`agentChip`]
   * reading `AgentRun::task` the other way round, derived at render time and stored nowhere —
   * two writers for one fact is how a task ends up claiming an agent that exited an hour ago.
   */
  agent: string | null
  /** Oldest first on the wire; [`commentOrder`] is the defence against a file that is not. */
  comments: readonly CommentView[]
  /**
   * Every status transition, oldest first; [`historyOrder`] is the same defence. (M27)
   *
   * Recorded in Rust where the mutation is applied, so "by whom" is the connection's identity
   * and not a claim in a payload. The card draws it **collapsed** — it is the log you need
   * rarely and must be able to trust absolutely when you do.
   */
  history: readonly StatusChangeView[]
  /**
   * Who asked for this task. Structural restatement of `Task::createdBy`.
   *
   * The same three-armed author the log carries, and read the same way — off the enum, never off
   * a name. The card draws it as [`authorLabel`], so the user's own tasks say *You* whatever an
   * agent definition has called itself.
   *
   * It is not nullable and there is no "unknown" arm: `cide_tasks::repair` recovers the creator
   * of a task written before the field existed from the comment that build seeded, and anything
   * it cannot recover is the user — which was that build's rule for what an unseeded log meant.
   */
  createdBy: CommentAuthor
  createdMs: number
  updatedMs: number
}

/**
 * Who wrote a comment, or who asked for a task, as a word.
 *
 * **Read off the author enum and never off a name string**, which is what the wire carries a
 * three-armed union for. With a string, "is this mine" would be `author === 'you'` against a
 * value an agent definition is free to claim, and the answer would be wrong in the one case it
 * matters: a user reading a log back to work out which half of it they said.
 *
 * The fallback ladder for an agent — label, then id, then the bare word — is [`agentChip`]'s, and
 * for its reason: a role renamed or deleted out of `.cide/` must still leave a line attributable
 * to *something*, because an anonymous assertion in a log is one no reader can weigh.
 *
 * Lives here rather than in `TaskDetail.tsx` so `check-agents.mjs` can drive it: *You* is a claim
 * about the user's own writing, and the card and the creator line have to make it identically.
 */
export function authorLabel(author: CommentAuthor): string {
  if (author.kind === 'user') return 'You'
  if (author.kind === 'orchestrator') return 'Orchestrator'
  if (author.label.trim() !== '') return author.label
  return author.agent.trim() !== '' ? author.agent : 'Agent'
}

/**
 * What the panel knows about this project's tracker right now.
 *
 * The wire's `TaskBoard` has three arms; this has **four**, and the extra one is the point.
 * `tasks.board` is called from a render effect through `pendingCommand` with a `null` fallback,
 * so there is a real interval — every open of the panel, and the whole of it on a build without
 * the handler — in which nobody has looked yet. Rendering `Absent`'s designed prose during that
 * interval would print *"there is no tracker in /home/you/work/thing"* one frame before the
 * tracker arrives, under a button that creates a file; rendering `Ready` with no tasks would be
 * the confident empty list `ProblemsPanel` was written against. So "nobody has looked" is a
 * state, not the absence of one, and [`BOARD_UNKNOWN`] is its value.
 */
export type Board =
  | { kind: 'unknown' }
  | { kind: 'absent'; hint: string; path: string }
  | { kind: 'unreadable'; path: string; error: string }
  | { kind: 'ready'; tasks: readonly TaskView[]; rev: number }

/** The board before anything has answered. See [`Board`] for why this is not `Absent`. */
export const BOARD_UNKNOWN: Board = { kind: 'unknown' }

/* -------------------------------------------------------------------------- the cross-link */

/**
 * A live run, as much of one as this panel needs.
 *
 * Deliberately **not** `AgentsPanel/model.ts`'s `RunView`. Both modules must stay import-free
 * for `check-agents.mjs` to compile them standalone, so neither can import the other and there
 * is no third module they could share — a shared one would have to be imported, which is the
 * thing that is not allowed. This is the smaller half of that trade, and it is bounded: five
 * fields, and the check pins the phase vocabulary across the seam.
 *
 * `phase` is therefore an **opaque string** here, validated only against [`WORKING_PHASES`]. A
 * phase this build has never heard of is treated as not working, which is the conservative
 * direction: an unknown state draws no lit chip, so an exited agent can never be claimed to be
 * working by a value nobody recognised.
 */
export interface RunRef {
  run: string
  /** The task this run was dispatched against, or `null` for an ad-hoc run. */
  task: string | null
  /** `AgentRun::agentLabel`, copied at dispatch so it still reads after a role is renamed. */
  agentLabel: string
  phase: string
  /** The PTY session, so the chip's Open control has something to act on. `null` while queued. */
  session: string | null
}

/**
 * The phases in which a run holds a slot and the worktree it was dispatched into — that is, is
 * **working** — a second copy of `AgentsPanel/model.ts`'s `WORKING_PHASES`, for the reason
 * [`RunRef`] gives.
 *
 * The question this list exists to answer here is [`agentChip`]'s: **is a run working on this
 * task right now**, where working means the turn is in that run's hands. A task is claimed for
 * exactly as long as some run holds the checkout the task is being done in.
 *
 * `queued` is excluded: a queued run has no child and no session, and a chip lit for one would
 * say an agent is working when nothing has started. The two terminal phases are excluded for
 * the stronger version of the same reason — a lit chip on a finished run is precisely the "an
 * exited agent is still working" claim this whole function exists to make impossible. `idle` is
 * excluded because the turn was handed back and the slot released: the child is still there,
 * which is why the run is not *gone*, but nothing is being done on the task.
 *
 * `paused` is included, and it is the one that is worth stating. A frozen child has handed
 * nothing back and has not let go of the worktree, so the task is still claimed and the chip
 * stays lit; going dark there would say the task is free when picking it up would collide with
 * a checkout another run still owns. The Agents panel's copy makes the same call for the same
 * reason, and `check-agents.mjs` is what keeps the two answers one answer.
 *
 * `check-agents.mjs` asserts every member of this list is a member of `RUN_PHASES` over there,
 * so the copy cannot drift into naming a phase that no longer exists, and that it equals the
 * original phase for phase.
 */
export const WORKING_PHASES: readonly string[] = [
  'starting',
  'running',
  'awaitingPermission',
  'paused',
]

/**
 * Is this run working on its task right now — holding a slot and the worktree?
 *
 * An unrecognised phase is not: `false` is the answer that cannot claim work nobody verified.
 */
export function isWorkingPhase(phase: string): boolean {
  return WORKING_PHASES.includes(phase)
}

/**
 * How a task's agent column is drawn, and which of the two facts it is drawing.
 *
 * `lit` and `tone` are **both** carried, and the redundancy is deliberate. The whole claim this
 * chip makes is that a live run and a mere assignment do not look alike; a `Chip` with only a
 * label would let a view render the two identically and say an agent that exited an hour ago is
 * still working. Two independent discriminators mean the view has to go out of its way to
 * collapse them, and `check-agents.mjs` asserts the two renderings differ.
 */
export type ChipTone = 'live' | 'attention' | 'assigned'

export interface Chip {
  /** The role's name. **Never empty** — see [`agentChip`]'s fallback ladder. */
  label: string
  /** True only when a run that is *working* on this task — see [`WORKING_PHASES`] — backs it. */
  lit: boolean
  /**
   * With `lit` true: `attention` is a live run awaiting permission, `live` is every other
   * working phase. With `lit` false: `assigned` is a plain assignment, and `attention` is the
   * **stalled** reading — a `doing` task whose role has no run engaged on it (see
   * [`agentChip`]'s fourth arm). The pair is the vocabulary on purpose: a fourth `ChipTone`
   * member would let a view render stalled without consulting `lit`, and collapsing the two
   * discriminators is exactly what the type exists to prevent.
   */
  tone: ChipTone
  /** The run's phase verbatim, unvalidated, or `null` when only an assignment backs the chip. */
  phase: string | null
  /** The run id, or `null`. What the row's Open control acts on. */
  run: string | null
  /** The run's session, or `null`. `null` with a non-null `run` means the run is queued. */
  session: string | null
}

/**
 * Look a role label up in a plain object safely.
 *
 * `Object.hasOwn` and not `roles[id] ?? null`: the map is keyed by agent ids that come out of
 * `.cide/agents/*.md` file stems, and `roles['constructor']` on a bare object literal returns a
 * function. A function reaching JSX is not a valid React child.
 */
function roleLabel(roles: Readonly<Record<string, string>>, id: string | null): string | null {
  if (id === null || id.trim() === '') return null
  if (!Object.hasOwn(roles, id)) return id
  const label = roles[id]
  if (typeof label !== 'string' || label.trim() === '') return id
  return label
}

/**
 * **The cross-link, and the one place the two directions meet.**
 *
 * A *live run* on this task wins and is marked `lit`; otherwise a `doing` task whose role has
 * no run engaged on it is marked **stalled**; otherwise the assigned role is marked dim;
 * otherwise there is nothing to say and the answer is `null`. **Four renderings for four
 * facts.** A chip that looked the same either way would claim an exited agent is still working,
 * which is the single wrong statement this panel is in a position to make: the row would be
 * telling the user that work is under way when the process is gone and nothing is coming.
 *
 * # The stalled reading, and its exact boundary
 *
 * The orchestrator's own debug notes named the gap: a killed run comments nothing and resets
 * nothing, so the board showed `doing`, zero comments, no run — "a state that is
 * indistinguishable from *just started* without manually inspecting the worktree", and one task
 * was orphaned that way three times. This arm is the board saying it out loud. The boundary,
 * each edge a decision:
 *
 * * **Only `doing`.** An assigned `todo` task is simply not started — assignment auto-starts a
 *   run, but a project with agents off, a full queue snapshot, or a plain hand-assignment are
 *   all ordinary; `review` and `done` are past the point where a missing run means anything.
 *   `doing` is the one status that *claims* work is under way, so it is the one status whose
 *   claim can be false.
 * * **A `queued` run suppresses it.** Queued is not working — the chip stays unlit — but the
 *   system is on the task and will start it when a slot frees; stalled would say "nobody is
 *   coming" about a run that is literally next.
 * * **An `idle` run suppresses it.** The child is alive at its prompt, one retry or follow-up
 *   from moving, and the turn-end nudge has already routed that fact to the orchestrator; a
 *   stalled flag would double-report a state that has a live handle.
 * * **An `interrupted` run does NOT suppress it.** Its child died with a cide restart and
 *   nothing moves until a human presses Resume — which is precisely what the attention tone
 *   asks for. The Agents panel shows the same fact on the run's own row; this is the *task's*
 *   side of it, because the board is what the orchestrator and the user actually read.
 *   `finished`, `failed` and any phase this build has never heard of are the same answer for
 *   the same reason: no engaged run, claim false, say so.
 *
 * The list row and the detail strip both call this, so they cannot disagree about which of the
 * four a task is in.
 *
 * Among several live runs on one task, an `awaitingPermission` one wins. That is the only run
 * state that is a call to action, and hiding it behind a sibling that merely happens to be
 * earlier in the array would bury the one row the user has to act on.
 *
 * Never throws and never yields an empty label, for any input — a run naming a task that does
 * not exist, a task naming a role the roster does not define, a phase outside the enum. All
 * three are ordinary: the roster and the board are separate reads of separate files by separate
 * commands, and they are routinely a few hundred milliseconds apart.
 */
export function agentChip(
  task: TaskView,
  runs: readonly RunRef[],
  roles: Readonly<Record<string, string>>,
): Chip | null {
  let live: RunRef | null = null
  for (const run of runs) {
    if (run.task !== task.id || !isWorkingPhase(run.phase)) continue
    if (live === null) live = run
    // The call to action outranks arrival order; see the doc comment.
    if (run.phase === 'awaitingPermission') {
      live = run
      break
    }
  }

  if (live !== null) {
    // The ladder: the label the run was dispatched under, then the role the task is assigned
    // to, then a word. A chip with an empty label is a chip that reads as a rendering bug.
    const label =
      live.agentLabel.trim() !== '' ? live.agentLabel : (roleLabel(roles, task.agent) ?? 'Agent')
    return {
      label,
      lit: true,
      tone: live.phase === 'awaitingPermission' ? 'attention' : 'live',
      phase: live.phase,
      run: live.run,
      session: live.session,
    }
  }

  const assigned = roleLabel(roles, task.agent)
  if (assigned === null) return null

  /*
   * The stalled arm — see the doc's boundary. Working phases cannot reach this line (the live
   * branch above claimed them), so "engaged" here is only the two quiet-but-alive shapes:
   * queued (the system is on it) and idle (a live child, one prompt from moving). The literal
   * `'doing'` is deliberate rather than `groupOf`: a rogue status is *drawn* under Todo, and a
   * stall claim about a status this build cannot read would be an attention flag on a guess.
   */
  const engaged = runs.some(
    (run) => run.task === task.id && (run.phase === 'queued' || run.phase === 'idle'),
  )
  if (task.status === 'doing' && !engaged) {
    return { label: assigned, lit: false, tone: 'attention', phase: null, run: null, session: null }
  }

  return { label: assigned, lit: false, tone: 'assigned', phase: null, run: null, session: null }
}

/* -------------------------------------------------------------------------- the group list */

/** One group of the panel's list: a status, its heading, and the tasks in it. */
export interface TaskGroup {
  status: TaskStatus
  label: string
  tasks: readonly TaskView[]
}

/**
 * Which group a task is drawn in — **the one answer to that question**, shared by [`groups`]
 * and [`matchesFilter`].
 *
 * A status outside the four lands in `todo`, and the reason is [`groups`]'s: a task the panel
 * cannot place must stay visible rather than fall out of a tracker somebody is relying on. The
 * reason it is a *function* rather than two copies of `isTaskStatus(…) ? … : 'todo'` is the
 * filter: if the list put a rogue-status task under `Todo` and the filter did not match it
 * there, then filtering to Todo would make a task disappear that the unfiltered list had just
 * shown under Todo — which is the same task-silently-leaves-the-tracker failure, arrived at
 * from the other side.
 */
export function groupOf(task: TaskView): TaskStatus {
  return isTaskStatus(task.status) ? task.status : 'todo'
}

/**
 * What the list is narrowed to, or `null` for the whole board.
 *
 * **"All" is the absence of a filter, not a fifth value**, and that is a decision rather than a
 * spelling. A fifth member would have to be excluded by hand from [`TASK_STATUSES`] — which is
 * pinned against `pub enum TaskStatus` — and from the three `Record<TaskStatus, …>` tables,
 * each of which would then need a glyph, a label and a tone for a thing that is not a status.
 * `null` costs one comparison in [`matchesFilter`] and nothing anywhere else, and it makes the
 * *default* state of the panel the one that cannot lie: no filter, every task.
 */
export type StatusFilter = TaskStatus | null

/**
 * Does this task survive the filter?
 *
 * Total: `null` matches everything, and a status this build cannot read is matched by whatever
 * filter [`groupOf`] places it under, so a rogue task can be reached rather than being
 * permanently unreachable behind a filter that never matches it.
 */
export function matchesFilter(task: TaskView, filter: StatusFilter): boolean {
  if (filter === null) return true
  return groupOf(task) === filter
}

/* ------------------------------------------------------------------------------ the search */

/*
 * The one definition of "this text contains that query", shared by [`matchesQuery`] and
 * [`queryAfterCreate`] so the two cannot disagree about what a hit is — the same one-answer
 * discipline `groupOf` exists for, one feature over.
 */
function hitsQuery(q: string, texts: readonly string[]): boolean {
  return texts.some((text) => text.toLowerCase().includes(q))
}

/**
 * Does this task survive the search box?
 *
 * **`''` is the absence of a search**, the way `null` is the absence of a [`StatusFilter`] —
 * but spelled as the empty string rather than as `null`, because the query *is* a text box's
 * value and a text box has no null: the state the control rests in has to be the state that
 * matches everything, or clearing the box would need a second gesture. Whitespace counts as
 * absent too, so a stray space cannot blank the board with nothing visibly typed.
 *
 * **Case-insensitive substring, and deliberately not the pickers' fuzzy scoring.** A scored
 * match wants to *rank*, and the list's order is already spoken for — recency, the same with or
 * without a query (see [`groups`]) — so a relevance order would make rows jump as the query
 * grew. A subsequence matcher bolted onto a list that must keep an order of its own would also
 * surface rows for queries that visibly contain none of the typed text, with no score on screen
 * to explain them; a substring either is in the row or is not, which is the only kind of match
 * a user can verify at a glance.
 *
 * **Over the id, the title and the body — and not the comments.** The id because it is on the
 * row and agents quote it in prompts, so "t-14" must find t-14. The body because it is the
 * statement of the work, which is what a user half-remembering a task actually recalls. The
 * comments are excluded on purpose: a hit there would draw a row whose visible line contains
 * nothing the user typed — a match with an invisible reason — and the log is the noisiest,
 * most model-authored text in the tracker, so it is also where a short query matches most and
 * means least. Searching the conversation is a different feature from finding a task.
 *
 * Total, and never throws: every field read is a `string` by construction, and a rogue status
 * plays no part in it, so a task this build cannot place is still findable.
 */
export function matchesQuery(task: TaskView, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (q === '') return true
  return hitsQuery(q, [task.id, task.title, task.body])
}

/**
 * Why the list has nothing in it — **and the three answers are different sentences**.
 *
 * `'tracker'` is *this project has no tasks*, which is the screen that already existed and the
 * one that offers New task. `'filter'` is *this project has tasks and you are looking at a
 * slice with none in it*, which is a completely different thing to tell a user: a filtered
 * board that silently drew nothing is how somebody concludes their tasks are gone, and this
 * project has a standing rule against a confident empty list. `'search'` is that same fact
 * arrived at through the text box, and it is a third sentence rather than a reuse of the
 * second because the way out differs: the filter's screen names a status and offers Show all,
 * the search's has to name what was *typed* and offer to clear it.
 *
 * When both narrowings are on and nothing survives, **the search takes the blame**. Not
 * because it is likelier — because it is the answer the screen can be honest about: the
 * search sentence names the query *and* the filter it ran inside, where blaming the filter
 * would print a sentence claiming a status hid tasks that a different control is hiding.
 *
 * `null` for every board arm that is not `ready` — those three have their own designed screens
 * and none of them is a list — and `null` when the narrowed list is not empty at all.
 */
export type EmptyList = 'tracker' | 'filter' | 'search'

export function listEmpty(board: Board, filter: StatusFilter, query = ''): EmptyList | null {
  if (board.kind !== 'ready') return null
  if (board.tasks.length === 0) return 'tracker'
  if (board.tasks.some((task) => matchesFilter(task, filter) && matchesQuery(task, query))) {
    return null
  }
  return query.trim() === '' ? 'filter' : 'search'
}

/**
 * The panel's body, grouped by status in [`GROUP_ORDER`].
 *
 * **Order within a group is recency: last touched first.** `updatedMs` descending, then
 * `createdMs` descending, then — the sort being stable — the file's own order for full ties.
 * "Touched" already means what a user expects it to, because Rust stamps `updated_unix_ms` on
 * **every** `TaskEdit` in one place (`TasksStore::edit`, after the match): a status change, a
 * new comment, an assignment, a retitle, an edited body all count. The panel answers *what is
 * happening*, and within a status the task something just happened to is the one being asked
 * after.
 *
 * The paragraph that used to be here argued the opposite — that `TaskFile::tasks` is a list
 * precisely so the array can *be* the priority, and that re-sorting would invent a second
 * ordering the file contradicts. It lost on use: appends land at the end of the array, so the
 * file's order is oldest-first for ever and the rows agents were actively working sat below a
 * backlog nobody was reading. What survives of it is real, though — the array is still the
 * order a pull request reads, still the tie-break here, and the file is not rewritten to match
 * the screen: this is a *reading* order derived from stamps the file already carries, never a
 * stored second one, which is why `TaskFile`'s argument against an `order` field stands.
 *
 * **Empty groups are omitted.** A `Done` heading over nothing is noise on a 320px panel, and
 * the states that genuinely need prose are the board's own three non-`ready` arms, which return
 * `[]` here so the panel draws its designed screen instead of four empty headings.
 *
 * **The filter and the query narrow the tasks, never the headings.** A group with nothing in
 * it is omitted exactly as before, so filtering to Doing draws the Doing heading and no others
 * rather than four headings with one populated — and the caller still gets `[]` when nothing
 * matched, which is [`listEmpty`]'s job to explain rather than this one's. The two narrowings
 * compose as the intersection, because each is its own claim about the row and a row on screen
 * must satisfy both of the controls that say it should be there.
 *
 * A task whose status is none of the four is **not dropped**. Grouping strictly by value would
 * match no heading and the task would simply vanish from a tracker somebody is relying on —
 * unacceptable for the same reason `countBySeverity` has an `other` bucket rather than
 * discarding an unrecognised severity. It lands in `todo` instead: visible, actionable, and in
 * the group that claims the least. Not `doing`, which would be the panel asserting that work is
 * under way on the strength of a value it could not read.
 */
export function groups(board: Board, filter: StatusFilter = null, query = ''): TaskGroup[] {
  if (board.kind !== 'ready') return []
  const buckets = new Map<TaskStatus, TaskView[]>()
  for (const status of GROUP_ORDER) buckets.set(status, [])
  for (const task of board.tasks) {
    if (!matchesFilter(task, filter) || !matchesQuery(task, query)) continue
    buckets.get(groupOf(task))?.push(task)
  }
  const out: TaskGroup[] = []
  for (const status of GROUP_ORDER) {
    const tasks = buckets.get(status)
    if (tasks === undefined || tasks.length === 0) continue
    out.push({ status, label: statusLabel(status), tasks: recencyOrder(tasks) })
  }
  return out
}

/*
 * A timestamp that can be compared. `adapt.ts`'s `Number(…)` cannot make a non-finite value out
 * of the wire's u64, but this file is hand-editable JSON two layers down and `newerBoard`
 * already refuses to order by a stamp it cannot trust. Non-finite compares as the epoch, so a
 * mangled stamp sinks to the bottom of its group — a comparator that returns `NaN` is not an
 * ordering at all, and hands `Array.sort` licence to leave the array in any arrangement it
 * likes.
 */
function stamp(ms: number): number {
  return Number.isFinite(ms) ? ms : 0
}

/**
 * [`groups`]'s within-group order — last touched first; its doc carries the argument. Sorts in
 * place, because the one caller hands it a bucket it just built and nothing else holds it.
 */
function recencyOrder(tasks: TaskView[]): TaskView[] {
  return tasks.sort(
    (a, b) => stamp(b.updatedMs) - stamp(a.updatedMs) || stamp(b.createdMs) - stamp(a.createdMs),
  )
}

/**
 * How many tasks are not done, or `null` when nobody has looked.
 *
 * **`null` and `0` are different claims and must not be conflated**, which is `ProblemsPanel`'s
 * badge rule: the activity rail draws nothing for `null` — *unknown* — and nothing for `0` —
 * *looked, and the tracker is clear* — and the moment a surface prints `0` for the first of
 * them the panel is making a claim nothing checked. Every arm but `ready` returns `null`,
 * including `absent`: a project with no tracker has no count, it has no tracker.
 */
export function openCount(board: Board): number | null {
  if (board.kind !== 'ready') return null
  let n = 0
  for (const task of board.tasks) if (task.status !== 'done') n += 1
  return n
}

/**
 * The header's right-aligned meta figure, or `null` to draw nothing at all.
 *
 * `null` rather than `ProblemsPanel`'s `'—'` because this header has two panels beside it and
 * an em dash in every one of them is three claims of ignorance where the honest render is an
 * empty slot.
 *
 * `open/total` when some tasks are done, because the two numbers answer different questions —
 * "how much is left" and "how big is this" — and a header showing only the first reads as a
 * shrinking tracker rather than a progressing one.
 *
 * # It takes no [`StatusFilter`], and that is the decision rather than an omission
 *
 * The header names the **tracker**, not the slice of it the user is currently reading. Make it
 * follow the filter and the `done` filter reads `0/1` — *nothing left, one task* — over a board
 * with three open tasks in it, which is exactly the "my tasks are gone" conclusion the filter's
 * own empty state ([`listEmpty`]) exists to prevent, relocated into the one line of the panel a
 * user trusts to be a fact about the file. The counts the filter itself needs belong to the
 * filter row, next to the control that changed them.
 *
 * A second reason, weaker but worth writing down because it is the one that would be discovered
 * late: this figure sits beside [`openCount`], which `App.tsx` calls **on the store's board
 * directly** for the activity rail's badge. The two are already independent — a filter in this
 * panel cannot reach the badge, and could not be made to — so a filtered header would put two
 * different numbers for the same tracker on screen at once, one of them in a rail the user can
 * see with the panel shut.
 */
export function metaFigure(board: Board): string | null {
  if (board.kind !== 'ready') return null
  const open = board.tasks.filter((task) => task.status !== 'done').length
  return open === board.tasks.length ? String(open) : `${open}/${board.tasks.length}`
}

/* --------------------------------------------------------------------------- the rev drop */

/**
 * Which of two boards the store should hold — `applySnapshot`'s rev-drop rule, as a pure
 * function so a check can drive it.
 *
 * `.cide/tasks.json` genuinely has several writers — two cide windows and every dispatched
 * agent — so `cide://tasks-changed` snapshots can arrive out of order, and a receiver that took
 * the last one to land would show a board that is visibly behind the click that changed it.
 * That is `workspace_changed`'s situation exactly, and it gets `workspace_changed`'s answer:
 * **a `ready` board whose `rev` is not strictly newer is dropped.**
 *
 * Equal is dropped, not accepted. Two windows re-reading the same file produce two equal-`rev`
 * snapshots and admitting the second buys nothing while costing a re-render of a list the user
 * may be mid-scroll in.
 *
 * A dropped snapshot **returns `current` itself, not a copy**, so a `useSyncExternalStore`
 * reader sees the identical reference and does not re-render at all — the same argument
 * `chrome/notices.ts`'s `admit` makes when it declines a duplicate. A structurally-equal copy
 * would satisfy every assertion about *contents* and still repaint the panel on every event.
 *
 * A **non-`ready` board always replaces**, whatever the predecessor's `rev` was. `rev` is a
 * property of a file that parsed; a tracker that has become unreadable, or been deleted, has no
 * rev to compare and is news regardless. Holding the last good board because it had a higher
 * number would leave the panel offering writes against a file that no longer parses — which is
 * the one thing `Unreadable` exists to prevent.
 */
export function newerBoard(current: Board, next: Board): Board {
  if (next.kind !== 'ready') return next
  if (current.kind !== 'ready') return next
  // A non-finite `rev` cannot be compared into an ordering, so it is treated as not-newer: the
  // known-good board is kept rather than replaced by one whose position is unknowable.
  if (!Number.isFinite(next.rev)) return current
  if (!Number.isFinite(current.rev)) return next
  return next.rev > current.rev ? next : current
}

/**
 * May the panel offer anything that writes?
 *
 * **`false` for `unreadable`, and that is the point of the function.** An app that "recovers"
 * from an unparseable tracker by overwriting it has destroyed the user's data in order to fix
 * its own display — and the most likely cause of an unparseable `.cide/tasks.json` is a
 * half-resolved merge conflict, which is to say a file that still contains *both sides of
 * everything*. That state offers Reveal and Retry and nothing else: no New task, no Reset, no
 * repair.
 *
 * `false` for `unknown` too: nobody has looked, so a write would be composed against a board
 * that may not be the one on disk.
 *
 * **`true` for `absent`**, which is the case that makes this a question rather than a constant.
 * There is no file, and the one offered button — New task — is what creates it. The panel says
 * so in full, naming the path, *before* the button: a control that quietly adds a tracked file
 * to somebody's repository is a surprise commit.
 */
export function canWrite(board: Board): boolean {
  return board.kind === 'absent' || board.kind === 'ready'
}

/**
 * A destructive gesture the user has armed: **which task, and on which board**.
 *
 * The `rev` is the whole point. Arming is a claim about a *screen* — "this row, the one I am
 * looking at" — and `.cide/tasks.json` has several writers (two cide windows, and every
 * dispatched agent through the orchestration MCP server), so the screen can be replaced between
 * the arming click and the confirming one. Carrying only the task id would let the second click
 * land on a board the user never agreed to, which for a delete means removing a row that moved,
 * changed status or acquired an agent since they decided.
 */
export interface ArmedDelete {
  task: string
  /** The `rev` of the `ready` board the arming click was made against. */
  rev: number
}

/**
 * Which task, if any, may draw its **confirming** delete control — the pure half of the
 * confirm-on-second-click pattern.
 *
 * Three ways to answer `null`, and each one is a bug that would otherwise be reachable:
 *
 *  - the board is not `ready`. That is the [`canWrite`] gate as well as a type narrowing — a
 *    `ready` board is one of the two arms `canWrite` admits, and the other (`absent`) holds no
 *    tasks to arm against — so a second `canWrite(board)` call here would be a second copy of
 *    the rule rather than a second gate. A confirm button over an unreadable tracker would be a
 *    control offering to write a file the panel has already promised not to touch.
 *  - the board has moved on. **Any** new `rev` disarms, because a rev only advances when the
 *    file actually changed; `newerBoard` hands back the identical board when it drops a
 *    snapshot, so a heartbeat that changes nothing disarms nothing.
 *  - the task is not in the board any more — somebody else deleted it, or it arrived from a
 *    project switch. A confirm button on a row that is gone acts on an id nothing on screen
 *    shows.
 *
 * This is deliberately a *pure* gate applied on every render, rather than only an effect in the
 * host that clears the state when the board changes. React runs effects **after** paint, so an
 * effect alone leaves exactly one frame in which the new board is drawn with the old arming
 * still live — one frame in which a click confirms against a screen the user never saw. The
 * host clears the state as well, so the arming does not linger invisibly; this is what makes
 * the frame safe, and it is what `check-agents.mjs` can drive without a DOM.
 */
export function armedDelete(board: Board, armed: ArmedDelete | null): string | null {
  if (armed === null) return null
  if (board.kind !== 'ready') return null
  if (board.rev !== armed.rev) return null
  return board.tasks.some((task) => task.id === armed.task) ? armed.task : null
}

/**
 * A task's comments, oldest first.
 *
 * `Task::comments` is documented as already being in that order, so this is a defence rather
 * than a transformation — and the thing it defends against is real: this file is committed, and
 * a merge or a hand edit can interleave two agents' lines out of order. A log that is not in
 * time order is a log that reads as a different conversation.
 *
 * The sort is **stable by construction**: ties on `atMs` keep their original position, because
 * two comments stamped in the same millisecond were written in the order they are in and there
 * is nothing better to break the tie with. `Array.prototype.sort` is required to be stable in
 * ES2019 and later, which this project targets.
 *
 * Returns the **identical array** when it is already ordered, so a reader that memoises on
 * identity does not rebuild the log on every keystroke in the title field above it — the same
 * argument [`newerBoard`] and `notices.ts`'s `admit` make.
 */
export function commentOrder(task: TaskView): readonly CommentView[] {
  const comments = task.comments
  let ordered = true
  for (let i = 1; i < comments.length; i += 1) {
    const previous = comments[i - 1]
    const current = comments[i]
    if (previous === undefined || current === undefined) continue
    if (current.atMs < previous.atMs) {
      ordered = false
      break
    }
  }
  if (ordered) return comments
  return [...comments].sort((a, b) => a.atMs - b.atMs)
}

/**
 * A task's status transitions, oldest first — [`commentOrder`]'s defence for the other log. (M27)
 *
 * The same three properties, for the same reasons: stable, identity when already ordered, and a
 * defence rather than a transformation — this file is committed, and a merge can interleave two
 * sides' rows.
 */
export function historyOrder(task: TaskView): readonly StatusChangeView[] {
  const history = task.history
  let ordered = true
  for (let i = 1; i < history.length; i += 1) {
    const previous = history[i - 1]
    const current = history[i]
    if (previous === undefined || current === undefined) continue
    if (current.atMs < previous.atMs) {
      ordered = false
      break
    }
  }
  if (ordered) return history
  return [...history].sort((a, b) => a.atMs - b.atMs)
}

/**
 * A timestamp as the wall clock read it: `14:32:07`, local time, 24-hour, always eight
 * characters. (M27)
 *
 * The card prints this **beside** the relative age, not instead of it — `14:32:07 (4m ago)` —
 * because the two answer different questions: "which of these two lines came first" needs the
 * absolute stamp (two `4m ago`s are indistinguishable), and "is this still current" needs the
 * age (a bare clock time is ambiguous across days, and the age in the brackets is what
 * disambiguates it without a date vocabulary this panel does not have).
 *
 * Local time on purpose — it is read next to the reader's own wall clock. Total: a value that
 * is not a finite number of milliseconds renders as a visibly-broken placeholder rather than
 * `NaN:NaN:NaN`.
 */
export function clock(atMs: number): string {
  if (!Number.isFinite(atMs)) return '--:--:--'
  const date = new Date(atMs)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

/* ------------------------------------------------------- the card's read/edit posture */

/**
 * **The card is read-only until a field is put into edit, one field at a time.**
 *
 * The rules for that live here rather than in `TaskDetail.tsx` for the reason everything else
 * in this file does: `check-agents.mjs` compiles this module alone and drives it under node, and
 * "what happens to the title you were typing when you click the pencil on the body" is exactly
 * the kind of question that is decided three times in three event handlers and answered
 * differently by each. The component below executes the intent it is handed; it does not decide
 * one.
 *
 * # Why the posture changed
 *
 * The first version of the card was a form: a title `<input>` that committed on blur, a
 * `<textarea>` for the body, a `<select>` for the assignee. Every one of them wrote to
 * `.cide/tasks.json` — a file the whole team commits — the moment the user looked away, so
 * *reading* a task and *rewriting* it were the same screen and a stray keystroke was a commit.
 * Opening a task now shows its values as text; changing one is a deliberate act with a control
 * of its own.
 *
 * # Which fields, and why not all four
 *
 * `status` is in [`TASK_FIELDS`] and deliberately **not** in [`EDITABLE_FIELDS`]. It stays the
 * four-way segment it already was, live at all times, with no pencil on it. Three reasons, and
 * the first is the one that decides it:
 *
 *  - **A segment cannot be changed by a gesture that was not aimed at it.** Each of its four
 *    buttons has to be clicked, or tabbed to and Entered, *by name*. That is the whole hazard
 *    the read posture exists to remove, and it is already absent here. A `<select>` is the
 *    opposite and is the reason `assignee` *is* in the list: focus one while merely traversing
 *    the card and press ↓, and the assignment changes and is committed — a write nobody aimed.
 *  - **The lit segment is already the read-only rendering.** The value is on screen without
 *    opening anything, which is what "read-only by default" is asking for. A pencil would hide
 *    the task's state behind a disclosure to reveal a control that shows the same thing.
 *  - **It is the gesture the tracker exists for.** Moving a task from Doing to Review is what a
 *    board is; putting a click in front of it costs the panel's most frequent act to protect
 *    against a hazard it does not have.
 *
 * The other half of that argument is why `assignee` is here even though a `<select>` is already
 * one click: what that click opens is a list nobody can read until it is open, so the "one-step
 * control" the pencil would replace was never one step. It costs a click on the rare gesture — a
 * task is assigned once and moves status four times — and buys the same protection the two text
 * fields get.
 *
 * Comments are not a field and have no entry here. The log is append-only on the wire and
 * append-only in the card: an editable comment is one an agent can quietly rewrite after the
 * fact with nobody in a position to tell.
 */
export type TaskField = 'title' | 'status' | 'assignee' | 'body'

/** The four, in the order the card draws them. */
export const TASK_FIELDS: readonly TaskField[] = ['title', 'status', 'assignee', 'body']

/** A field that rests as text and is put into edit by the control on its own row. */
export type EditableField = 'title' | 'assignee' | 'body'

/**
 * The three fields that carry an edit affordance. **A subset of [`TASK_FIELDS`], never equal to
 * it** — see the posture note above for why `status` is the one left out, and `check-agents.mjs`
 * pins the exclusion so a later pass at consistency has to argue with it rather than slip past.
 */
export const EDITABLE_FIELDS: readonly EditableField[] = ['title', 'assignee', 'body']

/** Is this one of the four fields the card draws? Takes `string`; see [`isTaskStatus`]. */
export function isTaskField(value: string): value is TaskField {
  return TASK_FIELDS.includes(value as TaskField)
}

/** Is this a field that can be put into edit? `false` for `status`, and for anything else. */
export function isEditableField(value: string): value is EditableField {
  return EDITABLE_FIELDS.includes(value as EditableField)
}

const FIELD_LABEL: Record<TaskField, string> = {
  title: 'Title',
  status: 'Status',
  assignee: 'Assignee',
  body: 'Body',
}

/*
 * What a field this build cannot name is called, and what it reads as at rest.
 *
 * Constants rather than the raw value for [`UNKNOWN_GLYPH`]'s reason: an empty label is a row
 * whose heading silently collapses, which reads as "this task has no such field" rather than
 * "cide does not know this field". Neither is reachable from the app today — every call site
 * names a literal — and both exist because the check drives these functions with `'constructor'`.
 */
const UNKNOWN_FIELD_LABEL = 'Field'
const UNKNOWN_FIELD_TEXT = '—'

/** The heading over a field. Never empty, for any input. */
export function fieldLabel(field: string): string {
  return isTaskField(field) ? FIELD_LABEL[field] : UNKNOWN_FIELD_LABEL
}

/** What an unassigned task's assignee row reads as. */
export const UNASSIGNED = 'Unassigned'

/** What a task with no title reads as — it has one on screen either way. */
export const NO_TITLE = 'Untitled'

/** And with no body. A tracker row with an empty statement of work is ordinary, not broken. */
export const NO_BODY = 'No description.'

/**
 * The assignee, as a word: the role's label, else the raw id, else [`UNASSIGNED`].
 *
 * The id is the fallback rather than "Unassigned", and the difference matters: a task assigned
 * to a role somebody deleted from `.cide/agents/` is still assigned, and drawing it as
 * unassigned would be the card telling the user their assignment is gone when the file says
 * otherwise. Goes through the same `Object.hasOwn` lookup [`agentChip`] uses, so an agent id of
 * `constructor` yields the id rather than `Object.prototype.constructor`.
 */
export function assigneeLabel(
  agent: string | null,
  roles: Readonly<Record<string, string>>,
): string {
  return roleLabel(roles, agent) ?? UNASSIGNED
}

/**
 * What a field reads as when it is **at rest** — the text the card draws in place of a control.
 *
 * Total, and **never empty for any input**. An empty string here is a row that collapses to its
 * heading, which is indistinguishable on screen from a rendering bug; the placeholders above are
 * what the three genuinely-empty cases get instead.
 *
 * `status` is answered too even though the card draws it as a live segment rather than as text.
 * One function that covers every field is what stops a later pass — the one that decides the
 * segment should rest as text after all — from having to invent a second answer for it.
 */
export function restText(
  task: TaskView,
  field: string,
  roles: Readonly<Record<string, string>>,
): string {
  /*
   * Explicit comparisons rather than a `Record` lookup, and that is not style. The value reaches
   * here from a `data-field` attribute and from the check, so `'constructor'` is a real input;
   * a table lookup would hand back `Object.prototype.constructor` — a function, which React
   * refuses as a child and which `className` stringifies into the whole source text of `Object`.
   */
  if (field === 'title') return task.title.trim() !== '' ? task.title : NO_TITLE
  if (field === 'body') return task.body.trim() !== '' ? task.body : NO_BODY
  if (field === 'assignee') return assigneeLabel(task.agent, roles)
  if (field === 'status') return statusLabel(task.status)
  return UNKNOWN_FIELD_TEXT
}

/**
 * A field's value as the editor holds it: a **string**, with `''` for an unassigned task.
 *
 * One function, because "what does the editor start with" and "has the user changed anything"
 * must be the same answer. Two copies is how an untouched assignee `<select>` on a task with no
 * agent comes to look dirty — `null` against `''` — and writes an `Assign(null)` over a `null`
 * on the way out, bumping `rev` and repainting every window for a change that is not one.
 */
export function fieldValue(task: TaskView, field: string): string {
  if (field === 'title') return task.title
  if (field === 'body') return task.body
  if (field === 'assignee') return task.agent ?? ''
  return ''
}

/**
 * The one field that is in edit, and what the user has typed into it so far.
 *
 * The draft lives here — in the host's state, handed down as a prop — rather than in the DOM.
 * The old card's fields were uncontrolled `defaultValue`s committing on blur, which was right
 * for a form and is wrong for this: whether a field is *dirty* is now a decision several
 * gestures consult (activating another field, closing the card), and a decision that reads the
 * DOM cannot be driven by `check-agents.mjs`. The snapshot argument that motivated the
 * uncontrolled fields does not apply — the draft is the user's own text and is never derived
 * from the board, so a `tasks-changed` landing mid-sentence cannot overwrite it.
 */
export interface FieldEdit {
  field: EditableField
  draft: string
}

/** One field's write, ready to be turned into the matching `TaskEdit` by the caller. */
export interface FieldCommit {
  field: EditableField
  value: string
}

/**
 * What a gesture on the card does, as data: **write this, then be in this state**.
 *
 * Every gesture — the pencil on a field, Save, Cancel, Escape, the scrim, the close button —
 * produces one of these and the component executes it verbatim. That is what keeps "committing
 * the field you were in before opening another one" a single rule rather than a thing each
 * handler remembers to do.
 */
export interface EditIntent {
  /** The one field to write before anything else, or `null` for a gesture that writes nothing. */
  commit: FieldCommit | null
  /** The edit state afterwards. `null` puts the card back to fully read-only. */
  editing: FieldEdit | null
  /** Whether the card closes. */
  close: boolean
}

/** A field freshly put into edit: the draft starts at the value on screen. */
export function startEdit(task: TaskView, field: EditableField): FieldEdit {
  return { field, draft: fieldValue(task, field) }
}

/**
 * Has the user actually changed the field that is in edit?
 *
 * `false` for no edit at all, and `false` for a draft equal to the value — which is the case
 * that has to be right, because every "write on the way out" below is gated on this. A `SetTitle`
 * carrying the title the task already has is not a no-op: it bumps `rev`, broadcasts to every
 * window, and puts a line in the diff of a file the whole team commits saying nothing happened.
 *
 * Compared verbatim, without trimming. A user who added a trailing space meant to, and a card
 * that silently declined to save it would be a second, invisible rule about what their text is.
 */
export function isDirty(task: TaskView, edit: FieldEdit | null): boolean {
  if (edit === null) return false
  return edit.draft !== fieldValue(task, edit.field)
}

/**
 * **The pencil.** Put `field` into edit — and deal with the field that is already in one.
 *
 * At most one field is in edit at a time, so activating a second has to answer for the first,
 * and there are only three answers. **Discarding it silently is the wrong one**: the draft is
 * text the user typed, and losing it to a click on an unrelated row is the worst kind of data
 * loss because nothing on screen said it was at risk. **Asking** is worse still — a confirmation
 * over a dialog, for a value the wire can write in one call, on a file `git` has a copy of.
 *
 * So the first field is **committed**, and the second opens. That is sound precisely because the
 * wire is per-field: `TaskEdit` is `SetTitle | SetBody | SetStatus | Assign | Comment`, one
 * variant per field, so there is no cross-field validity to hold open and no whole-form save to
 * be half-done. It is also what the old card already did — blur committed — so it is the
 * behaviour a user of this panel has already learned.
 *
 * Re-activating the field that is *already* in edit is a no-op that keeps the draft: it returns
 * the identical [`FieldEdit`], so a double click on the pencil cannot commit-and-reopen and
 * cannot reset what has been typed. A field this build cannot edit — a `data-field` from a
 * future cide, or `'constructor'` — changes nothing at all rather than blanking the field that
 * is open, because a gesture nobody can name must not be able to close one they can.
 */
export function beginEdit(
  task: TaskView,
  current: FieldEdit | null,
  field: string,
): EditIntent {
  if (!isEditableField(field)) return { commit: null, editing: current, close: false }
  if (current !== null && current.field === field) {
    return { commit: null, editing: current, close: false }
  }
  return {
    commit: isDirty(task, current) && current !== null
      ? { field: current.field, value: current.draft }
      : null,
    editing: startEdit(task, field),
    close: false,
  }
}

/**
 * **Save.** Write the field if it changed, and put the card back to reading.
 *
 * A clean field writes **nothing** — see [`isDirty`]. The card closes the editor either way,
 * because the user asked it to and refusing would leave them pressing a button that appears to
 * do nothing.
 */
export function commitEdit(task: TaskView, current: FieldEdit | null): EditIntent {
  return {
    commit: isDirty(task, current) && current !== null
      ? { field: current.field, value: current.draft }
      : null,
    editing: null,
    close: false,
  }
}

/** **Cancel.** The old value stands and nothing is written. Named so no handler re-decides it. */
export function cancelEdit(): EditIntent {
  return { commit: null, editing: null, close: false }
}

/**
 * How the card was asked to close.
 *
 * `escape` is a key, `dismiss` is every deliberate close — the ✕, the scrim. They are separate
 * because Escape is the only one of them that must be able to mean something *other* than
 * closing; see [`closeCard`].
 */
export type CloseCause = 'escape' | 'dismiss'

/**
 * **Closing.** What happens to the card, and to a field left mid-edit.
 *
 * Two rules, and the split between them is the whole answer to "Escape is both cancel and
 * dismiss":
 *
 *  - **Escape is scoped to the innermost thing that is open.** With a field in edit it cancels
 *    that field and the card stays up; with nothing in edit it closes the card. So Escape never
 *    writes and never costs the user the card they are reading — the two ways a single-meaning
 *    Escape would surprise someone, in opposite directions.
 *  - **A deliberate close commits a dirty field.** Clicking the scrim or the ✕ is *leaving*, and
 *    leaving a field has meant "keep what I typed" in this panel since the first version of the
 *    card. Discarding it would be the silent data loss [`beginEdit`] refuses, and asking would
 *    be a confirmation on top of a dialog. The field is visibly in edit, with Cancel beside it,
 *    and Escape is the way to leave without writing.
 *
 * A clean field commits nothing on the way out either way, for [`isDirty`]'s reason.
 */
export function closeCard(
  task: TaskView,
  current: FieldEdit | null,
  cause: CloseCause,
): EditIntent {
  if (cause === 'escape' && current !== null) {
    return { commit: null, editing: null, close: false }
  }
  return {
    commit: cause !== 'escape' && isDirty(task, current) && current !== null
      ? { field: current.field, value: current.draft }
      : null,
    editing: null,
    close: true,
  }
}

/**
 * Which task the card is open on, or `null` for no card at all.
 *
 * The single answer to that question, because two of them — one deciding whether to draw the
 * modal and one deciding what to draw in it — is how a card ends up on screen over a task the
 * board no longer holds. `some`/`find` compare values and never consult the prototype chain, so
 * a selected id of `constructor` finds nothing rather than a function.
 */
export function openTask(board: Board, selected: string | null): TaskView | null {
  if (board.kind !== 'ready' || selected === null) return null
  return board.tasks.find((task) => task.id === selected) ?? null
}

/**
 * Which edit still applies — the **pure gate**, [`armedDelete`]'s shape and for its reason.
 *
 * React runs effects after paint, so the host clearing its state when the open task goes away
 * still leaves one frame in which the old edit is drawn over the new board: one frame holding a
 * Save button aimed at a task that is not there. Applied on every render, this makes that frame
 * safe, and it is what a check without a DOM can drive.
 *
 * Three refusals:
 *
 *  - the board is not `ready`. That is [`canWrite`]'s answer as well — an editor over an
 *    unparseable tracker is a control offering to write a file the panel has promised not to
 *    touch — and a narrowing, since only a `ready` board holds tasks.
 *  - the task is not on the board. Somebody else deleted it, or it came from the project the
 *    user just left.
 *  - the field is not one this build can edit.
 *
 * Note what is deliberately **not** here: a new `rev` does not clear an edit. `armedDelete`
 * disarms on any board change because arming is a claim about a *screen* the user was looking
 * at, and the screen was replaced. A draft is not a claim about a screen — it is the user's own
 * sentence — and an agent appending a comment to this task while somebody types a title must not
 * take the title away from them.
 */
export function activeEdit(
  board: Board,
  task: TaskView | null,
  edit: FieldEdit | null,
): FieldEdit | null {
  if (edit === null || task === null) return null
  if (board.kind !== 'ready') return null
  if (!board.tasks.some((candidate) => candidate.id === task.id)) return null
  return isEditableField(edit.field) ? edit : null
}

/**
 * The ids an assignee `<select>` may offer, sorted, with the value it is already showing folded
 * in.
 *
 * A `<select>` whose current value is not among its options **silently shows the first one**, so
 * a task assigned to a role somebody deleted from `.cide/agents/` would render as *Unassigned*
 * and then reassign itself to nobody the moment the control was touched. Folding the id in is
 * what keeps the control honest about a roster and a board that are two reads of two files.
 *
 * Takes the **current assignee** rather than a task, because the compose dialog has no task and
 * needs the same list: a draft's assignee can only ever have come from these options, so nothing
 * is folded in there, but a second copy of the sorted-ids expression is how the dialog would
 * quietly keep the old list the day this rule grows a second clause.
 *
 * Here rather than in the component because it is that rule, not a list comprehension.
 */
export function assignableRoles(
  agent: string | null,
  roles: Readonly<Record<string, string>>,
): string[] {
  const ids = Object.keys(roles).sort()
  if (agent !== null && agent.trim() !== '' && !ids.includes(agent)) ids.push(agent)
  return ids
}

/**
 * Why the assignee list is short, as a sentence — or `null` when there is nothing to say.
 *
 * Rendered under the assignee `<select>`, whose options come from [`assignableRoles`]: with a
 * roster that is not `ready` that list is empty, and an empty dropdown with no sentence looks
 * exactly like the bug it used to be (the host passed `{}` for years of one milestone). Two
 * arms say something and two deliberately do not:
 *
 * - `'disabled'` restates `AgentsPanel/model.ts`'s `OFF_FOR_THIS_PROJECT` word for word — a
 *   deliberate second copy, `WORKING_PHASES`'s trade (this module is import-free so the check
 *   can compile it alone), pinned `===` against the original by `check:agents` — plus the
 *   clause naming where the switch is.
 * - `'empty'` names the file that would add a role, because that is the only action there is.
 * - `'ready'` has real options, and `'unknown'` means nobody has looked yet — the roster's own
 *   convention (see `AgentsPanel/model.ts`'s `Roster`) is that drawing nothing is the only
 *   honest rendering of not having looked, and a hint would claim knowledge this window does
 *   not have.
 *
 * The kind is an opaque `string` for [`RunRef.phase`]'s reason; an unrecognised one answers
 * `null`, the arm that claims nothing.
 */
export function assigneeHint(rosterKind: string): string | null {
  switch (rosterKind) {
    case 'disabled':
      return 'Subagents are off for this project. Turn them on in the Agents panel to assign tasks to roles.'
    case 'empty':
      return 'This project defines no roles yet — add one under .cide/agents/ or in the Agents panel.'
    default:
      return null
  }
}

/**
 * A draft from the assignee editor, back into `Assign`'s `agent`.
 *
 * The other half of [`fieldValue`]'s `agent ?? ''`, and here rather than in the component
 * because the two must agree: a `<select>` has no null, so the empty option *is* "unassigned",
 * and if these two disagreed then opening the assignee editor on an unassigned task and closing
 * it again would write `Assign(Some(""))` — an agent id of the empty string, in a committed file,
 * that no roster will ever match.
 *
 * A blank id collapses to `null` for the same reason [`roleLabel`] refuses one: a whitespace
 * agent id is not an assignment, and treating it as one puts a chip on a row that names nobody.
 */
export function assigneeFromDraft(draft: string): string | null {
  return draft.trim() === '' ? null : draft
}

/**
 * Is this field's value absent, so that [`restText`] is drawing a placeholder rather than the
 * task's own text?
 *
 * The card draws the placeholder dim, and the alternative — comparing `restText`'s answer
 * against the constant it would have returned for an empty task — is a comparison that passes
 * for a task whose title is *literally* `Untitled`, drawing a real title as if it were absent.
 * Asked of the value rather than of the rendering, there is nothing to get wrong.
 */
export function isFieldEmpty(task: TaskView, field: string): boolean {
  return fieldValue(task, field).trim() === ''
}

/* ------------------------------------------------------------------ composing a new task */

/**
 * The new task the user is filling in, before anything has been written. (M21)
 *
 * # Why this exists at all
 *
 * *New task* used to create the task and then open its card: a row titled `New task` appeared in
 * `.cide/tasks.json` on the click, and the four fields were edited one at a time afterwards,
 * each one its own write. Reported as *"i'm expecting that all fields are editable and task
 * isn't created while not press Create"*, and the report is right about more than taste — that
 * file is committed and shared with every agent in the project, so a mis-click put a row into
 * the team's tracker that had to be deleted rather than abandoned, and a run dispatched in
 * between could read a task whose title was still the placeholder.
 *
 * So the draft lives here, in the webview, until Create. Nothing on disk, nothing on the wire,
 * nothing any other window or agent can see. It is the one piece of task state that is
 * deliberately **not** in Rust, and it is allowed to be because it does not exist yet.
 *
 * `assignee` is the `<select>`'s own value and therefore a string, `''` meaning unassigned — the
 * same convention [`assigneeFromDraft`] exists to keep on the editing side, and the same
 * function converts it.
 */
export interface TaskDraft {
  title: string
  status: TaskStatus
  assignee: string
  body: string
}

/**
 * What the dialog opens on.
 *
 * `todo` and not "the filter the list happens to be on": a status the user did not choose is a
 * status they will not notice, and this one is written into a committed file. The filter's
 * interaction with a create is answered by [`filterAfterCreate`] instead, which changes what is
 * *shown* rather than what is written.
 */
export const EMPTY_DRAFT: TaskDraft = { title: '', status: 'todo', assignee: '', body: '' }

/**
 * May this draft be created?
 *
 * The title is the one required field — `TaskNew::title`'s own doc says a task without one is a
 * row nobody can act on, and `cide_tasks::validate` refuses it — so Create is inert until there
 * is one. Drawn **disabled** rather than not drawn at all, which is the opposite of the
 * `ready-queued` rule one panel over and for a reason worth stating: that rule is about a
 * control that can never work *in this state*, where the state is not the user's to change. This
 * one becomes live as soon as they type, and a Create button that materialises out of nowhere
 * mid-word is harder to understand than one that is visibly waiting.
 */
export function draftReady(draft: TaskDraft): boolean {
  return draft.title.trim() !== ''
}

/**
 * Has anything been typed into this draft?
 *
 * Compared field by field against [`EMPTY_DRAFT`] rather than by identity, because the dialog
 * rebuilds the object on every keystroke — and typing a character and deleting it again leaves a
 * draft that is `!==` the constant and means exactly the same thing.
 *
 * Whitespace counts as clean for the same reason [`isDirty`] trims: a stray space is not work
 * worth protecting, and treating it as dirty would make the scrim stop working for no visible
 * cause.
 */
export function draftDirty(draft: TaskDraft): boolean {
  return (
    draft.title.trim() !== '' ||
    draft.body.trim() !== '' ||
    draft.assignee !== '' ||
    draft.status !== EMPTY_DRAFT.status
  )
}

/**
 * How the compose dialog was asked to close, and whether it closes. (M21)
 *
 * **Cancel, the ✕ and Escape always discard**, which is the whole point of the dialog: the user
 * asked for a create that does not happen until Create, and a close that quietly wrote one would
 * be the same defect in a new place. Nothing is committed on the way out, and there is nothing
 * to commit — the draft never reached Rust.
 *
 * **The scrim is refused while the draft is dirty**, and that is the one asymmetry with
 * [`closeCard`]. There, leaving means keeping what you typed, so a stray click on the scrim
 * costs nothing. Here leaving means *discarding*, so the same stray click would take a
 * half-written body with it and there is no undo, no draft on disk and no way to tell it
 * happened. Three deliberate ways out remain and every one of them is visible or standard —
 * Cancel, the ✕, Escape — so the cost of ignoring one accidental gesture is a click that does
 * nothing, and the cost of honouring it is the user's paragraph.
 */
export function closeCompose(draft: TaskDraft, cause: CloseCause | 'cancel'): boolean {
  if (cause === 'dismiss' && draftDirty(draft)) return false
  return true
}

/**
 * The filter to be on once a task has been created into `status`.
 *
 * A create the user cannot see reads as a create that did not happen. Filter the list to
 * `Doing`, create a `Todo` task, and the row lands in a group the screen is not drawing — the
 * dialog closes, nothing appears, and the only evidence is a file they are not looking at.
 *
 * Cleared rather than moved to the new task's status: `All` is the state that shows the new row
 * **and** everything the user had chosen to look at, where switching the filter to `todo` would
 * answer one surprise by hiding whatever they were watching. A filter that already admits the
 * new task is left exactly as it is.
 */
export function filterAfterCreate(filter: StatusFilter, status: TaskStatus): StatusFilter {
  return filter === null || filter === status ? filter : null
}

/**
 * The query to be in the box once a task has been created from `draft` — [`filterAfterCreate`]
 * for the search, and for its reason: a create the user cannot see reads as a create that did
 * not happen, and a query the new row does not match is the same trapdoor as a filter it is
 * not in.
 *
 * Matched against the draft's **title and body only**, though [`matchesQuery`] also reads the
 * id: the id is minted in Rust after the write, so this function cannot know it, and the
 * conservative direction is to clear — a query kept on a guess would hide the row when the
 * guess was wrong, where a query cleared needlessly merely shows the user more than they asked
 * for, with the row they just created among it. A query the draft *does* match is kept, so a
 * user working through a themed backlog ("retry", create another retry task) is not stripped
 * of their narrowing every time they add to it.
 */
export function queryAfterCreate(query: string, draft: TaskDraft): string {
  const q = query.trim().toLowerCase()
  if (q === '') return query
  return hitsQuery(q, [draft.title, draft.body]) ? query : ''
}
