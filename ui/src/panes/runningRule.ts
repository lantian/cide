/**
 * What the header's project tabs say about *work in progress*, as a pure decision. (M94)
 *
 * The chip beside this one — `awaitingRule.ts` — answers *who is waiting for me*. This one
 * answers the opposite and equally useful question about a project the user is not currently
 * in: *how much is working in there right now*. Agent runs plus console panes, one number.
 *
 * # Rust decides, this file only shapes the answer
 *
 * `awaitingRule.ts` exists because the *decision* genuinely lives in the webview: acknowledging
 * a session is a click or a keystroke Rust never sees, so the frontend folds transitions and
 * reports its answer up. Nothing here is acknowledged. Every fact behind a running count — a
 * run's state, whether a child is alive, whether a pane is merely a run's mirror, what the hook
 * server last heard — lives in a Rust registry, for every project, including the ones this
 * window is not drawing. So this module holds no state machine at all: it folds a broadcast into
 * a table, decides whether a late reply is still news, and writes the two sentences the chip
 * and its tooltip need.
 *
 * That is still worth a module of its own, for `awaitingRule.ts`'s reason: the wording and the
 * cap are the parts a check script can drive, and "1 consoles" is exactly the kind of thing that
 * ships when the sentence is built inline in a component nothing compiles on its own.
 *
 * DOM-free and import-free on purpose, like `awaitingRule.ts`, `menus/model.ts` and
 * `panes/exitMarker.ts`: `ui/scripts/check-running.mjs` compiles this file with a bare `tsc` and
 * drives it, and there is no JS test runner in this repo. Do not import React, `@/…` or a DOM
 * type into this file.
 */

/**
 * One project's two counts.
 *
 * Both halves are kept rather than pre-summed, because the *tooltip* needs to name them
 * separately — see [`runningHint`]. The chip shows the total.
 *
 * Written out rather than imported from the generated bindings so the check script can compile
 * this file alone. The assignment in `running.ts` is the equivalence check: a field renamed in
 * Rust stops typechecking there rather than reading `undefined` into a badge that silently
 * says 0.
 */
export interface Tally {
  readonly runs: number
  readonly panes: number
}

/** One entry of a `cide://project-running` broadcast, as this module reads it. */
export interface RunningEntry {
  readonly project: string
  readonly runs: number
  readonly panes: number
}

/** The table, keyed by project id. */
export type Counts = ReadonlyMap<string, Tally>

/** A project nothing is running in. */
export const NOTHING: Tally = { runs: 0, panes: 0 }

/**
 * Fold a whole broadcast into a table.
 *
 * **Absence is zero, not unknown**, and that is the contract Rust holds up by omitting every
 * project with nothing running: the set is complete by construction, so a project that is not
 * in it has nothing working. There is deliberately no third state here — an "unknown" would have
 * to be drawn as *something*, and both choices are wrong (a chip on a quiet project, or a quiet
 * project indistinguishable from one cide has not looked at).
 *
 * Zero entries are dropped on the way in as well, so a producer that ever starts sending them
 * cannot make [`runningIn`] disagree with itself about a project that appears with `0`.
 */
export function foldRunning(entries: readonly RunningEntry[]): Counts {
  const counts = new Map<string, Tally>()
  for (const entry of entries) {
    if (entry.runs <= 0 && entry.panes <= 0) continue
    // Last one wins, rather than summing: a broadcast carries one entry per project, and
    // summing a malformed duplicate would inflate a badge rather than make the bug visible.
    counts.set(entry.project, { runs: entry.runs, panes: entry.panes })
  }
  return counts
}

