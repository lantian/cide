/**
 * The Agents panel's pure core: the run vocabulary, the role rows the panel is a list of, and
 * the one gate that decides whether a role can be dispatched. (M18)
 *
 * # Why this file imports nothing
 *
 * `ui/scripts/check-agents.mjs` compiles this module — and its twin in `TasksPanel/` — alone
 * with the TypeScript already in `node_modules` and then imports the emitted JS under node.
 * That works only while the module has no imports at all, not even a type-only one through the
 * `@/*` alias, which a bare `tsc` cannot resolve. `settings/theme.ts`, `chrome/sidebarView.ts`,
 * `chrome/sidebarWidth.ts` and `ProblemsPanel/model.ts` are all import-free for exactly this
 * reason; this project has no JS test runner and the panel must never have to be launched to
 * find out whether a button is offered.
 *
 * So the wire shapes below are **structural restatements** of `AgentRoster`, `AgentDef`,
 * `AgentRun` and `RunState` from `ui/src/ipc/generated.ts` rather than those types.
 * `sidebarWidth.ts` states the same trade for `SidebarSettings` and it is the same one here:
 * `adapt.ts` is the seam allowed to import the generated DTOs, it calls these functions with
 * values built from the real ones, and a field renamed on the Rust side therefore still fails
 * the build — one module further out. What a restatement *cannot* catch on its own is a
 * renamed enum **variant**, so `check-agents.mjs` reads `pub enum RunState` and `pub enum
 * Harness` out of `crates/cide-ipc/src/agents.rs` and pins [`RUN_PHASES`] and [`HARNESSES`]
 * against them as sets.
 *
 * # `number`, not `bigint`
 *
 * `startedUnixMs` and `RunState::Paused`'s `sinceUnixMs` are `u64` in Rust and therefore
 * **`bigint`** in `generated.ts`. Every millisecond field in this module is a `number`, and the
 * `Number(…)` conversion belongs to `adapt.ts` beside the rest of the wire translation.
 *
 * Two reasons, and the second is the one that bites. A `Number` holds an exact integer to 2^53,
 * which as Unix milliseconds runs to the year 287396, so nothing is lost. And mixing a `bigint`
 * with a `number` in arithmetic or a comparison throws a `TypeError` rather than coercing —
 * `Date.now() - run.startedUnixMs` would be a hard throw inside a render, and React 19 unmounts
 * the tree on a render that throws, so the window goes blank. One conversion at the seam beats
 * seven call sites remembering which side of it they are on.
 */

/* -------------------------------------------------------------------------- the harnesses */

/**
 * Which CLI actually runs a role. Restates `Harness` in `crates/cide-ipc/src/agents.rs`.
 *
 * Closed on purpose there and closed here: it selects an implementation, and a role naming a
 * harness this build does not have is a parse error with a line number in it, which is only
 * possible while the set has a definition.
 */
export type Harness = 'claude' | 'opencode'

/**
 * The harnesses, as data, so membership is a lookup rather than a type assertion.
 *
 * `check-agents.mjs` asserts this equals Rust's variant list as a set, so adding `Codex` there
 * fails the build here rather than rendering an empty harness column nobody notices.
 */
export const HARNESSES: readonly Harness[] = ['claude', 'opencode']

/** Is this one of the harnesses this build knows how to label? */
export function isHarness(value: string): value is Harness {
  return HARNESSES.includes(value as Harness)
}

/**
 * What the row prints for a harness.
 *
 * Falls back to the raw string rather than to a placeholder: a harness cide has not heard of is
 * still a fact about the run, and printing the value the backend sent is strictly more useful
 * than printing `unknown`. Empty is the only value that cannot be rendered, and it gets the
 * placeholder.
 */
export function harnessLabel(harness: string): string {
  if (harness === 'claude') return 'claude'
  if (harness === 'opencode') return 'opencode'
  return harness.trim() === '' ? 'unknown harness' : harness
}

/* ----------------------------------------------------------------------------- the phases */

/**
 * Where one run is — the tag of `RunState`, flattened.
 *
 * `RunState` is a tagged union whose payloads (`sinceUnixMs`, `code`, `reason`) belong to three
 * of its eight arms. The panel groups, glyphs and tones by the **tag** in every one of those
 * places, so the tag is lifted out into its own type and the payloads are carried beside it on
 * [`RunView`]. A row that had to `switch` on the union to find out which section it belongs in
 * would be several `switch`es that can disagree.
 *
 * `idle` is the one whose meaning is not obvious from its name and is worth carrying here:
 * **the turn is over and the child is alive**. It is not `finished` — nothing exited, no exit
 * code exists, and the run can be given another turn — and it is not `running`, because nothing
 * is being computed. `RunState::Idle`'s doc in `crates/cide-ipc/src/agents.rs` has the whole
 * argument, including why one phase here answers to two `SessionState` variants.
 *
 * The names are `RunState`'s own camelCase wire spellings, and `check-agents.mjs` pins them
 * against `pub enum RunState`.
 */
export type RunPhase =
  | 'queued'
  | 'starting'
  | 'running'
  | 'idle'
  | 'awaitingPermission'
  | 'paused'
  | 'finished'
  | 'failed'

/** The eight phases, in `RunState`'s declaration order. */
export const RUN_PHASES: readonly RunPhase[] = [
  'queued',
  'starting',
  'running',
  'idle',
  'awaitingPermission',
  'paused',
  'finished',
  'failed',
]

/**
 * Is this one of the eight phases?
 *
 * Takes `string` rather than `RunPhase` on purpose, for [`isSeverity`'s reason one panel over]:
 * every caller has a value *annotated* `RunPhase` by a wire type, and the whole point of the
 * guard is that the annotation is a promise from another process rather than a fact.
 *
 * `Array.includes` and never `value in SOME_TABLE`: `in` walks the prototype chain, so
 * `'constructor' in { queued: '○', … }` is `true` and the lookup hands back
 * `Object.prototype.constructor` — a function, which React refuses as a child and which
 * `className` stringifies into the whole source text of `Object`. That is not hypothetical
 * hardening; `check-problems.mjs` exists partly because it happened.
 */
export function isRunPhase(value: string): value is RunPhase {
  return RUN_PHASES.includes(value as RunPhase)
}

