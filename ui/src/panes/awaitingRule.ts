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
  | 'paused'
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
 * * `Paused` — frozen by the user, mid-turn, with `SIGSTOP`. Deliberately **not** awaiting, on
 *   the same argument as `Splash`: the user is the one who froze it, so it is a state they are
 *   already looking at rather than news to carry to the task bar. Telling somebody "1 session is
 *   waiting for you" about the session they just paused is the noise this module exists to
 *   prevent. `ranATurn` survives the freeze — a paused turn is an unfinished turn, not an
 *   un-run one, so the `Idle` that follows a resume still means "finished".
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
    case 'paused':
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

/**
 * Adopt Rust's set into a window that has only just opened.
 *
 * A **union**, and that is the whole difference from [`mergeAuthoritative`]: this answers a
 * question the window asked, and the answer was taken before the question landed. A session
 * that started waiting in the round trip is in this window's own table and is *not* in the
 * reply, so replacing would throw away the first thing the window learned for itself — and
 * the marker would only come back on the next transition, which for a session that has
 * finished its turn is never.
 *
 * The catch-up is needed at all because `cide://session-awaiting` is a change notification.
 * A window built by a detach is opened *between* changes: Rust has already written
 * `Awaiting: 1` into its OS title, and without this the pane inside it would show nothing.
 */
export function adopt(
  tracks: ReadonlyMap<string, Track>,
  awaiting: readonly string[],
): Map<string, Track> {
  const next = new Map(tracks)
  for (const session of awaiting) {
    const track = next.get(session)
    next.set(
      session,
      track === undefined
        ? { ranATurn: true, awaiting: true, gone: false }
        : { ...track, awaiting: true },
    )
  }
  return next
}

// --- the aggregate a *tab* shows ----------------------------------------------------------
//
// A pane in a background tab is still mounted — hiding a tab is `visibility: hidden`, never an
// unmount — but nothing it draws is painted, so its marker is invisible: the OS title says one
// session wants you and nothing on screen says which. The tab strip is the one part of a
// background tab that stays visible, and it can carry the answer because the table above is
// keyed by session and lives outside React's tree — the same property that lets a detached
// window paint a marker for a pane it has never mounted.

/**
 * A pane's session as its tab knows it. `null`/`undefined` for a pane with no child — a diff
 * or an editor, or a Claude pane still showing its resume splash.
 */
export type PaneSession = string | null | undefined

/**
 * **The one derivation.** Which of a group of panes' sessions are waiting on the user.
 *
 * Every attention surface in the app is a reading of this function over a different pane set,
 * and that is a requirement rather than tidiness. There are four surfaces — a pane's own
 * highlight, its tab's badge, its project's badge in the header, and the `Awaiting: N` in the
 * OS title — and the failure this shape exists to prevent is the obvious implementation:
 * three booleans set from three places, where acknowledging a pane clears its own mark and
 * leaves a tab or a header still saying *come back here* for a pane that has been dealt with.
 * A user who sees that once stops believing the badge, and the whole feature is a badge.
 *
 * So the rule is: **no surface holds attention state.** Each one names its panes and reads the
 * answer out of this table:
 *
 * * a pane asks about itself — [`isAwaiting`], one session;
 * * a tab asks about its panes — [`awaitingIn`], the size of this set;
 * * a project asks about every pane in every one of its tabs, plus its detached ones;
 * * a window asks the same question in Rust (`cmd::window::retitle` over `sessions_of`),
 *   because only Rust knows which panes an OS window is showing.
 *
 * They cannot disagree, because there is nothing to disagree with: acknowledging one session
 * moves one entry in one map, and all four answers are recomputed from it.
 *
 * **Distinct** is not a detail either. A mirrored pane is two panes showing one child, and the
 * pane markers deliberately light up in both — that is one conversation waiting, and a tab
 * that said `2` about it would be counting windows onto a thing rather than the thing. A
 * `Set` of sessions is what makes that automatic at every level, including the project one,
 * where the same session can appear in two tabs.
 *
 * Sessions this window has never heard of contribute nothing rather than defaulting to
 * waiting: `pane.session` survives a restart, so a freshly launched app is full of pane
 * records naming children from a previous process. Absent from the table means "no news",
 * and no news must never read as "come back to this one".
 */
export function awaitingAmong(
  tracks: ReadonlyMap<string, Track>,
  sessions: readonly PaneSession[],
): ReadonlySet<string> {
  const waiting = new Set<string>()
  for (const session of sessions) {
    if (!session) continue
    // `gone` tracks are never `awaiting`, so an exited session drops out here without a
    // second condition — and the caller has usually deleted the entry already.
    if (tracks.get(session)?.awaiting === true) waiting.add(session)
  }
  return waiting
}

/**
 * How many distinct sessions in one group of panes are waiting on the user.
 *
 * The count a tab or a project badge shows. Expressed over [`awaitingAmong`] rather than
 * counting for itself, so "this tab says 2" and "these two panes are marked" are the same
 * statement rather than two implementations of it.
 */
export function awaitingIn(
  tracks: ReadonlyMap<string, Track>,
  sessions: readonly PaneSession[],
): number {
  return awaitingAmong(tracks, sessions).size
}

/**
 * Whether one pane's session is waiting on the user.
 *
 * The pane highlight, and it goes through the same function as the counts for the reason
 * above: a pane that draws a highlight its tab does not count is the half-clear this module
 * is arranged to make impossible. A one-element set is a trivial amount of work for a
 * guarantee that is otherwise a convention nobody can check.
 */
export function isAwaiting(tracks: ReadonlyMap<string, Track>, session: PaneSession): boolean {
  return awaitingAmong(tracks, [session]).size > 0
}

/**
 * The most this marker will spell out. Past it the number stops being a thing you act on and
 * starts being a thing you read, and the box it has to fit in is 13px.
 */
const BADGE_CAP = 9

/**
 * The glyph for a tab's count, or `''` when nothing is waiting.
 *
 * A **count, not a dot**, and the reason is that a tab is an aggregate where a pane is not.
 * A dot says "something behind here"; the user then activates the tab and hunts. A number
 * says how many, which is what decides whether they go now — and it *decrements* as each pane
 * is acknowledged, which is the only progress signal available while the panes are hidden and
 * their own markers are painted nowhere. A dot would sit there unchanged until the last one
 * and then vanish, which is indistinguishable from a stuck dot.
 *
 * Capped rather than allowed to grow, because the box is fixed width and a marker that
 * widened the tab it sits on would reflow the strip under the pointer — see the CSS.
 */
export function awaitingBadge(count: number): string {
  if (count <= 0) return ''
  return count > BADGE_CAP ? `${BADGE_CAP}+` : String(count)
}

/**
 * The sentence behind the marker, for the tooltip and the accessible name, or `undefined`
 * when there is nothing to say.
 *
 * Here rather than in the component because it is the one part of the marker a check script
 * can pin: the count is capped at `9+` on screen, so the tooltip is the only place a user
 * with eleven waiting sessions learns there are eleven, and "1 sessions" is the kind of thing
 * that ships. `where` names the container — the header's project tabs have exactly this
 * problem one level up and would otherwise need a second copy of the wording.
 */
export function awaitingHint(count: number, where: string): string | undefined {
  if (count <= 0) return undefined
  return count === 1
    ? `1 session in this ${where} is waiting for you`
    : `${count} sessions in this ${where} are waiting for you`
}
