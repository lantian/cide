/**
 * The wire's `AgentRoster` into the panel's `Roster`. (M18)
 *
 * # Why there are two of them at all
 *
 * `model.ts` **imports nothing** — that is what lets `ui/scripts/check-agents.mjs` compile it
 * alone with a bare `tsc` and drive it under node, which together with the render check is this
 * panel's whole test story. So it cannot import `generated.ts` and restates the shapes
 * structurally instead. This module is the one place the two meet, and it is the only file in
 * the directory allowed to import both. `TasksPanel/adapt.ts` does the identical job one
 * directory over and this follows it; `ProblemsPanel/adapt.ts` is the original.
 *
 * Nothing else may import this. It has one caller — `sidebar/agentsStore.ts` — and it is
 * deliberately not re-exported from `index.ts`, because a barrel entry would only offer a
 * second way to reach it and, through it, a second place for the conversion to happen.
 *
 * # What actually differs, and why none of it is laziness
 *
 * * **`bigint` → `number`.** `AgentRun::startedUnixMs` and `RunState::Paused::sinceUnixMs` are
 *   `u64` in Rust, and ts-rs renders a `u64` as **`bigint`**. Both are `number` in `model.ts`,
 *   because `elapsed(nowMs, startedMs)` subtracts them from a `Date.now()`: mixing a `bigint`
 *   with a `number` in an arithmetic expression throws a `TypeError` rather than coercing, and
 *   a throw out of a render unmounts the whole tree under React 19 — a blank window rather than
 *   a wrong duration.
 *
 *   `Number()` on a millisecond timestamp is **exact**: a `number` holds every integer up to
 *   2^53 - 1, which as Unix milliseconds is the year 287396. Nothing is lost and nothing is
 *   rounded; the conversion is total.
 *
 *   It is total in the *other* direction too, which is worth knowing before somebody "fixes" it
 *   with a `typeof` branch: what actually arrives over Tauri's IPC is a JSON number, because
 *   serde writes a `u64` as one, so `bigint` is what the bindings *say* rather than what the
 *   wire carries, and `Number()` is then the identity function. Converting unconditionally is
 *   what makes this module correct under either reading — and the type is the thing to trust,
 *   because the day a payload does arrive as a `bigint` the subtraction in `elapsed` throws.
 *
 * * **`RunState` flattened.** The wire carries a tagged union with three payload-bearing arms;
 *   `RunView` carries a `phase` plus `pausedSinceMs`, `exitCode` and `failure`, each `null` in
 *   every phase but its own. `model.ts`'s own comment argues why: a row that had to `switch` on
 *   the union to find out which of four sections it belongs in would be four `switch`es that
 *   can disagree. The flattening happens **here**, once, and the `switch` below is exhaustive so
 *   an eighth `RunState` variant added in Rust is a compile error in this file.
 *
 * * **`T | null` on the wire against the model's own optionality.** Every outbound optional in
 *   `cide-ipc` is `T | null` rather than an absent field, because `#[ts(optional)]` changes only
 *   the emitted TypeScript and not what serde writes. Under `exactOptionalPropertyTypes` those
 *   are genuinely different types, so each one is restated structurally rather than passed
 *   through — `model`, `unavailable`, `session`, `task` and `note` are all of them.
 *
 * Everything else is a structural copy, and that is the point: this is a conversion, not a
 * second model. The moment it grows a *decision* — a default, a fallback, a piece of wording —
 * that decision belongs in `model.ts`, where the check script can reach it.
 *
 * # Field by field, never `as`
 *
 * The objects are built out explicitly instead of being cast or spread. A cast would compile
 * today and go on compiling after a Rust field was renamed, which is the exact drift
 * `cargo xtask codegen --check` exists to catch one layer down; writing the fields out means the
 * rename fails *here*, in the seam that is supposed to know about it.
 */
import type {
  AgentDef as WireDef,
  AgentRoster as WireRoster,
  AgentRun as WireRun,
  RunState as WireState,
} from '@/ipc/client'
import type { AgentDefView, Roster, RunPhase, RunView } from './model'

/**
 * `bigint` → `number`, in one place so the argument above is made once.
 *
 * Exact for every value either side of this seam carries; see the header.
 */
function ms(value: bigint): number {
  return Number(value)
}

/**
 * One role. `AgentDefView` is `AgentDef` restated, field for field.
 *
 * No count in this sentence on purpose: it said *seven* while the struct carried eight, which is
 * what a number in a comment does the first time somebody adds a field. `tsc` counts them.
 */