/**
 * Colour roles a phase can take. Each maps to one token in `AgentsPanel.module.css`.
 *
 * Six names for eight phases. `starting` and `running` share `busy`, because "the child is
 * coming up" and "a turn is in flight" are the same thing to look at and differ only in the
 * glyph. `queued` and `idle` share `idle`: both are runs that are quiet, and the row's glyph
 * and label are what separate "has not started" from "has stopped for now, and is still there".
 * A seventh tone for `idle` would have been a colour the user has to learn in order to read a
 * row that already says `Idle` in words. Everything else is its own tone, and `attention` is
 * deliberately separate from `error` even though the activity rail draws both loudly — a
 * permission prompt is a *request* and a failure is a *result*, and a user who has learned that
 * red means "something broke" must not be trained out of it by a run that is merely waiting for
 * an answer.
 */
export type Tone = 'idle' | 'busy' | 'attention' | 'paused' | 'done' | 'error'

/** Every tone a table below may return. Exported so the check can pin the tables to it. */
export const TONES: readonly Tone[] = ['idle', 'busy', 'attention', 'paused', 'done', 'error']

/*
 * The three tables, precisely typed as `Record<RunPhase, …>`.
 *
 * That precision is safe only because every read below goes through `isRunPhase` first. The
 * looser `Record<string, string | undefined>` was the alternative and it is a trap: an
 * `undefined` check does not catch `'constructor'`, whose lookup returns a function.
 */
const PHASE_GLYPH: Record<RunPhase, string> = {
  queued: '○',
  starting: '◌',
  running: '●',
  // A ringed dot rather than a hollow or a filled one: the child is *there* (the centre) but is
  // not turning (the ring). It has to be readable against both `queued`'s hollow ○ and
  // `running`'s solid ●, because those are the two states a glance at an idle row could
  // otherwise mistake it for, and they are the two opposite mistakes.
  idle: '◉',
  awaitingPermission: '◆',
  paused: '⏸',
  finished: '✓',
  failed: '✗',
}

const PHASE_LABEL: Record<RunPhase, string> = {
  queued: 'Queued',
  starting: 'Starting',
  running: 'Running',
  idle: 'Idle',
  awaitingPermission: 'Awaiting permission',
  paused: 'Paused',
  finished: 'Finished',
  failed: 'Failed',
}

const PHASE_TONE: Record<RunPhase, Tone> = {
  queued: 'idle',
  starting: 'busy',
  running: 'busy',
  idle: 'idle',
  awaitingPermission: 'attention',
  paused: 'paused',
  finished: 'done',
  failed: 'error',
}

/*
 * What an unrecognised phase gets. Constants rather than the raw value, because the raw value
 * may be the empty string, and an empty glyph is a row whose dot column silently collapses —
 * which reads as "this run has no state" rather than "cide does not know this state".
 */
const UNKNOWN_GLYPH = '?'
const UNKNOWN_LABEL = 'Unknown'
const UNKNOWN_TONE: Tone = 'idle'

/** The dot at the head of a run row. Never empty, for any input. */
export function phaseGlyph(phase: RunPhase): string {
  return isRunPhase(phase) ? PHASE_GLYPH[phase] : UNKNOWN_GLYPH
}

/** What the row says the run is doing. Never empty, for any input. */
export function phaseLabel(phase: RunPhase): string {
  return isRunPhase(phase) ? PHASE_LABEL[phase] : UNKNOWN_LABEL
}

/** Which colour role the dot takes. Never `undefined`, for any input. */
export function phaseTone(phase: RunPhase): Tone {
  return isRunPhase(phase) ? PHASE_TONE[phase] : UNKNOWN_TONE
}

/**
 * The phases in which a run **holds a slot** — its place under its role's concurrency ceiling,
 * and the worktree that role checked out for it — that is, **working**.
 *
 * *Working* is a claim about the **turn**, not about the process. A run is working while the
 * turn is in its hands: it was given one and has not handed anything back, whether it is
 * advancing that turn or frozen part-way through one. What that buys it is a pair of resources
 * — one slot against `AgentDefView.maxConcurrent`, one checkout no sibling run may enter — and
 * that pair, not the existence of a child, is what every caller of this list is asking after.
 *
 * # `idle` is out and `paused` is in, and that pair is why the name had to change
 *
 * `idle` is **out**. An idle run's **child is alive** — that is the whole content of
 * `RunState::Idle` — so under the old name, `LIVE_PHASES`, read plainly as "the child exists",
 * it belonged here. But the turn is *over*: the agent handed it back, nothing is being
 * computed, and the slot is released for the next dispatch. A live child is what stops the run
 * from being `finished`; it is not what makes it working, and this list is not about it.
 *
 * `paused` is **in**, and its child is doing strictly less than an idle one's — SIGSTOP, not a
 * single instruction. But it is frozen *mid-turn*: it has handed nothing back, and it still
 * holds its session, its place under the ceiling and its worktree. A pause that quietly freed a
 * slot would let a second run of the role start in the same checkout, which is the
 * file-clobbering that worktree isolation exists to prevent.
 *
 * So the two disagree in opposite directions — `idle` is alive and not working, `paused` is
 * working and not doing anything — which is exactly how "the child exists" and "the run holds a
 * slot" turned out to be two different lists that merely coincide on `running`. **This is the
 * second list, and it is now named for the second question.** Every caller wants that one:
 *
 *   - [`occupiedSlotsFor`] feeds [`canDispatch`]'s ceiling. An idle run that counted would hold
 *     its role's slot for as long as its child sat at a prompt, and the next queued task would
 *     never start. **That is the exact stall the wire's old `Finished { code: 0 }` mapping was
 *     written to avoid**, and dropping the fabricated exit code must not bring it back.
 *   - [`occupiedSlots`] feeds the panel header and, through `App.tsx`, the activity rail's
 *     badge: how much of this project's capacity is spoken for. Counting idle children would
 *     mean a figure that never returned to zero while one lingered, and a badge that never
 *     rests is a badge nobody reads. What that figure does **not** answer is "how many agents
 *     are getting work done" — see its own doc, because a paused project holds every slot it
 *     held a second ago while advancing nothing.
 *   - `TasksPanel`'s chip asks whether a task is still *claimed*: whether some run is mid-turn
 *     on it, inside the worktree it was dispatched into. Same question, same list, second copy.
 *
 * `queued` is out of it altogether. A queued run has no child at all (`AgentRun::session` is
 * `None` in that state, which is why the row draws no Open control) and is *waiting* for a slot
 * rather than holding one. Counting it would make [`canDispatch`] refuse the very dispatch that
 * would drain the queue.
 *
 * Keeping `idle` out preserves the queue behaviour and the chip behaviour the old
 * `Finished { code: 0 }` mapping produced, byte for byte, while the wire stops claiming a
 * process exited. That is the whole point of the split.
 *
 * `TasksPanel/model.ts` restates this list, because the two modules must not import each other
 * and neither may import a shared one. `check-agents.mjs` destructures **both copies by name**
 * and pins them equal, which is why the rename out of `LIVE_PHASES` had to happen in all three
 * files at once or not at all, and why it sat recorded in this comment for two milestones
 * before a change owned all three.
 */