/**
 * Whether a set carrying generation `at` is news to a reader that has seen `seen`.
 *
 * **The whole reason `ProjectRunningSet` carries a number at all.** `cide://project-running` is
 * a change notification, so a window that opened between two changes has heard nothing and must
 * ask — and the reply describes the set as it was when the question landed, which a broadcast
 * emitted in the meantime can overtake on the way back. Without this comparison such a window
 * settles on a stale count and stays there until the next genuine change, which for a
 * long-running agent is many minutes of a wrong badge with nothing logged.
 *
 * Strictly greater, so a repeat of the generation already applied is dropped. The producer
 * bumps on every computed set, including the ones it decides not to emit, so equal really does
 * mean "this exact answer".
 */
export function isNewer(seen: number, at: number): boolean {
  return at > seen
}

/** One project's tally, or [`NOTHING`]. */
export function tallyIn(counts: Counts, project: string | null | undefined): Tally {
  if (!project) return NOTHING
  return counts.get(project) ?? NOTHING
}

/**
 * How many things are working in one project — agent runs **plus** console panes.
 *
 * One number on the chip rather than two, because the question a header tab answers is *is
 * there anything going on in there*, and a user deciding whether to switch does not need the
 * split to decide. The split is in the tooltip, where there is room for a sentence.
 *
 * They cannot double-count: Rust drops a console pane that is merely a run's mirror
 * (`AgentRegistry::owns_session`) precisely so that a run showing in a pane is counted once, as
 * a run.
 */
export function runningIn(counts: Counts, project: string | null | undefined): number {
  const tally = tallyIn(counts, project)
  return tally.runs + tally.panes
}

/** The agent-run half alone, which is what [`runningHint`] needs to word itself. */
export function runsIn(counts: Counts, project: string | null | undefined): number {
  return tallyIn(counts, project).runs
}

/**
 * The most this chip will spell out. `awaitingBadge`'s cap and its argument: past it the number
 * stops being a thing you act on and starts being a thing you read, and the box it has to fit
 * in is fixed.
 */
const BADGE_CAP = 9

/**
 * The glyph for a project's count, or `''` when nothing is running.
 *
 * Capped rather than allowed to grow, because the chip is fixed width and a marker that widened
 * the tab it sits on would reflow the header strip under the pointer — see the CSS. The
 * uncapped figure is in the tooltip, which is the whole reason [`runningHint`] takes the raw
 * numbers rather than the capped string.
 */
export function runningBadge(count: number): string {
  if (count <= 0) return ''
  return count > BADGE_CAP ? `${BADGE_CAP}+` : String(count)
}

/**
 * The sentence behind the chip, for the tooltip and the accessible name, or `undefined` when
 * there is nothing to say.
 *
 * **Both halves are named.** "2 running" over a project with two agent runs and a console the
 * user is typing in is a number they cannot act on — the action for a busy agent is *go and
 * read the board*, and for a busy console it is *go and look at it*. It is also the only place
 * a user with eleven learns it is eleven, because the chip stops at `9+`.
 *
 * It says "agent runs" and "consoles", never "sessions", and that wording is load-bearing
 * rather than stylistic: a **shell** pane running a long build is counted by neither chip's
 * producer here, while `lifecycle::watch_jobs` does raise the *awaiting* chip when that build
 * ends. A user who read this as "2 sessions" would be right to call the pair inconsistent; a
 * user who reads it as "2 consoles" is being told exactly what was counted.
 *
 * `where` names the container, as `awaitingHint`'s does, so a second surface that ever wants
 * this needs no second copy of the wording.
 */
export function runningHint(runs: number, panes: number, where: string): string | undefined {
  const parts: string[] = []
  if (runs > 0) parts.push(runs === 1 ? '1 agent run' : `${runs} agent runs`)
  if (panes > 0) parts.push(panes === 1 ? '1 console' : `${panes} consoles`)
  if (parts.length === 0) return undefined
  // "is" only when the whole subject is singular. Two clauses joined by `and` are plural
  // however small each number is, which is the case a naive `total === 1` gets wrong.
  const verb = runs + panes === 1 ? 'is' : 'are'
  return `${parts.join(' and ')} ${verb} working in this ${where}`
}