function def(from: WireDef): AgentDefView {
  return {
    id: from.id,
    label: from.label,
    scope: from.scope,
    harness: from.harness,
    description: from.description,
    systemPrompt: from.systemPrompt,
    // `Option<String>` on the wire, `string | null` in the model — the same spelling, on
    // purpose. See the header: a model that accepted `undefined` too would have two ways to
    // ask "did the author name a model", and the two would drift.
    model: from.model,
    /*
     * The dispatchability sentence, carried across verbatim and **not** interpreted here.
     * `canDispatch` is the only thing allowed to read it, and it lives in `model.ts` so the
     * check script can drive it. A conversion that decided anything about this field would be
     * a second copy of that gate, in the file no check can reach.
     */
    unavailable: from.unavailable,
    maxConcurrent: from.maxConcurrent,
    // `#[serde(default = "default_worktree")]` in Rust, so a payload from before the field
    // existed omits it and means `true`. Restated rather than left to `??` at each reader: the
    // one reader that forgot would print *Archive* over a run whose branch is about to merge.
    worktree: from.worktree,
  }
}

/**
 * `RunState`'s eight arms into `RunView`'s `phase` plus its three payload fields.
 *
 * The `switch` is exhaustive and returns from every arm, so adding a variant in Rust and
 * regenerating fails this file rather than silently producing a run with no phase. The payload
 * fields are `null` in every arm but the one that carries them, which is what `model.ts` means
 * by "`null` in every other phase" on each of the three.
 */
function state(from: WireState): {
  phase: RunPhase
  pausedSinceMs: number | null
  exitCode: number | null
  failure: string | null
} {
  const none = { pausedSinceMs: null, exitCode: null, failure: null }
  switch (from.state) {
    case 'queued':
      return { phase: 'queued', ...none }
    case 'starting':
      return { phase: 'starting', ...none }
    case 'running':
      return { phase: 'running', ...none }

    // Turn handed back, child alive — deliberately not `finished`, which would put an
    // exit code on the wire for a process that has not exited. `RunState::Idle`'s own doc
    // carries the argument; this arm just has to not flatten the two back together.
    case 'idle':
      return { phase: 'idle', ...none }
    case 'awaitingPermission':
      return { phase: 'awaitingPermission', ...none }
    case 'paused':
      return { ...none, phase: 'paused', pausedSinceMs: ms(from.sinceUnixMs) }
    // Restored from the registry's snapshot after a restart: no child, no code, nothing
    // frozen — a row whose one affordance is Resume, which continues the conversation.
    case 'interrupted':
      return { phase: 'interrupted', ...none }
    case 'finished':
      return { ...none, phase: 'finished', exitCode: from.code }
    case 'failed':
      return { ...none, phase: 'failed', failure: from.reason }
  }
}

function run(from: WireRun): RunView {
  return {
    run: from.run,
    agent: from.agent,
    // Copied at dispatch by Rust and copied again here, never joined against the roster. A row
    // that re-derived its label would rename itself when a role file changed — see the field's
    // doc on both sides, which say the same thing because it must not be "fixed" on either.
    agentLabel: from.agentLabel,
    harness: from.harness,
    session: from.session,
    ...state(from.state),
    task: from.task,
    startedMs: ms(from.startedUnixMs),
    staleTurn: from.staleTurn,
    note: from.note,
  }
}

/**
 * The roster as the panel reads it.
 *
 * Three arms in and three arms out — the model's fourth, `unknown`, is **not reachable from
 * here** and that is deliberate. `ROSTER_UNKNOWN` means *nobody has looked*, which is a fact
 * about this window rather than about the project, and it is the store's to hold:
 * `agents.roster` answering `null` (a build whose backend has no such handler) and not having
 * answered yet are both that state, and neither is an `AgentRoster`. A conversion that could
 * invent `unknown` would let a real answer be turned into "we do not know", which is the one
 * direction that must not exist — and the answer it would most often erase is `disabled`, whose
 * screen carries the button that writes a file into the user's repository.
 */
export function adaptRoster(from: WireRoster): Roster {
  switch (from.kind) {
    case 'disabled':
      return { kind: 'disabled', hint: from.hint, configPath: from.configPath }
    case 'empty':
      return { kind: 'empty', configPath: from.configPath }
    case 'ready':
      return {
        kind: 'ready',
        agents: from.agents.map(def),
        runs: from.runs.map(run),
        dispatching: from.dispatching,
      }
  }
}