export const WORKING_PHASES: readonly RunPhase[] = [
  'starting',
  'running',
  'awaitingPermission',
  'paused',
]

/**
 * Is this run working right now — is the turn in its hands, and the slot and worktree with it?
 *
 * `false` for `idle`, whose child is alive and has handed its turn back; `true` for `paused`,
 * whose child is frozen and has not. See [`WORKING_PHASES`] for why those two phases are the
 * pair that decides what this list means.
 */
export function isWorkingPhase(phase: string): boolean {
  return WORKING_PHASES.includes(phase as RunPhase)
}

/**
 * Has this run stopped for good? The two phases the Recent section collects.
 *
 * `idle` is **not** one of them, and the negation is where it matters: `RunRow` derives its Stop
 * control as `!isDonePhase(phase)`, so an idle run keeps its Stop button — which is the gesture
 * that ends it and hands its worktree back. A run that had been filed as done would have offered
 * no way to wind down the child that was still there.
 */
export function isDonePhase(phase: string): boolean {
  return phase === 'finished' || phase === 'failed'
}

/* ------------------------------------------------------------------------------ the views */

/**
 * One role, as the panel draws it. Structural restatement of `AgentDef`.
 *
 * `null` rather than `undefined` on the two optional fields, matching the wire exactly:
 * `Option<String>` serialises to `null`, and a model that accepted both spellings would be a
 * model with two ways to ask "is there a model override".
 */
export interface AgentDefView {
  /** The role name, which is also the file stem of `.cide/agents/<id>.md`. */
  id: string
  /** What the row says. Title-cased from the id by Rust unless the definition gives one. */
  label: string
  harness: Harness
  /** One line: what the role is *for*, in the author's own words. */
  description: string
  /** The role's system prompt, verbatim. Shown folded and read-only; never executed here. */
  systemPrompt: string
  /** A model alias or full name, or `null` for the harness's own default. */
  model: string | null
  /**
   * Why this role cannot be dispatched, as a sentence — or `null` when it can.
   *
   * This is `Command::unavailable` one layer down; see [`canDispatch`], which is the only thing
   * allowed to read it.
   */
  unavailable: string | null
  /** How many runs of this role may hold a slot at once. */
  maxConcurrent: number
}

/**
 * One dispatched execution, as the panel draws it. Structural restatement of `AgentRun`, with
 * `RunState` flattened into [`RunView.phase`] plus the three payload fields.
 */
export interface RunView {
  run: string
  agent: string
  /**
   * `AgentDef::label` **copied at dispatch, not looked up at render** — the wire's decision,
   * restated here so nobody "fixes" it by joining against the roster. A row that re-derives its
   * label renames itself when a config file changes and empties when a role is deleted, and a
   * row that renames itself is a row that lies about what happened.
   */
  agentLabel: string
  harness: Harness
  /**
   * The PTY session, once there is one. `null` while queued, which is what [`canOpen`] reads.
   * Also the join key for the phase dot: the store subscribes to `cide://session-state` on it.
   */
  session: string | null
  phase: RunPhase
  /** The task this run was dispatched against. `null` for an ad-hoc run. */
  task: string | null
  /** `AgentRun::startedUnixMs`, converted from `bigint` by `adapt.ts`. See the module header. */
  startedMs: number
  /** `RunState::Paused`'s `sinceUnixMs`. `null` in every other phase. */
  pausedSinceMs: number | null
  /** `RunState::Finished`'s `code`. `null` in every other phase. */
  exitCode: number | null
  /** `RunState::Failed`'s `reason`. `null` in every other phase. */
  failure: string | null
  /** The run was frozen long enough that its in-flight model request may have timed out. */
  staleTurn: boolean
  /** One line of extra context — what the queue is waiting on, which worktree this run holds. */
  note: string | null
}

/**
 * What the panel knows about this project's subagents right now.
 *
 * The wire's `AgentRoster` has three arms; this has **four**, and the extra one is the point.
 * `agents.roster` is called from a render effect through `pendingCommand` with a `null`
 * fallback, so there is a real interval — every open of the panel, and the whole of it on a
 * build without the handler — in which nobody has looked yet. Rendering `Disabled`'s designed
 * prose during that interval would tell every user that subagents are off for a project that
 * has them on, one frame before the truth arrives; rendering `Ready` with no runs would be the
 * confident empty list `ProblemsPanel` was written against. So "nobody has looked" is a state,
 * not the absence of one, and [`ROSTER_UNKNOWN`] is its value.
 */
export type Roster =
  | { kind: 'unknown' }
  | { kind: 'disabled'; hint: string; configPath: string }
  | { kind: 'empty'; configPath: string }
  | { kind: 'ready'; agents: readonly AgentDefView[]; runs: readonly RunView[]; dispatching: boolean }

/** The roster before anything has answered. See [`Roster`] for why this is not `Disabled`. */
export const ROSTER_UNKNOWN: Roster = { kind: 'unknown' }

/* ------------------------------------------------------------------------------ the gate */

/** A green light, or the sentence saying why not. Never both, never neither. */
export type Dispatchable = { ok: true } | { ok: false; reason: string }

/** The sentence a roster that is not `ready` refuses with. */
export const OFF_FOR_THIS_PROJECT = 'Subagents are off for this project.'

