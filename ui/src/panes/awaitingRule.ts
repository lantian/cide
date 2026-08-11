/**
 * When a session is *waiting for the user*, as opposed to merely not busy.
 *
 * The user's request was "notify me when it finished processing and is waiting for me". The
 * hard part is not the notification, it is the predicate: `SessionState` alone cannot answer
 * it. `Busy` is plainly not waiting and `AwaitingPermission` plainly is, but `Idle` is two
 * completely different situations wearing one name —
 *
 * * a session that started and has never been asked anything (`SessionStart` → `Idle`), which
 *   is *not* waiting on the user in any sense worth interrupting them for; and
 * * a session that has just finished a turn (`Stop` → `Idle`), which is exactly the moment
 *   the user asked to be told about.
 *
 * The two are indistinguishable in the state and completely distinguishable in the *history*:
 * the second one has been `Busy`. So this module tracks one extra bit per session — has it
 * ever run a turn — and that bit is the whole difference. Nothing on the Rust side had to
 * change for it, because `cide://session-state` is emitted per transition and a transition is
 * precisely a piece of history.
 *
 * The alternative that lost was teaching the hook server a fifth state (`Finished`). It is a
 * bigger change for the same information, it would have to be persisted for a session the
 * hooks lose track of, and it puts a UI notion — "the user has not looked yet" — into a state
 * machine that is otherwise a faithful model of what Claude Code is doing.
 *
 * DOM-free and import-free on purpose, in the same way and for the same reason as
 * `menus/model.ts` and `panes/exitMarker.ts`: `ui/scripts/check-awaiting.mjs` compiles this
 * file with a bare `tsc` and drives it, and there is no JS test runner in this repo. Do not
 * import React, `@/…` or a DOM type into this file.
 */

/**
 * The tag of `SessionState`, which is the only part of it this decision reads.
 *
 * Written out rather than imported from the generated bindings so the check script can
 * compile this file alone. The assignment in `awaiting.ts` is the equivalence check: a
 * variant renamed in Rust stops typechecking there rather than falling silently through the
 * switch below.
 */
export type SessionPhase =
  | 'spawning'
  | 'splash'
  | 'idle'
  | 'busy'
  | 'awaitingPermission'
  | 'awaitingInput'
  | 'exited'

/** What is remembered about one session. */
export interface Track {
  /** Whether this session has ever been observed mid-turn. The bit `Idle` cannot supply. */
  readonly ranATurn: boolean
  /** Whether it is waiting on the user *right now*, unacknowledged. */
  readonly awaiting: boolean
  /** Set once the child is gone, so the caller can drop the entry. */
  readonly gone: boolean
}

/** A session nothing has been heard about yet. */
export const UNSEEN: Track = { ranATurn: false, awaiting: false, gone: false }

/**
 * Fold one `cide://session-state` transition into a session's track.
 *
 * `Idle` is the interesting arm and the only one that consults history. Everything else is
 * decided by the state alone:
 *
 * * `AwaitingPermission` / `AwaitingInput` — the turn has stopped *on* the user. Waiting even
 *   if this is the first turn, which is why neither consults `ranATurn`.
 * * `Busy` — plainly not waiting, and it is also what arms the bit: from here, the next
 *   `Idle` means "finished".
 * * `Spawning` / `Splash` — the session has not started; a splash is waiting for a click, but
 *   it is a control the user is already looking at rather than news to carry to the task bar.
 * * `Exited` — nothing is waiting for anybody. Reported so the caller drops the entry rather
 *   than leaving a dead session counted for the life of the window.
 */
export function onState(track: Track, phase: SessionPhase): Track {
  switch (phase) {
    case 'busy':
      return { ranATurn: true, awaiting: false, gone: false }
    case 'awaitingPermission':
    case 'awaitingInput':
      return { ranATurn: true, awaiting: true, gone: false }
    case 'idle':
      // The whole point of the module. An `Idle` that has never been `Busy` is a session
      // sitting at a fresh prompt, and telling the user "1 session is waiting for you" about
      // it on every launch is how a notification becomes noise they stop reading.
      return { ranATurn: track.ranATurn, awaiting: track.ranATurn, gone: false }
    case 'spawning':
    case 'splash':
      return { ranATurn: track.ranATurn, awaiting: false, gone: false }
    case 'exited':
      return { ranATurn: track.ranATurn, awaiting: false, gone: true }
  }
}

/**
 * The user has dealt with this session: clicked into its pane, or typed at it.
 *
 * `ranATurn` deliberately survives. Acknowledging is "I have seen this one", not "this
 * session has never run" — the next `Stop` has to raise the marker again, and clearing the
 * bit here would mean a session only ever announced itself once.
 *
 * There is no timer and no "seen because it is on screen". A marker that cleared itself after
 * a few seconds would be gone by the time the user came back to the machine, which is the
 * case the whole feature exists for.
 */
export function onAcknowledge(track: Track): Track {
  return { ranATurn: track.ranATurn, awaiting: false, gone: track.gone }
}

/**
 * Fold a whole `cide://session-awaiting` broadcast into a local table.
 *
 * Rust holds the authoritative set — it is the only place that sees every window's reports —
 * and re-broadcasts it so a window that opened after the fact still paints the marker. This
 * merges that set in without losing `ranATurn`, which is per-window history the broadcast
 * cannot carry and which the *next* `Idle` in this window depends on.
 *
 * Sessions absent from the broadcast are cleared rather than left alone: the set is complete
 * by construction, so absence is the message "no longer waiting" — which is how one window
 * acknowledging a session clears its marker in the other window showing the same pane.
 */
export function mergeAuthoritative(
  tracks: ReadonlyMap<string, Track>,
  awaiting: readonly string[],
): Map<string, Track> {
  const wanted = new Set(awaiting)
  const next = new Map<string, Track>()
  for (const [session, track] of tracks) {
    if (track.gone) continue
    next.set(session, { ...track, awaiting: wanted.has(session) })
  }
  for (const session of wanted) {
    if (!next.has(session)) next.set(session, { ranATurn: true, awaiting: true, gone: false })
  }
  return next
}
