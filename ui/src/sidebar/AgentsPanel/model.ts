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
export type Harness = 'claude' | 'opencode' | 'qwen' | 'codex' | 'mimo'

/**
 * The harnesses, as data, so membership is a lookup rather than a type assertion.
 *
 * `check-agents.mjs` asserts this equals Rust's variant list as a set, so adding `Codex` there
 * fails the build here rather than rendering an empty harness column nobody notices.
 */
export const HARNESSES: readonly Harness[] = ['claude', 'opencode', 'qwen', 'codex', 'mimo']

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
  if (harness === 'qwen') return 'qwen'
  if (harness === 'codex') return 'codex'
  if (harness === 'mimo') return 'mimo'
  return harness.trim() === '' ? 'unknown harness' : harness
}

/* -------------------------------------------------------------------------------- the scopes */

/**
 * Which of the four directories a definition was read from. Restates `AgentScope` in
 * `crates/cide-ipc/src/agents.rs`, and `check-agents.mjs` pins the two as sets — a variant added
 * in Rust and missed here fails the frontend build rather than rendering as nothing.
 */
export type Scope = 'project' | 'global' | 'claudeProject' | 'claudeGlobal'

export const SCOPES: readonly Scope[] = [
  'project',
  'global',
  'claudeProject',
  'claudeGlobal',
]

/** Is this one of the scopes this build knows how to label? */
export function isScope(value: string): value is Scope {
  return SCOPES.includes(value as Scope)
}

/**
 * The badge a role's row carries for where it came from, or `null` for no badge at all.
 *
 * # Why only one of the four scopes gets a mark
 *
 * A badge is a claim that something is *unusual*, and three of these four are not: `.cide/agents/`
 * and its global twin are cide's own directories, and a row from either behaves exactly as every
 * row in this panel has always behaved. Badging all four would put a chip on every line and say
 * nothing.
 *
 * What a Claude Code subagent needs the user to know is a real difference in kind. cide did not
 * define that file's format and cannot model all of it; the definition is applied by the CLI
 * through `--agent` rather than by cide's own flags; there is no harness to choose; and the same
 * file is read by `claude` outside cide entirely. "Why does this role have no harness dropdown"
 * and "why did editing this change what my terminal does" both have one answer, and this chip is
 * where it is visible without opening anything.
 *
 * Project and user share the mark deliberately: which of the two a subagent is in matters when you
 * go to edit it and not when you are reading a roster, and two chips a character apart would be
 * two chips nobody reads carefully.
 */
export function scopeBadge(scope: string): string | null {
  return scope === 'claudeProject' || scope === 'claudeGlobal'
    ? 'Claude Code'
    : null
}

/* ----------------------------------------------------------------------------- the phases */

/**
 * Where one run is — the tag of `RunState`, flattened.
 *
 * `RunState` is a tagged union whose payloads (`sinceUnixMs`, `code`, `reason`) belong to three
 * of its nine arms. The panel groups, glyphs and tones by the **tag** in every one of those
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
  | 'interrupted'
  | 'finished'
  | 'failed'

/** The nine phases, in `RunState`'s declaration order. */
export const RUN_PHASES: readonly RunPhase[] = [
  'queued',
  'starting',
  'running',
  'idle',
  'awaitingPermission',
  'paused',
  'interrupted',
  'finished',
  'failed',
]

/**
 * Is this one of the nine phases?
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
 * Six names for nine phases. `starting` and `running` share `busy`, because "the child is
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
/*
 * The eight phases, as icon **names** rather than as `IconName`, and the `string` is not
 * laziness: this module is import-free on purpose — `check-agents.mjs` compiles it standalone
 * with no `--rootDir` and imports the output directly — so a single `import type` would pull a
 * third file into the program, move tsc's inferred common source directory and break that path.
 * `asIcon` at the render site is what turns a name back into a checked one, and
 * `check-ui-icons.mjs` is what proves every string here is a mark the app actually ships.
 *
 * One silhouette, eight interiors — which is what `○ ◌ ● ◉` was reaching for and could not hold,
 * because those four came from four different parts of Unicode and rendered at four weights.
 */