/**
 * The sentence a roster nobody has read yet refuses with.
 *
 * Deliberately *not* [`OFF_FOR_THIS_PROJECT`]. This branch is only reachable from a stale
 * `AgentDefView` held across a roster change — no def can be drawn from an `unknown` roster in
 * the first place — but if it is ever shown, telling the user a feature is off when the truth
 * is that cide has not looked is the confident-empty-list failure in sentence form. Two states,
 * two sentences, for the same reason the roster has two arms.
 */
export const ROSTER_NOT_READ = 'cide has not read this project’s agent roster yet.'

/**
 * Something to call a role that has no usable label.
 *
 * Every sentence below interpolates a name, and a name that is the empty string produces
 * `" already has 2 running."` — a sentence with a hole where the subject was. The fallback
 * ladder is label → id → this.
 */
function roleName(def: AgentDefView): string {
  if (def.label.trim() !== '') return def.label
  if (def.id.trim() !== '') return def.id
  return 'This role'
}

/**
 * How many of this role's slots are held right now.
 *
 * Counted over [`WORKING_PHASES`], so a queued run of the same role does not block the dispatch
 * that would start it, and a paused one does — a frozen run has not let go of the checkout, and
 * starting a second run of the role into it is the collision worktree isolation prevents.
 */
export function occupiedSlotsFor(roster: Roster, agent: string): number {
  if (roster.kind !== 'ready') return 0
  let n = 0
  for (const run of roster.runs) {
    if (run.agent === agent && isWorkingPhase(run.phase)) n += 1
  }
  return n
}

/**
 * **Can this role be dispatched, and if not, what is the sentence?**
 *
 * This is `cide_core::commands`' `Command::unavailable` one layer down, and for the identical
 * reason. A role that is listed and silently inert is the state this project has paid for
 * twenty-four times — the registry once declared 42 commands against 7 dispatcher arms, so 35
 * palette rows did nothing at all when picked, and nothing anywhere failed. The answer there is
 * the answer here: make it unrepresentable. A greyed control with no sentence is a dead control
 * wearing grey, so the return type has no third shape: **a green light or a non-empty sentence,
 * never neither, and `check-agents.mjs` fails if it can be both.**
 *
 * The panel draws exactly one of the two — a dispatchable row carries the button, a refused row
 * carries the sentence — and never both.
 *
 * The order of the tests is the order of the answers a user would want: the role's own reason
 * first because it is the most specific and it is the one somebody wrote by hand, then the
 * project switch, then the queue, then the role's ceiling.
 */
export function canDispatch(def: AgentDefView, roster: Roster): Dispatchable {
  if (def.unavailable !== null) {
    // A reason that is present but blank would return `{ ok: false, reason: '' }` — the exact
    // "neither" state this function exists to make impossible — so an empty one is replaced
    // rather than passed through. The role stays refused either way: an unavailable marker cide
    // cannot read is not evidence that the role works.
    const reason = def.unavailable.trim()
    return { ok: false, reason: reason === '' ? `${roleName(def)} is unavailable.` : def.unavailable }
  }
  if (roster.kind !== 'ready') {
    return { ok: false, reason: roster.kind === 'unknown' ? ROSTER_NOT_READ : OFF_FOR_THIS_PROJECT }
  }
  if (!roster.dispatching) {
    return { ok: false, reason: 'The dispatch queue is paused.' }
  }
  // `<= 0` and a finiteness test rather than `=== 0`: `max-concurrent` comes out of a markdown
  // file the user writes, and a `NaN` from a malformed one would make every `n >= max`
  // comparison below `false` — a role with an unreadable ceiling would become a role with no
  // ceiling, which is the direction that spawns processes.
  if (!Number.isFinite(def.maxConcurrent) || def.maxConcurrent <= 0) {
    return { ok: false, reason: `${roleName(def)} is configured with no concurrency.` }
  }
  const held = occupiedSlotsFor(roster, def.id)
  if (held >= def.maxConcurrent) {
    return { ok: false, reason: `${roleName(def)} already has ${held} running.` }
  }
  return { ok: true }
}

/**
 * Is there a transcript to attach a pane to?
 *
 * The session id alone is the answer — a queued run has none, which is exactly why the Open
 * control is *withheld* from a queued row rather than drawn disabled. The phase test is
 * belt-and-braces against a backend that fills `session` before the child reports in: a mirror
 * of a session with no PTY behind it is an empty pane the user then has to close.
 *
 * Deliberately **true for a finished run**: `PtySession` feeds its `vt100` mirror independently
 * of sinks, so a run that ran headless for twenty minutes has its screen waiting, and reading
 * what an agent did is most of the reason to open one at all.
 *
 * **True for an idle run, which is the best case there is.** Its turn is over, so the transcript
 * is complete, *and* its child is alive, so the pane is interactive rather than a screenshot of
 * one: the user can read what the agent did and then type the next turn into it themselves. This
 * needs no arm of its own — `session` is set and the phase is not `queued` — but it is the case
 * the function most exists for, so it is written down.
 */
export function canOpen(run: RunView): boolean {
  return run.session !== null && run.phase !== 'queued'
}

/**
 * Is there a child to freeze?
 *
 * Not `queued` — there is no process, and the queue's own pause is the project-scope control,
 * not this one. Not `paused` — the row draws Resume there instead. Not the two terminal phases.
 *
 * **Not `idle`, and that is a decision rather than an omission.** There is a real child there,
 * so `SIGSTOP` would work; it is what the freeze would *buy* that is empty. An idle run is
 * spending no tokens, holding no file open and not going to move until something writes to its
 * stdin, so the only observable effects of pausing one are a colour change and a Resume the user
 * now has to press before the follow-up can be delivered — a control whose entire contribution
 * is a second step in a gesture that had one.
 *
 * The stale-turn machinery makes the same point from the other side: [`staleTurnLine`] exists
 * because a freeze can time out an **in-flight model request**, and an idle run has none, so the
 * one genuine hazard of pausing is the one thing pausing an idle run cannot even risk. What the
 * user actually wants at an idle row is Open (read it, or take the next turn by hand) or Stop
 * (wind it down and hand back the worktree), and `RunRow` draws both.
 */
export function canPause(run: RunView): boolean {
  return run.phase === 'starting' || run.phase === 'running' || run.phase === 'awaitingPermission'
}

/* --------------------------------------------------------------------------- the counters */

