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
 * **Import-free**, like the rest of the tab's pure core: `check-sessions.mjs` compiles it alone.
 */

/** The slice of a row this needs; `SessionView` satisfies it structurally. */
export interface SeenRow {
  id: string
  live: boolean
  activity: string
}

/**
 * `rows` with the seen `awaitingInput` ones read as `idle`, and the seen set pruned of every id
 * whose raw activity is no longer `awaitingInput` (or that left the list). The set is returned
 * as-is when nothing was pruned, so a store can compare it by identity.
 */
export function applySeen<T extends SeenRow>(
  raw: readonly T[],
  seen: ReadonlySet<string>,
): { rows: T[]; seen: ReadonlySet<string> } {
  let kept: Set<string> | null = null
  const rows = raw.map((r) => {
    if (r.activity !== 'awaitingInput' || !seen.has(r.id)) return r
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