const PHASE_GLYPH: Record<RunPhase, string> = {
  queued: 'circle',
  starting: 'circle-dashed',
  // An arc, so the mark reads as *turning*: that is the distinction `idle` below turns on. It
  // has to *actually* turn for that to be true, and for a long time it did not — see
  // [`SPINNING_GLYPH`].
  running: 'loader-circle',
  // A ringed dot rather than a hollow or a filled one: the child is *there* (the centre) but is
  // not turning (the ring). It has to be readable against both `queued`'s empty circle and
  // `running`'s spinner, because those are the two states a glance at an idle row could
  // otherwise mistake it for, and they are the two opposite mistakes.
  idle: 'circle-dot',
  awaitingPermission: 'circle-alert',
  paused: 'circle-pause',
  // A bar through the silhouette: the child was taken away (cide's shutdown ladder), which is
  // neither `paused`'s two bars (a live child, frozen) nor `failed`'s ✗ (a result). Resumable,
  // so it must not wear a terminal mark.
  interrupted: 'circle-minus',
  finished: 'circle-check',
  failed: 'circle-x',
}

const PHASE_LABEL: Record<RunPhase, string> = {
  queued: 'Queued',
  starting: 'Starting',
  running: 'Running',
  idle: 'Idle',
  awaitingPermission: 'Awaiting permission',
  paused: 'Paused',
  interrupted: 'Interrupted',
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
  // `paused`'s tone, not `error`'s: the conversation is intact and resuming it is ordinary —
  // `RunState::Interrupted`'s doc carries the whole not-Failed argument. A red row here would
  // greet every restart of cide with a wall of alarms about its own shutdown ladder.
  interrupted: 'paused',
  finished: 'done',
  failed: 'error',
}

/*
 * What an unrecognised phase gets. Constants rather than the raw value, because the raw value
 * may be the empty string, and an empty glyph is a row whose dot column silently collapses —
 * which reads as "this run has no state" rather than "cide does not know this state".
 */
const UNKNOWN_GLYPH = 'circle-slash'
const UNKNOWN_LABEL = 'Unknown'
const UNKNOWN_TONE: Tone = 'idle'

/**
 * The one mark in this vocabulary that is drawn **rotating**.
 *
 * # Why the animation keys off the glyph and not off the phase
 *
 * Because the claim being animated is the *mark's*, not the state's. `loader-circle` is a
 * three-quarter arc: it says "turning", and `idle`'s ringed dot is distinguishable from it only
 * on that reading — [`PHASE_GLYPH`] says so in as many words. A phase-keyed rule would let the
 * two drift: give `running` a different mark and it would spin something that is not a spinner;
 * give another phase the arc and it would sit still next to one that moves.
 *
 * It sat still everywhere for a milestone and a half — the task list's chip, the task card's run
 * strip, the Agents panel's rows, and the accept button's own copy — so the single signal that
 * anything was happening was a static shape. Every surface that draws a phase mark now asks this
 * and adds its stylesheet's spin class. Three stylesheets, one rule, because a CSS module cannot
 * share a `@keyframes`.
 */
export const SPINNING_GLYPH = 'loader-circle'

/** Should this mark be drawn turning? See [`SPINNING_GLYPH`]. */
export function glyphSpins(glyph: string): boolean {
  return glyph === SPINNING_GLYPH
}

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
 * The longer truth behind a one-word label, where one word is not the whole truth.
 *
 * Only `paused` carries one today, and it exists because a user watched their local model keep
 * inferring after they pressed Pause and reasonably concluded the panel was lying. It was not —
 * the child is frozen to the instruction — but a **model call already in flight is not a local
 * fact**: the provider does not know its client froze, so it finishes generating server-side
 * and the bytes wait in the kernel's socket buffer until Resume, when the child reads them and
 * the turn continues without loss. cide cannot cancel that call — only the child could abort
 * its own request, and the child cannot run an instruction; killing the connection instead
 * would destroy the turn, which is Stop, not Pause. So the honest sentence rides the row as a
 * `title` rather than the mechanism pretending otherwise.
 */