/**
 * How many of this project's slots are held right now, or `null` when nobody has looked.
 *
 * **What it answers, exactly: how much of the project's capacity is spoken for** — one per run
 * in [`WORKING_PHASES`]. It is emphatically **not** "how many agents are working" in the sense
 * a user reads off a bare digit. `Pause all` freezes every run in the project, and every one of
 * them stays in `paused`, so this figure reads `2` while nothing advances by a single token.
 * That is the right answer to the question being asked — the two slots and two worktrees really
 * are still held, and a dispatch really would still be refused — and the wrong answer to the
 * question a lone number invites, so the name says slots and not work.
 *
 * # One number, two surfaces, and only one of them mitigates the paused reading
 *
 * [`metaFigure`] wants occupancy and gets it: the header prints this beside the queue depth and
 * beside the Resume control, so a frozen project reads `2 ▶ Resume` and cannot be misread — the
 * digit and the reason it is not moving are adjacent. **That mitigation is structural and it
 * holds only while the two stay side by side**; move Resume out of the header, or print the
 * figure somewhere Resume is not, and the misreading is back.
 *
 * The activity rail's ⌬ badge takes the same number through `App.tsx` and has **no Resume
 * beside it**: it draws a `2` pill whose accessible name is "2 runs", with nothing on the rail
 * saying they are frozen. So one number serves both surfaces arithmetically, and only one of
 * them says what the number means. Splitting them — the rail wants "is anything moving here",
 * the header wants "how loaded is this project" — is a change to what `App.tsx` calls, and is
 * the follow-up this rename deliberately did not reach into.
 *
 * **`null` and `0` are different claims and must not be conflated**, which is `ProblemsPanel`'s
 * badge rule and the reason its `statusBarCounts` returns `null` rather than zeroes. The
 * activity rail draws nothing for `null` — *unknown* — and nothing for `0` — *looked, idle* —
 * so the two look alike on the rail, but they are not alike anywhere a number is printed, and
 * the moment one surface prints `0` for "unknown" the panel is making a claim nothing checked.
 *
 * Every arm but `ready` returns `null`, including `disabled`: only `ready` carries a `runs`
 * array, so only `ready` is a state in which anything counted.
 *
 * An **idle** run is not counted — [`WORKING_PHASES`] excludes it — so this figure can read `0`
 * while a role row on screen says `Idle` and offers an Open. That is the intended reading and
 * not a hole: every slot really is free, the queue really can move, and the row says in a word
 * what the number is not counting.
 */
export function occupiedSlots(roster: Roster): number | null {
  if (roster.kind !== 'ready') return null
  let n = 0
  for (const run of roster.runs) if (isWorkingPhase(run.phase)) n += 1
  return n
}

/**
 * The old spelling of [`occupiedSlots`], and **the same function**, not a second answer.
 *
 * It exists only because `ui/src/App.tsx` imports this name for the activity rail's badge, and
 * that file is outside the change that renamed the rest: renaming an export whose one caller
 * you may not edit trades a misleading name for a broken build. `check-agents.mjs` asserts the
 * two are the identical function object, so this can never drift into a parallel
 * implementation, and retiring it is a two-line follow-up — the import and the call site.
 */
export const liveCount = occupiedSlots

/**
 * How many runs are waiting for a slot, or `null` when nobody has looked. See [`occupiedSlots`]
 * for the `null`-is-not-`0` rule, which is the same here.
 */
export function queuedCount(roster: Roster): number | null {
  if (roster.kind !== 'ready') return null
  let n = 0
  for (const run of roster.runs) if (run.phase === 'queued') n += 1
  return n
}

/**
 * The header's right-aligned meta figure, or `null` to draw nothing at all.
 *
 * `null` rather than `ProblemsPanel`'s `'—'` because this header has two panels beside it and
 * an em dash in every one of them is three claims of ignorance where the honest render is an
 * empty slot. The panel prints whatever comes back verbatim.
 *
 * The queue depth is appended after a `+` when there is one: a header reading `0` while five
 * runs wait to start is the same quiet lie as an unchecked zero, one field over.
 */
export function metaFigure(roster: Roster): string | null {
  const held = occupiedSlots(roster)
  const queued = queuedCount(roster)
  if (held === null || queued === null) return null
  return queued === 0 ? String(held) : `${held}+${queued}`
}

/* --------------------------------------------------------------------------- the sections */

/**
 * The two groups the panel draws, in the order it draws them.
 *
 * # Why this used to be five, and why one of them is now the whole panel
 *
 * It was `running | idle | queued | roles | recent` — a **run**-centric view, and the user who
 * ran it said what was wrong with it in one sentence: they wanted to see *their subagents and
 * what each is doing*, and instead got a list of runs in which a role with nothing running
 * appeared only under `Roles`, a role with one run appeared twice, and no row anywhere answered
 * "what is `qa` doing right now" without reading four sections and joining them by name.
 *
 * So the list is now **one row per role**, and the run vocabulary — every phase, glyph, label
 * and tone above — is what a role row uses to say what it is doing. Nothing was deleted from
 * that vocabulary: a role's state *is* the state of its runs, and [`roleRow`] is the join that
 * was previously left to the reader.
 *
 * `recent` survives, and the argument for it is stronger than it was rather than weaker. A
 * finished run is not something a subagent is doing, so it must not appear on a role row — the
 * user's rule is that a role with nothing live offers no Open at all — and with the four run
 * sections gone this is the only place a finished transcript is reachable from. `PtySession`
 * feeds its vt100 mirror whether or not anything is attached, so a run that worked headless for
 * twenty minutes still has its screen waiting, and reading what an agent did after it finished
 * is most of the reason to open one. Dropping the section would not tidy the panel; it would
 * delete the only route to that screen.
 */
export type SectionKind = 'agents' | 'recent'

/** One run row, with everything the view needs already decided. */
export interface RunRow {
  run: RunView
  glyph: string
  label: string
  tone: Tone
  /**
   * The title of [`RunView.task`], when the Tasks board knows it.
   *
   * `null` covers both "this run has no task" and "the board has not been read", and the row
   * draws `no task` in dim for either — a run nothing can account for is worth seeing, which is
   * why it is not simply omitted.
   */
  taskTitle: string | null
  canOpen: boolean
  canPause: boolean
  /** The stale-turn bar's sentence, or `null`. See [`staleTurnLine`]. */
  staleTurn: string | null
}

