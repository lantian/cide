/**
 * What the activity rail's Agents button draws, and what it is called. (M18, M132)
 *
 * The button carries **two marks that answer different questions**, and this is the one place
 * the rule between them is written:
 *
 * - the **run pill** (top right): how many subagent runs are live. Red when one of them is
 *   stopped at a permission prompt — `agentsAwaiting`, which recolours the pill and can never
 *   draw one by itself (no live run, no pill to recolour);
 * - the **waiting mark** (bottom right): tasks on the board wait for the user — accepted by eye
 *   and in review, or carrying an open question. The Agents panel's Waiting tab lists them.
 *
 * **Either can be drawn without the other**, and that independence is the point. A question
 * outlives the run that asked it (no live run, so no pill — and that is exactly when a task
 * waits longest unnoticed), and runs work with nothing owed (pill, no mark). The two wrong
 * versions both type-check and both look right in the state somebody screenshots: the mark
 * gated on the pill, which hides it in the first case, and the mark folded into the pill's tone,
 * which cannot tell "a run is stuck on a prompt" from "a task wants an answer".
 *
 * Pure and **import-free**, so `check-rail-marks.mjs` compiles this file alone with a bare `tsc`
 * and runs it under node: `ActivityRail.tsx` cannot be rendered there (`@/menus` reaches
 * xterm and the DOM at import time), so the decision lives here and the component draws it.
 * `digits` is a parameter for the same reason — the rail passes `groupDigits`, which lives in a
 * module this one may not import.
 */

export interface AgentsMarks {
  /** The pill's count, or `null` for no pill. Only a positive, known count draws one. */
  runs: number | null
  /** The pill is drawn red: a run is stopped at a permission prompt. Never without a pill. */
  blocked: boolean
  /** The waiting mark's count, or `null` for no mark. Independent of `runs`. */
  waiting: number | null
  /** What follows "Agents — " in the button's name, or `null` when there is nothing to add. */
  name: string | null
}

/**
 * The Agents button's marks.
 *
 * `null` and `0` both draw nothing, for every count, and the rail's standing reason: `null` is
 * nobody looked (subagents off, no tracker, nothing read yet) and `0` is looked and there is
 * nothing, and a mark is a call to action — neither case has anything to act on.
 */
export function agentsMarks(
  agents: number | null | undefined,
  awaiting: boolean | undefined,
  waiting: number | null | undefined,
  digits: (n: number) => string = String,
): AgentsMarks {
  const runs = typeof agents === 'number' && agents > 0 ? agents : null
  const owed = typeof waiting === 'number' && waiting > 0 ? waiting : null
  const blocked = runs !== null && awaiting === true
  const parts: string[] = []
  if (runs !== null) {
    const counted = `${digits(runs)} ${runs === 1 ? 'run' : 'runs'}`
    parts.push(blocked ? `${counted}, one blocked on a permission` : counted)
  }
  // "task", never a bare "1 waiting for you": beside a run count, a bare number reads as runs.
  if (owed !== null) parts.push(`${digits(owed)} ${owed === 1 ? 'task' : 'tasks'} waiting for you`)
  return { runs, blocked, waiting: owed, name: parts.length === 0 ? null : parts.join(', ') }
}
