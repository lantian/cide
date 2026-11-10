/**
 * What the activity rail's Agents button draws, and what it is called. (M18, M132, M199)
 *
 * The button carries **two marks that answer different questions**, and this is the one place
 * the rule between them is written:
 *
 * - the **run pill** (top right, the kit's neutral `Counter`): how many subagent runs are live —
 *   a quiet fact, nothing owed;
 * - the **waiting count** (bottom right, amber): how many things wait for the user — sessions
 *   stopped on them (input, a question, a permission prompt: the Sessions tab's pill) and the
 *   Waiting tab's tasks and requests, **a session's question counted once** although it is both.
 *
 * **Either can be drawn without the other**, and that independence is the point. A question
 * outlives the run that asked it (no live run, so no pill — and that is exactly when it waits
 * longest unnoticed), and runs work with nothing owed (pill, no count).
 *
 * # What M199 changed
 *
 * Until M199 the waiting mark was a dot with no number, counting board tasks only, and the run
 * pill turned red when a run sat at a permission prompt. A permission prompt is now one of the
 * waiting sessions, counted in amber with everything else the user owes, so the red pill would
 * have said the same thing twice in two colours; it went. The amber mark became a number because
 * it now sums two tabs, and "something, somewhere in this panel" was no longer enough to act on.
 *
 * Pure and **import-free**, so `check-rail-marks.mjs` compiles this file alone with a bare `tsc`
 * and runs it under node: `ActivityRail.tsx` cannot be rendered there (`@/menus` reaches
 * xterm and the DOM at import time), so the decision lives here and the component draws it.
 * `digits` is a parameter for the same reason — the rail passes `groupDigits`, which lives in a
 * module this one may not import.
 */

/** What waits for the user, as `sessionsModel.waitingSummary` and the Waiting tab count it. */
export interface AgentsWaiting {
  /** Everything, each question once — the number drawn. */
  total: number
  /** Live sessions stopped on the user — the Sessions tab's pill. */
  sessions: number
  /** The Waiting tab's own count: tasks in review, questions, requests. */
  decisions: number
}

export interface AgentsMarks {
  /** The pill's count, or `null` for no pill. Only a positive, known count draws one. */
  runs: number | null
  /** The waiting count, or `null` for none. Independent of `runs`. */
  waiting: number | null
  /** What follows "Agents — " in the button's name, or `null` when there is nothing to add. */
  name: string | null
}

const plural = (n: number, one: string, many: string) => (n === 1 ? one : many)

/**
 * The Agents button's marks.
 *
 * `null` and `0` both draw nothing, for every count, and the rail's standing reason: `null` is
 * nobody looked (subagents off, nothing read yet) and `0` is looked and there is nothing, and a
 * mark is a call to action — neither case has anything to act on.
 */
export function agentsMarks(
  agents: number | null | undefined,
  waiting: AgentsWaiting | null | undefined,
  digits: (n: number) => string = String,
): AgentsMarks {
  const runs = typeof agents === 'number' && agents > 0 ? agents : null
  const owed = waiting !== null && waiting !== undefined && waiting.total > 0 ? waiting.total : null
  const parts: string[] = []
  if (runs !== null) parts.push(`${digits(runs)} ${plural(runs, 'run', 'runs')}`)
  if (owed !== null && waiting !== null && waiting !== undefined) {
    // The split, so the number can be found: which tab holds what. "Waiting for you" always,
    // never a bare number: beside a run count, a bare number reads as runs.
    const split: string[] = []
    if (waiting.sessions > 0)
      split.push(`${digits(waiting.sessions)} ${plural(waiting.sessions, 'session', 'sessions')}`)
    if (waiting.decisions > 0) split.push(`${digits(waiting.decisions)} in Waiting`)
    const both = waiting.sessions + waiting.decisions - waiting.total
    const once = both > 0 ? `; ${both === 1 ? 'one is' : `${digits(both)} are`} both, counted once` : ''
    parts.push(`${digits(owed)} waiting for you${split.length > 1 || once !== '' ? ` (${split.join(', ')}${once})` : ''}`)
  }
  return { runs, waiting: owed, name: parts.length === 0 ? null : parts.join(', ') }
}