/**
 * **The phases a role row treats as "this subagent is doing something", most demanding first.**
 *
 * This is the order the summary line of a role with several runs is decided by, and it is a
 * claim about *which of them the user needs to know about first* rather than about which
 * started first.
 *
 *   - `awaitingPermission` leads because it is the only phase that is **blocked on the user**.
 *     A role that has one run waiting for an answer and another quietly working is a role whose
 *     summary must say *awaiting permission*; the reverse summary hides the one row where a
 *     human is the bottleneck. `TasksPanel`'s `agentChip` breaks its own tie the same way, in
 *     as many words — "the run that is a call to action wins over the one that merely came
 *     first" — and two panels answering that question differently is how a user learns to
 *     distrust both.
 *   - `running` then `starting`: both are working, and one of them has a child that has reported
 *     in. A summary saying *starting* over a run that is mid-turn understates the role.
 *   - `paused` is above `idle` even though it is doing strictly less, because it is *frozen
 *     mid-turn* — it holds a slot and a worktree, and it needs a gesture (Resume) before
 *     anything moves again. An idle run needs nothing. See [`WORKING_PHASES`], which draws the
 *     same distinction for the same reason.
 *   - `queued` is last and is still on the list: it has no child, so [`canOpen`] withholds Open
 *     from it, but "waiting for a slot" is unquestionably something the role is doing and a role
 *     whose only run is queued must not read *Not running*.
 *
 * `finished` and `failed` are **deliberately absent**, and that absence is the user's rule made
 * structural: a role whose runs have all ended is doing nothing, its row draws no Open, and its
 * runs are in Recent.
 *
 * This list is the **order**, not the membership. [`isActivePhase`] is the membership, and it is
 * deliberately wider — see its doc for the phase this list cannot name.
 */
export const ACTIVE_PHASES: readonly RunPhase[] = [
  'awaitingPermission',
  'running',
  'starting',
  'paused',
  'idle',
  'queued',
]

/**
 * Is this run one of the role's *current* ones — one the role row draws a line for?
 *
 * **The negation of [`isDonePhase`], and not membership of [`ACTIVE_PHASES`].** The two differ on
 * exactly one input and it is the one that matters: a phase from a cide newer than this one.
 *
 * `ACTIVE_PHASES` is a list of six names, so a seventh phase is not on it — and if this asked
 * that list, such a run would be **neither active nor done**, would appear under no role and in
 * no Recent, and would be a `claude` spending the user's quota with no row anywhere on screen. A
 * membership test that can drop a row is the failure `check-problems.mjs` exists for; the old
 * five-section layout avoided it by making Running the fall-through, and this is the same
 * decision in the shape the panel now has.
 *
 * So an unreadable phase draws a line, with the `?` glyph and the `Unknown` label the tables
 * already provide, sorted **after** every phase this build can read — see [`byUrgency`]. Last,
 * because a value nothing could parse must not outrank a run that is genuinely awaiting
 * permission when the role's one-word summary is decided; drawn, because it might be a running
 * agent.
 */
export function isActivePhase(phase: string): boolean {
  return !isDonePhase(phase)
}

/**
 * What a role with nothing running shows in the three places a run would have filled.
 *
 * A glyph, because the column is fixed-width and an empty cell reads as a rendering fault rather
 * than as *nothing is happening*; the quietest mark available, because that is what nothing
 * should look like beside `●` and `◆`. The words are the ones the user's own sentence used.
 */
export const RESTING_GLYPH = '·'
export const RESTING_LABEL = 'Not running'
export const RESTING_TONE: Tone = 'idle'

/**
 * The sentence a role that only exists because a run named it is refused with.
 *
 * See [`sections`] on orphan runs. It is phrased as the fact it is — the roster has no
 * definition under this id — rather than as an accusation, because the ordinary way to reach it
 * is to edit `.cide/agents/` while something is running, which is a reasonable thing to do.
 */
export const ROLE_UNDEFINED = 'No definition for this role in .cide/agents/ any more.'

/**
 * One role, and everything about it the panel draws. **The panel's only list.**
 *
 * A row is a *subagent*, not a run: its identity is the definition, and its state is derived
 * from whichever of its runs are active. Both facts are on it, already decided, because the two
 * places that would otherwise re-derive them — the summary line and the Open control — are the
 * pair that must never disagree.
 */
export interface RoleRow {
  def: AgentDefView
  /** The answer, whole: the view renders the button or the sentence, never both. */
  dispatch: Dispatchable
  /**
   * How many runs of this role hold a slot, for the `2/3` figure on the row. See
   * [`occupiedSlotsFor`], which computes it.
   *
   * Keeps the shorter name because `AgentsPanel.tsx` renders it as `role.live`; the field is a
   * view contract rather than part of the phase vocabulary, and the two were renamed by
   * different changes.
   */
  live: number
  /** How many are waiting. */
  queued: number
  /**
   * **What this subagent is doing right now**, one entry per active run, most demanding first.
   *
   * Empty for a role with nothing live — which is the state the user asked to be able to see at
   * a glance, and the state in which [`canOpen`] below is false and the view draws no Open
   * control at all.
   *
   * # Why every active run is listed rather than only the first
   *
   * `cide_agents::effective_max_concurrent` clamps a role to **one** working run whenever the
   * project uses worktree isolation, which is the default: one worktree per *agent*, not per
   * run, so a second concurrent run of a role would be two processes editing one checkout. So
   * for almost every project this list has 0 or 1 entry and the question does not arise.
   *
   * It arises under `isolation: shared`, where a role's own `max-concurrent` stands — and there
   * the honest answer is to draw them all. The alternative considered was one summary line plus
   * an "and 2 more" count, and it fails the panel's own standing rule: a run that is on screen
   * nowhere is a `claude` spending the user's quota that they cannot see, cannot open and cannot
   * stop. An idle run makes it reachable in the default configuration too — `idle` is outside
   * [`WORKING_PHASES`], so it holds no slot and a second run of the same role may start beside
   * it, which is two entries here under worktree isolation and no misconfiguration at all.
   *
   * The **order** is [`ACTIVE_PHASES`], then the oldest run first, then the run id: a total
   * order, for `groupByFile`'s reason one panel over — a partial one leaves two rows free to
   * swap places on re-render, and a list that reshuffles under the pointer is unusable.
   */
  runs: readonly RunRow[]
  /**
   * The summary: the glyph, words and tone of `runs[0]`, or the resting triple when there are
   * none.
   *
   * Three fields rather than an optional `RunRow`, because the row draws them unconditionally
   * and a view that had to ask "is there a run" a second time to find its own glyph would be
   * the second answer this interface exists to prevent.
   */
  glyph: string
  status: string
  tone: Tone
  /**
   * **Is there anything of this role's to look at?** The Open control's whole condition.
   *
   * `runs.some(canOpen)` and nothing else — not a second reading of the phase. The user's rule
   * is that a subagent doing nothing offers no way to open it, and that rule is this field: with
   * no active run it is `false`, and `AgentsPanel.tsx` draws no Open element anywhere in the
   * row. Withheld, never disabled — a disabled control promises that some reachable condition
   * would make it work, and for a role with nothing running there is none; the control that
   * changes that is Dispatch, which is on the same row.
   *
   * It is `false` for a role whose only run is `queued`, which has no session to mirror, and it
   * is `true` for an idle one, whose child is alive with a complete transcript — see
   * [`canOpen`], which is the single function both this and the per-run control read.
   */
  canOpen: boolean
}