export function phaseHint(phase: RunPhase): string | null {
  if (phase !== 'paused') return null
  return (
    'The child process is frozen mid-turn. A model call that was already in flight is not ' +
    'cancelled — the provider finishes it on its side, and the answer is read when the run is ' +
    'resumed. Nothing new starts while paused.'
  )
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
  /**
   * Which of the four directories this definition came from. Drawn through [`scopeBadge`], and
   * carried as the scope rather than as a boolean so the Settings screen can open the right file
   * from a roster row instead of probing each scope in turn.
   */
  scope: Scope
  harness: Harness
  /** One line: what the role is *for*, in the author's own words. */
  description: string
  /** The role's system prompt, verbatim. Shown folded and read-only; never executed here. */
  systemPrompt: string
  /** A model alias or full name, or `null` for the harness's own default. */
  model: string | null
  /**
   * One of [`AGENT_HUES`] the definition asked for, or `null` — which means *derive one*, and is
   * the ordinary case. Read through [`agentColor`] and never directly; see it for why.
   */
  color: string | null
  /**
   * Why this role cannot be dispatched, as a sentence — or `null` when it can.
   *
   * This is `Command::unavailable` one layer down; see [`canDispatch`], which is the only thing
   * allowed to read it.
   */
  unavailable: string | null
  /** How many runs of this role may hold a slot at once. */
  maxConcurrent: number
  /**
   * `AgentDef::worktree` — does this role's runs take a checkout under `.cide/worktrees/`?
   *
   * Carried into the view because it decides what a *button somewhere else* is allowed to say:
   * `OpenSpecPanel/model.ts`'s `primaryAction` names the accept gesture *Integrate & Archive*
   * only when there is a branch to merge, and `worktree: false` means the role committed
   * straight into the user's own checkout, so there is not one. Reading it here is a lookup;
   * the alternative was a second `openspec`/git round trip per card open to learn a fact
   * already on the wire.
   */
  worktree: boolean
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
  /**
   * `AgentRun::startedUnixMs`, converted from `bigint` by `adapt.ts`. See the module header.
   *
   * When the run was **dispatched**, which is a different question from how long it worked —
   * see [`workedMs`]. This is what History sorts by, and it is deliberately not what the row's
   * figure is any more.
   */
  startedMs: number
  /**
   * Milliseconds this run has worked, over its **closed** working intervals only.
   *
   * Read with [`workingSinceMs`], never alone: the figure is this plus, when that stamp is set,
   * the time since it. [`workedFor`] is the one place that sum happens.
   */
  workedMs: number
  /**
   * When the open working interval began, or `null` when this run is not working.
   *
   * `null` for a queued, idle, awaiting-permission, paused, interrupted, finished or failed
   * run — Rust's `RunState::counts_as_work` is the definition and this mirrors its answer. Its
   * being `null` is what makes those rows' figures *stop* rather than tick.
   */
  workingSinceMs: number | null
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
  /**
   * Whether **Open** has anything to show — `AgentRun::openable`, computed by Rust from what the
   * registry holds: a session it still owns, or a conversation the real harness can be re-opened
   * on. (M42) The wire's fact, restated here so nobody derives it from `session` again: a
   * finished opencode run has no session and an openable conversation.
   */
  openable: boolean
  /**
   * The model this run is on — `provider/model` where the harness spells it that way — or
   * `null` when nothing chose one and the harness picked its own default. (M89) `AgentRun::
   * model`: **the run's, never the role file's**, so a row whose role has since been overridden
   * or failed over still says what this run was actually told.
   */
  model: string | null
  /** The pool that chose `model` and how far down it — `fast 2 of 3` — or `null`. (M89) */
  poolPosition: string | null
}

/**
 * **What a run is on**, as one dim line: the harness, the model where one was chosen, and the
 * pool where one chose it. (M89)
 *
 * ```
 * opencode · zai/glm-4.6 · pool fast 2 of 3
 * claude · opus
 * codex
 * ```
 *
 * The harness always leads, because it is the one fact every run has and the one that decides
 * what Open brings up. A missing model draws **nothing** in its place rather than `default` —
 * the harness chose it and cide was not told what it was, and printing a word for a model
 * nobody named is the guess this row exists to replace. The pool is last and says `pool`,
 * because `fast 2 of 3` alone reads like a quota rather than a position in a list.
 */
