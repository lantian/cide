/**
 * "The user has looked at it" for a session that finished its turn. (M200)
 *
 * Rust's `activity` says `awaitingInput` from the `Stop` hook until the *next* `Busy` — it has no
 * idea the user opened the pane and read the closing note. So a finished agent kept reading
 * "Waiting for your input" and kept the tab pill and the rail's amber number raised after the
 * user had done the only thing it asked: look. That is a *notification*, not a debt, and a
 * notification ends when it is read.
 *
 * This is the one place the webview lays that over Rust's answer, as the question overlay is laid
 * over it in `sessionsModel.rowStatus`. It is applied **to the rows as they are adopted**
 * (`sessionsStore`), so every reader — the glyph, the pill, the rail, the idle sweep — sees
 * `idle`, and none has a second rule to forget.
 *
 * * Only `awaitingInput` is downgraded. `permission` and a question are real debts: the agent is
 *   stopped on the user, and looking does not pay them.
 * * A seen mark is **dropped the moment the raw activity is anything but `awaitingInput`**. The
 *   next turn busies the session, and when it ends waiting is raised afresh instead of being born
 *   already read.
 *
 * Deliberately looser than `panes/awaiting.ts`, which refuses to clear on "the tab is shown" so a
 * detached window behind another window keeps its marker. This row only says *go look*; once the
 * user has focused or opened the pane they have, and the pane's own highlight is the stricter
 * signal and is untouched.
 *
 * # The pane's answer counts too (M205)
 *
 * Looser, never *stricter*: a row whose pane marker the user has already cleared must not go on
 * saying "Waiting for your input". Until M205 it could, because the seen set was this module's
 * own second copy of "has the user dealt with it" — the exact shape `awaitingRule.ts` is arranged
 * against, flags on several surfaces that half-clear. Three roads led to it: the acknowledgement
 * listener lived in the Agents panel, so a click into a pane while another sidebar panel showed
 * reached nobody; a click in the 200 ms and one listing between the `Stop` and the row's
 * `awaitingInput` found the row still `working` and marked nothing; and a project switch dropped
 * every mark. So `applySeen` also asks `settled` — `awaiting.ts::isSettled`, the very table the
 * pane highlight, the tab badge and the OS title read — and a row whose live child that table says
 * is no longer waiting reads `idle` whatever the seen set holds. The table hears the transition
 * before the row does, so the order the two arrive in no longer matters.
 *
 * **Import-free**, like the rest of the tab's pure core: `check-sessions.mjs` compiles it alone.
 */

/** The slice of a row this needs; `SessionView` satisfies it structurally. */
export interface SeenRow {
  id: string
  live: boolean
  activity: string
  /** The live child's handle — the key `settled` is asked by. */
  liveSession: string | null
}

/** Whether the pane-level table holds this child and says it is not waiting (`awaiting.ts::isSettled`). */
export type Settled = (handle: string) => boolean

/** For a caller with no pane table: nothing is settled. */
export const NOTHING_SETTLED: Settled = () => false

/**
 * `rows` with the seen `awaitingInput` ones read as `idle` — seen here, or settled in the pane
 * table (see the header) — and the seen set pruned of every id whose raw activity is no longer
 * `awaitingInput` (or that left the list). `settled` neither adds to the set nor keeps a mark in
 * it: the table raises the next turn's end by itself. The set is returned as-is when nothing was
 * pruned, so a store can compare it by identity.
 */
export function applySeen<T extends SeenRow>(
  raw: readonly T[],
  seen: ReadonlySet<string>,
  settled: Settled = NOTHING_SETTLED,
): { rows: T[]; seen: ReadonlySet<string> } {
  let kept: Set<string> | null = null
  const rows = raw.map((r) => {
    if (r.activity !== 'awaitingInput') return r
    if (!seen.has(r.id) && !(r.liveSession !== null && settled(r.liveSession))) return r
    return { ...r, activity: 'idle' }
  })
  const waiting = new Set(raw.filter((r) => r.activity === 'awaitingInput').map((r) => r.id))
  for (const id of seen) {
    if (!waiting.has(id)) {
      kept ??= new Set(seen)
      kept.delete(id)
    }
  }
  return { rows, seen: kept ?? seen }
}

/** Whether `id` is a row that is waiting on the user for nothing but to be looked at. */
export function canMarkSeen(raw: readonly SeenRow[], id: string): boolean {
  return raw.some((r) => r.id === id && r.live && r.activity === 'awaitingInput')
}

/**
 * The live sessions an "end idle sessions" sweep would close: alive, turn over, nothing asked.
 * `rows` are the **displayed** rows (seen already applied) and `questioned` the conversations with
 * an open question; a run still owned by an agent run is left to the Subagents tab (`run`), and the project's
 * pinned console (`kind: 'console'`) is never swept — it is the one conversation that cannot be closed.
 */
export function idleLive<T extends SeenRow & { kind: string; run: string | null; liveSession: string | null }>(
  rows: readonly T[],
  questioned: ReadonlySet<string>,
): T[] {
  return rows.filter(
    (r) => r.live && r.liveSession !== null && r.run === null && r.kind !== 'console' && r.activity === 'idle' && !questioned.has(r.id),
  )
}