/** A section of the panel. Discriminated so the view cannot render roles as runs. */
export type Section =
  | { kind: 'agents'; label: string; rows: readonly RoleRow[] }
  | { kind: 'recent'; label: string; rows: readonly RunRow[] }

/**
 * How many finished runs the Recent section keeps.
 *
 * A cap rather than the whole history because the registry's is unbounded within a session and
 * a sidebar at 320px that grows a row per dispatch becomes a scroll nobody reads. Twenty is
 * about a screen and a half — enough to answer "what happened this afternoon" without becoming
 * the panel's main content.
 */
export const RECENT_CAP = 20

/**
 * Look a task title up in a plain object safely.
 *
 * `Object.hasOwn` and not `titles[id] ?? null`: the map is keyed by task ids that come from a
 * committed JSON file, and `titles['constructor']` on a bare object literal returns a function.
 * A function reaching JSX is not a valid React child; a function reaching `className`
 * stringifies into the source text of `Object`.
 */
function titleOf(titles: Readonly<Record<string, string>>, id: string | null): string | null {
  if (id === null) return null
  if (!Object.hasOwn(titles, id)) return null
  const title = titles[id]
  if (typeof title !== 'string' || title.trim() === '') return null
  return title
}

function runRow(run: RunView, titles: Readonly<Record<string, string>>): RunRow {
  return {
    run,
    glyph: phaseGlyph(run.phase),
    label: phaseLabel(run.phase),
    tone: phaseTone(run.phase),
    taskTitle: titleOf(titles, run.task),
    canOpen: canOpen(run),
    canPause: canPause(run),
    staleTurn: staleTurnLine(run),
  }
}

/**
 * Where a phase sits in the order, with the unreadable ones **last**.
 *
 * `indexOf` over [`ACTIVE_PHASES`] rather than a lookup table, because the list is six long and a
 * table would be a second place the order is written down. The `-1` is the whole reason this is
 * a function: a phase this build cannot read reaches here — [`isActivePhase`] admits it on
 * purpose — and raw `indexOf` would sort it *first*, putting `?` and `Unknown` in a role's
 * summary over a run that is genuinely awaiting permission.
 */
function rank(phase: RunPhase): number {
  const at = ACTIVE_PHASES.indexOf(phase)
  return at === -1 ? ACTIVE_PHASES.length : at
}

/**
 * The total order the runs under one role are drawn in. See [`RoleRow.runs`].
 */
function byUrgency(a: RunView, b: RunView): number {
  const byPhase = rank(a.phase) - rank(b.phase)
  if (byPhase !== 0) return byPhase
  // Oldest first: a run that has been going twenty minutes is the one a reader is asking about,
  // and it is the entry whose position does not move as new runs arrive beside it.
  if (a.startedMs !== b.startedMs) return a.startedMs - b.startedMs
  return a.run < b.run ? -1 : a.run > b.run ? 1 : 0
}

/**
 * A role the roster does not define, invented from a run that names it.
 *
 * See [`sections`] for when this happens and why the row has to exist. Everything that can be
 * read off the run is; everything else is empty, and `unavailable` carries [`ROLE_UNDEFINED`]
 * so [`canDispatch`] refuses a second run of a role whose definition has gone — with a sentence,
 * which is what the refusal has to have.
 *
 * `maxConcurrent: 0` is not a guess dressed up as a fact: nothing on the wire says what the
 * ceiling of a deleted role was, and 0 is the value [`canDispatch`] already refuses. The figure
 * the row prints from it — `1/0` — reads as the anomaly it is.
 */
function undefinedRole(run: RunView): AgentDefView {
  return {
    id: run.agent,
    // The label copied at dispatch, which is the only name anything still has for this role.
    label: run.agentLabel.trim() === '' ? run.agent : run.agentLabel,
    harness: run.harness,
    description: '',
    systemPrompt: '',
    model: null,
    unavailable: ROLE_UNDEFINED,
    maxConcurrent: 0,
  }
}

/** One role row: the gate, the counts, the active runs, and the summary they decide. */
function roleRow(
  def: AgentDefView,
  roster: Roster,
  runs: readonly RunView[],
  titles: Readonly<Record<string, string>>,
): RoleRow {
  const active = runs
    .filter((run) => run.agent === def.id && isActivePhase(run.phase))
    .sort(byUrgency)
    .map((run) => runRow(run, titles))
  const first = active[0]
  return {
    def,
    dispatch: canDispatch(def, roster),
    live: occupiedSlotsFor(roster, def.id),
    queued: runs.filter((run) => run.agent === def.id && run.phase === 'queued').length,
    runs: active,
    glyph: first === undefined ? RESTING_GLYPH : first.glyph,
    status: first === undefined ? RESTING_LABEL : first.label,
    tone: first === undefined ? RESTING_TONE : first.tone,
    /*
     * Derived from the rows that are already here rather than from the phases again. One
     * expression, so "the role offers Open" and "one of these lines offers Open" cannot become
     * two answers — and the second is what the view actually draws its buttons from.
     */
    canOpen: active.some((row) => row.canOpen),
  }
}