export function usingLabel(run: Pick<RunView, 'harness' | 'model' | 'poolPosition'>): string {
  const parts = [harnessLabel(run.harness)]
  const model = run.model?.trim() ?? ''
  if (model !== '') parts.push(model)
  const pool = run.poolPosition?.trim() ?? ''
  if (pool !== '') parts.push(`pool ${pool}`)
  return parts.join(' · ')
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
 * Agent id → label, for the Tasks panel's assignee list and chips. `{}` unless ready.
 *
 * This is the join `TasksPanelHost` was written waiting for — its header spent a paragraph on
 * "roles is empty, deliberately, until the Agents slice lands", and this function is the two
 * lines that changed. A blank id is skipped (an option whose `value` is `""` would collide with
 * the hardcoded Unassigned row), and a blank label falls back to the id so the option still has
 * a face — [`roleName`]'s ladder, minus its final rung, because `assignableRoles` keys on the id
 * and a "This role" entry would be a label with no key behind it.
 *
 * A plain object is safe here: every reader is prototype-proof (`roleLabel` goes through
 * `Object.hasOwn`, `assignableRoles` through `Object.keys`), and `check:agents` pins an
 * `id: 'constructor'` row to keep that true. Built through `Object.fromEntries` rather than
 * `roles[id] = label`, because bracket assignment of `"__proto__"` on a literal hits the setter
 * and silently creates nothing — no such id can come out of Rust's name rule, but this function
 * must not be the one place in the file that trusts that.
 */
export function rosterRoles(roster: Roster): Readonly<Record<string, string>> {
  if (roster.kind !== 'ready') return {}
  return Object.fromEntries(
    roster.agents
      .filter((def) => def.id.trim() !== '')
      .map((def) => [def.id, def.label.trim() !== '' ? def.label : def.id]),
  )
}

/* -------------------------------------------------------------------------- a role's colour */

/**
 * The colours a role may be drawn in, in the order a derived colour indexes them. (M75)
 *
 * **`cide_ipc::agents::AGENT_HUES` restated**, and `check:agents` scrapes that constant out of
 * `crates/cide-ipc/src/agents.rs` and asserts the two lists are equal *in order* — not as sets,
 * which is the usual shape of that assertion here and would be too weak: the index is what a
 * derived colour is, so two lists holding the same eight words in different orders would recolour
 * every uncoloured role in every tracker and fail nothing.
 *
 * Each name must have a `--agent-<name>` in **both** palette blocks of `tokens.css`; `check:theme`
 * is what asserts it, and the failure without it is silent — an undefined custom property is not
 * a colour, so the name would simply render in whatever it inherited.
 */
export const AGENT_HUES = [
  'blue',
  'green',
  'orange',
  'purple',
  'cyan',
  'red',
  'yellow',
  'pink',
] as const

/**
 * The orchestrator's colour: the project's primary session, the one that decomposes and
 * dispatches.
 *
 * **Reserved by not being in [`AGENT_HUES`]**, which is the whole mechanism. The user asked for a
 * colour that "will never be conflicted with other agents' colours", and the way to promise that
 * is to make it unreachable rather than to write a rule that skips it: no hash can return a hue
 * that is not in the list it indexes, and no `color:` can name one either, because
 * `cide_ipc::agents::hue` answers `None` for every word outside the eight.
 *
 * It is a slate rather than a ninth hue for the same reason. `check:theme` asserts it is at least
 * 100 redmean units from all eight — it measures 110 in both themes — where the eight are only 70
 * apart from each other. "Not one of the agents" has to read as a different *kind* of colour, not
 * as the ninth one along.
 */
export const ORCHESTRATOR_COLOR = 'var(--agent-orchestrator)'

/**
 * A role's colour, as a CSS custom property reference.
 *
 * `var(--token)` and never a hex — `gitlog/logModel.ts`'s `laneColor` states the reason and it is
 * the same one: the value has to follow the theme, and a hex resolved in JavaScript is a colour
 * from whichever theme happened to be on when the component rendered.
 *
 * Two rungs, and the order between them is the feature. A definition that *declares* a colour
 * gets it; everything else is derived from the role's **id**, which is the name the file is
 * called and the key the roster is built on. Deriving means nobody has to choose, a fresh role is
 * distinguishable the moment it exists, and the answer is identical in every window, every
 * repository and for every person on the team without a byte being stored.
 *
 * The id rather than the label, deliberately. `TaskAuthor::Agent` carries a label *copied at
 * write time* so an old comment still reads correctly after a rename, and colouring by the label
 * would mean a role's own comments changed colour halfway down a log the day somebody edited
 * `label:`. The id is what both the roster and the comment carry, so the panel and the card
 * cannot disagree.
 *
 * FNV-1a over the code units: it is eight lines, it is stable across engines and releases, and
 * the decision it informs is "give these names different colours", not a security question. The
 * modulus is taken defensively in the same spirit as `laneColor`'s — `AGENT_HUES.length` is
 * eight today, and an out-of-range index would select nothing and paint the inherited colour.
 */
export function agentColor(id: string, declared: string | null = null): string {
  const named = declared === null ? null : declared.trim().toLowerCase()
  if (named !== null && (AGENT_HUES as readonly string[]).includes(named)) {
    return `var(--agent-${named})`
  }
  let hash = 0x811c9dc5
  for (let i = 0; i < id.length; i += 1) {
    hash ^= id.charCodeAt(i)
    // FNV's 32-bit prime, as the shifts a 32-bit `Math.imul` would cost a polyfill for.
    hash = (hash + (hash << 1) + (hash << 4) + (hash << 7) + (hash << 8) + (hash << 24)) >>> 0
  }
  return `var(--agent-${AGENT_HUES[hash % AGENT_HUES.length]})`
}

/**
 * Every role's colour, by id — [`rosterRoles`]' sibling, and deliberately not a widening of it.
 *
 * `roles` is pinned by `check:agents` as an id→label map and read from three other files; a
 * second map of the same shape costs one `useMemo` and leaves that contract alone. Both are
 * memoised on the roster's identity at every host that builds them, which is not tidiness:
 * a selector that returns a fresh object re-renders for ever and ends at *Maximum update depth
 * exceeded*, which unmounts the whole root. That is `check:selectors`' entire subject.
 *
 * A role the roster has never heard of is *absent* rather than defaulted, so a reader falls
 * through to [`agentColor`] with the id alone — which is the right answer for a comment written
 * by a role that has since been deleted, and is why every call site passes the id and not just a
 * lookup.
 *
 * Built through `Object.fromEntries` and filtered on a blank id for [`rosterRoles`]' two reasons,
 * restated because the two functions must not drift on the prototype question.
 */
export function rosterColors(roster: Roster): Readonly<Record<string, string>> {
  if (roster.kind !== 'ready') return {}
  return Object.fromEntries(
    roster.agents
      .filter((def) => def.id.trim() !== '')
      .map((def) => [def.id, agentColor(def.id, def.color)]),
  )
}

/**
 * The colour a comment's, or a task's, author is drawn in.
 *
 * One function for all three arms so the card cannot answer differently in two places, and
 * `null` for the user: "You" keeps `.logAuthorUser`'s `--accent`, which it has had since M21 and
 * which is deliberately **outside** the separation gate — a reader is never in doubt about which
 * lines are their own, so the colour there is a flourish and not a signal.
 */
export function authorColor(
  author: { kind: string; agent?: string },
  colors: Readonly<Record<string, string>>,
): string | null {
  if (author.kind === 'orchestrator') return ORCHESTRATOR_COLOR
  if (author.kind !== 'agent') return null
  const id = author.agent ?? ''
  if (id.trim() === '') return null
  return Object.hasOwn(colors, id) ? (colors[id] ?? agentColor(id)) : agentColor(id)
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
 * Is there anything for **Open** to show?
 *
 * `openable` is Rust's answer — a session the registry still holds, or a conversation the real
 * harness can be re-opened on (M42) — and the phase test is belt-and-braces against a backend
 * that fills `session` before the child reports in: a mirror of a session with no PTY behind it
 * is an empty pane the user then has to close. A queued run has neither, which is exactly why
 * the Open control is *withheld* from a queued row rather than drawn disabled.
 *
 * What Open then does is Rust's to decide at the click (`agentRuns.open`): attach to the live
 * child, or spawn the harness on the conversation — `claude --resume` in the run's worktree,
 * or the opencode TUI on its `ses_…`. So this is **true for a finished run** (the transcript
 * is complete and the harness re-renders it), **true for an interrupted one** (its child died
 * with the old cide; the conversation did not — this used to be withheld because Open could
 * only mirror, and a mirror of a dead session was a blank pane under a row whose real
 * affordance was Resume), and **true for an idle run**, the best case there is: the turn is
 * over *and* the child is alive, so the pane is interactive rather than a picture of one.
 */
export function canOpen(run: RunView): boolean {
  return run.openable && run.phase !== 'queued'
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
  /**
   * The `title` behind the phase label, where the one-word label is not the whole truth —
   * see [`phaseHint`]. `null` for every phase whose word suffices, and the row then falls
   * back to the label itself.
   */
  hint: string | null
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
  // After `paused` and before `idle`: both are runs that will not advance until somebody acts,
  // and an interrupted one is a step further from running (no child at all) — but it still
  // outranks the two that are quiet by their own choice.
  'interrupted',
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
export const ROLE_UNDEFINED =
  'No definition for this role in .cide/agents/ or .claude/agents/ any more.'

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
   * A role genuinely runs its `max-concurrent` tasks at once now — worktrees went per *task*,
   * so two tasks of one role are two checkouts and the old one-run-per-role clamp is gone —
   * which makes several entries here the **ordinary** state of a fanned-out role, not the
   * `isolation: shared` curiosity it used to be. The honest answer is to draw them all: the
   * alternative considered was one summary line plus an "and 2 more" count, and it fails the
   * panel's own standing rule — a run that is on screen nowhere is a `claude` spending the
   * user's quota that they cannot see, cannot open and cannot stop.
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
    hint: phaseHint(run.phase),
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
    // Nothing on the wire says where a deleted role's definition used to live, and a run does
    // not carry it. `project` is the answer with the fewest consequences: it draws no badge, and
    // a badge on a row that exists only because a definition is *missing* would be a claim about
    // a file nobody can point at.
    scope: 'project',
    harness: run.harness,
    description: '',
    // Derived from the id, like every role with no `color:` — and that is why this row is the
    // same colour here as it is above the comments the deleted role wrote. `agentColor` is keyed
    // on the id, which a run carries, so a role that has gone does not change colour on its way
    // out of the roster.
    color: null,
    systemPrompt: '',
    model: null,
    unavailable: ROLE_UNDEFINED,
    maxConcurrent: 0,
    /*
     * The wire's default, and the safe one here.
     *
     * Nothing a run carries says whether the definition that has gone isolated its checkouts.
     * `true` is the answer that keeps a *later* surface honest rather than the one that reads
     * best on this row: `primaryAction` prints *Archive* on `false`, so guessing `false` for a
     * role whose branch really is waiting would tell the user there was nothing to merge. This
     * row cannot dispatch anything anyway — `unavailable` is set — so the value is never read
     * to start work.
     */
    worktree: true,
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
      // 'History', because that is what it is now: the registry snapshots ended runs across
      // restarts (50 kept per project), so this section is a durable record of what ran, not
      // a per-process scratchpad. The *kind* stays 'recent' — it is an internal name in every
      // fixture and check, and renaming a wire-adjacent identifier to chase a label is churn.
      label: 'History',
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
  return span(Math.max(0, nowMs - startedMs))
}

/**
 * A duration as `12s` / `4m` / `1h 04m`.
 *
 * The ladder, factored out of [`elapsed`] so that it and [`workedFor`] cannot drift into two
 * vocabularies — one producer, the rule `cide_git::push::preview` states, applied to a pair of
 * figures that appear in the same row and in the same tooltip. Non-finite input is the
 * caller's to guard, because each of them has a different thing to check.
 */
function span(ms: number): string {
  const totalSeconds = Math.floor(Math.max(0, ms) / 1000)
  if (totalSeconds < 60) return `${totalSeconds}s`
  const totalMinutes = Math.floor(totalSeconds / 60)
  if (totalMinutes < 60) return `${totalMinutes}m`
  const hours = Math.floor(totalMinutes / 60)
  const minutes = totalMinutes % 60
  // Zero-padded so `1h 04m` and `1h 40m` are the same width and a column of them does not
  // jitter as the minutes tick over.
  return `${hours}h ${minutes < 10 ? '0' : ''}${minutes}m`
}

/** The two halves of a run's worked clock, which is all these figures need of a run. */
export type WorkedClock = Pick<RunView, 'workedMs' | 'workingSinceMs'>

/**
 * How long a run has **worked** — the figure the row draws.
 *
 * # Why this is not `elapsed(nowMs, startedMs)`
 *
 * It was, and that was wrong in three ways at once, all of them ordinary use. `startedMs` is
 * stamped when a run is *dispatched* and never moves, so the old figure was age-since-dispatch:
 * a run paused overnight accrued the night; a run that outlived a cide restart accrued the
 * hours cide was shut; and a finished run's figure went on growing for ever, so a run that took
 * ninety seconds and ended four hours ago read `4h 00m` on the History row whose own doc says
 * "how long it took".
 *
 * # The sum, and why the wire carries two numbers instead of it
 *
 * `workedMs` is the closed total and `workingSinceMs` is the open interval. Rust could send the
 * sum, but then the figure would only move when a roster broadcast happened to arrive, so a
 * live run's row would sit still for seconds at a time and jump. Sending the halves lets the
 * row tick off `nowMs` at one second while a run that is *not* working — `workingSinceMs` is
 * `null` — draws a figure that does not move at all. That absence is the whole fix: the number
 * stops when the work does, which is what makes a paused row static and a finished row final
 * with no end timestamp anywhere in the system.
 *
 * A clock that has gone backwards yields the closed total rather than less than it, and a
 * non-finite input an em dash — [`elapsed`]'s rules, for [`elapsed`]'s reasons.
 */
export function workedFor(nowMs: number, run: WorkedClock): string {
  if (!Number.isFinite(nowMs) || !Number.isFinite(run.workedMs)) return '—'
  return span(workedMsAt(nowMs, run))
}

/** The sum, as a number. One producer for it, since three readers here want it. */
function workedMsAt(nowMs: number, run: WorkedClock): number {
  const open =
    run.workingSinceMs !== null && Number.isFinite(run.workingSinceMs)
      ? Math.max(0, nowMs - run.workingSinceMs)
      : 0
  return Math.max(0, run.workedMs) + open
}

/**
 * The time cell's `title`: the phase in words, then what the two clocks say.
 *
 * # The phase word stays first, and stays
 *
 * `RunRow` carries the argument: the dot at the head of the line is the only other place a
 * run's state is written, and a dot is not readable by a screen reader or by somebody who has
 * not learned the glyphs. So this *appends* to that word rather than replacing it.
 *
 * # Why the row needs a tooltip at all now
 *
 * The row draws worked time, which is the honest answer to "how long did this agent work" and
 * is *not* the answer to "how long has this been sitting there". Both are worth knowing, and a
 * run paused overnight is exactly the case where they differ enough to confuse somebody who
 * only has one of them. So the wall clock goes here, with the difference named.
 *
 * # A terminal run gets no excluded clause
 *
 * `nowMs - startedMs` goes on growing after a run has ended, while its worked figure is final,
 * so their difference would be a number that rises for ever while claiming to describe a pause
 * — the same lie in a new place as the figure this change removes. A finished run is told
 * when it was dispatched and how long it worked, and nothing is inferred from the gap.
 */
export function timeTitle(nowMs: number, run: RunView, phrase: string): string {
  if (!Number.isFinite(nowMs) || !Number.isFinite(run.startedMs)) return phrase
  const parts = [phrase, `dispatched ${elapsed(nowMs, run.startedMs)} ago`]
  const worked = workedFor(nowMs, run)
  if (worked !== '—') parts.push(`worked ${worked}`)
  if (!isOver(run.phase)) {
    const idle = Math.max(0, nowMs - run.startedMs) - workedMsAt(nowMs, run)
    // A second of slop is not worth a clause, and the figure is a subtraction of two clocks
    // that were read at different instants — so it is never exactly zero on a run that has
    // done nothing but work.
    if (idle >= 1000) parts.push(`${span(idle)} not working`)
  }
  return parts.join(' · ')
}

/** Whether this phase is one a run never leaves — the two the tooltip treats differently. */
function isOver(phase: RunPhase): boolean {
  return phase === 'finished' || phase === 'failed'
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