/**
 * The panel's body: **Subagents**, then **Recent** — whichever have anything in them.
 *
 * # One row per role, and every role gets one
 *
 * `roster.agents` is the list, in the order Rust sent it, and each row carries its own runs.
 * Nothing is grouped by phase any more; see [`SectionKind`] for what the user said about the
 * five groups this replaced. A role that cannot be dispatched still gets a row — `unavailable`
 * is a fact about the machine, and a role that vanished from the panel when its harness was
 * uninstalled would leave the user nothing to read the reason off.
 *
 * # The orphan rows, which are the only rows this function invents
 *
 * A run's `agent` is an id, and the roster's `agents` is whatever `.cide/agents/` held when Rust
 * last read it. Those two disagree the moment somebody deletes or renames a role file while a
 * run of it is in flight — the registry still has the run, the roster no longer has the
 * definition, and a list built only from `roster.agents` would **drop that run off the panel
 * entirely** while its `claude` kept working and kept billing. That is the failure this
 * repository names most often, arrived at from a new direction, so every *active* run whose role
 * the roster does not define gets a row built from the run itself ([`undefinedRole`]).
 *
 * They come after the defined roles, in the order their first run does, so the list a user
 * recognises is not reordered by an anomaly. A run that has *ended* under a deleted role gets no
 * row — it is in Recent, which draws the label the run carried at dispatch and needs no
 * definition at all.
 *
 * # Empty sections are omitted, uniformly
 *
 * A `Recent` heading over nothing is a heading that answers no question. The states that
 * genuinely need prose are the roster's three non-`ready` arms, and those return `[]` here so
 * the panel draws its designed screen instead of a list of empty headings — including `empty`,
 * whose screen is now a single centred button into Settings.
 *
 * `Recent` is re-ordered, newest first, because the answer wanted there is "what just happened"
 * rather than "what happened first". The sort is total — `startedMs` then the run id — for
 * `groupByFile`'s reason one panel over: a partial order leaves rows free to swap places on
 * re-render when two runs share a millisecond, and a list that reshuffles under the pointer is
 * unusable.
 */
export function sections(roster: Roster, taskTitles: Readonly<Record<string, string>>): Section[] {
  if (roster.kind !== 'ready') return []

  const roles: RoleRow[] = roster.agents.map((def) => roleRow(def, roster, roster.runs, taskTitles))

  /*
   * The orphans, found by asking the roster rather than by trusting the run: `defined` is built
   * from `roster.agents` and consulted with `Set.has`, which — unlike `id in {}` — cannot be
   * satisfied by `'constructor'`. A rogue id reaching this test would otherwise be filed as a
   * defined role and then match no row.
   */
  const defined = new Set(roster.agents.map((def) => def.id))
  const orphans: string[] = []
  for (const run of [...roster.runs].sort(byUrgency)) {
    if (!isActivePhase(run.phase)) continue
    if (defined.has(run.agent) || orphans.includes(run.agent)) continue
    orphans.push(run.agent)
    roles.push(roleRow(undefinedRole(run), roster, roster.runs, taskTitles))
  }

  const recent = roster.runs.filter((run) => isDonePhase(run.phase))
  recent.sort((a, b) => b.startedMs - a.startedMs || (a.run < b.run ? 1 : a.run > b.run ? -1 : 0))

  const out: Section[] = []
  /*
   * "Subagents" and not "Roles". *Role* is the word the config file uses and the word the
   * refusal sentences use; *subagent* is the word the user used for the thing they wanted to
   * see, and this heading sits over the answer to their question rather than over a directory
   * listing.
   */
  if (roles.length > 0) out.push({ kind: 'agents', label: 'Subagents', rows: roles })
  if (recent.length > 0) {
    out.push({
      kind: 'recent',
      label: 'Recent',
      rows: recent.slice(0, RECENT_CAP).map((run) => runRow(run, taskTitles)),
    })
  }
  return out
}

/* ---------------------------------------------------------------------------- the figures */

/**
 * How long a run has been going, as `12s` / `4m` / `1h 04m`.
 *
 * **`nowMs` is a parameter and this function never calls `Date.now()`.** Two things follow from
 * that and both are load-bearing: the panel is rendered through `react-dom/server` by the
 * render check, where a clock read makes the digest change between runs and the whole check
 * useless; and a time-dependent figure is otherwise untestable, so the one thing that can be
 * wrong here — the rollover from minutes to hours — would have no gate. Views read `nowMs` as a
 * prop for the same reason.
 *
 * A clock that has gone backwards (an NTP step, a suspended laptop, a `startedMs` stamped by a
 * machine whose clock is ahead) yields `0s` rather than a negative figure. A non-finite input
 * yields an em dash: `NaN` formatted into a row prints `NaNs`, which reads as a broken app
 * rather than as a missing number.
 */
export function elapsed(nowMs: number, startedMs: number): string {
  if (!Number.isFinite(nowMs) || !Number.isFinite(startedMs)) return '—'
  const ms = Math.max(0, nowMs - startedMs)
  const totalSeconds = Math.floor(ms / 1000)
  if (totalSeconds < 60) return `${totalSeconds}s`
  const totalMinutes = Math.floor(totalSeconds / 60)
  if (totalMinutes < 60) return `${totalMinutes}m`
  const hours = Math.floor(totalMinutes / 60)
  const minutes = totalMinutes % 60
  // Zero-padded so `1h 04m` and `1h 40m` are the same width and a column of them does not
  // jitter as the minutes tick over.
  return `${hours}h ${minutes < 10 ? '0' : ''}${minutes}m`
}

/**
 * The stale-turn bar's sentence, or `null` when there is nothing to say.
 *
 * A *suspicion*, phrased as one. cide cannot see the model request — only a process it stopped
 * and continued — so the bar offers Retry and Leave it rather than re-dispatching, because a
 * re-dispatch of a turn that in fact survived double-bills it. The wording says "may have" for
 * that reason and should stay hedged.
 *
 * Lives here rather than in `chrome/notices.ts` because a toast has no action affordance and
 * the decision has to stay attached to the run it is about — a notice that outlives its row is
 * a question about nothing.
 */
export function staleTurnLine(run: RunView): string | null {
  if (!run.staleTurn) return null
  return 'This turn may have timed out while paused.'
}
