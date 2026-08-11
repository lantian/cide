/**
 * The live "which sessions are waiting for me" table, and the two things that read it.
 *
 * `awaitingRule.ts` owns the *decision* and is compiled on its own by a check script. This
 * file owns the *plumbing*, which is the part that needs a webview: one listener on
 * `cide://session-state`, one report up to Rust when a session's answer changes, one
 * listener on `cide://session-awaiting` so every window converges on Rust's set, one ask for
 * that set at install time because the event only reports *changes*, and a
 * `useSyncExternalStore` subscription for the pane title bar's marker.
 *
 * # Why the frontend decides and Rust aggregates
 *
 * The decision needs *history* (see `awaitingRule.ts`), and history is exactly what
 * `cide://session-state` is — Rust emits one event per transition, to every window. So any
 * window can compute the answer for any session without the pane being mounted, which is
 * what makes a background project's session countable at all.
 *
 * The aggregation needs the *workspace* — which pane is in which window — and putting that
 * in the frontend would mean the shell window guessing about the contents of a detached
 * window it cannot see. Rust already holds the tree, so Rust does the arithmetic and sets
 * the titles. A webview cannot rename its own OS window in any case.
 *
 * Reports are per session and idempotent, never "here is my whole set". Three windows all
 * observe the same transitions and all report the same thing, which is harmless; a window
 * that has just opened has observed *nothing* and reports nothing, which is the point — a
 * whole-set report from a fresh window would clear every marker in the app.
 *
 * # Installation
 *
 * Lazily, from the first `useAwaiting` — that is, from the first pane title bar to render in
 * this window. Deliberately not from `App.tsx`: a feature that only works when someone
 * remembers to install it is a feature that is off until it is filed as a bug, and this app
 * has shipped three of those. Any window with a pane in it has a title bar; a window with no
 * pane has nothing to count.
 */
import { useSyncExternalStore } from 'react'
import {
  awaiting as awaitingApi,
  events,
  onSessionAwaiting,
  type SessionState,
  type Tab,
} from '@/ipc/client'
import { paneSessionId } from '@/layout/paneHosts'
import {
  UNSEEN,
  adopt,
  awaitingIn,
  mergeAuthoritative,
  onAcknowledge,
  onState,
  type PaneSession,
  type SessionPhase,
  type Track,
} from './awaitingRule'

/**
 * The equivalence check between the generated wire type and the rule module's own tag union.
 *
 * `awaitingRule.ts` cannot import the bindings — it is compiled standalone — so this
 * assignment is what fails to typecheck if `SessionState` gains or renames a variant, instead
 * of the new variant falling silently through `onState`'s switch.
 */
const phaseOf = (state: SessionState): SessionPhase => state.state

let tracks = new Map<string, Track>()
const subscribers = new Set<() => void>()

/**
 * Every subscriber is notified on every change, and each one's `getSnapshot` returns a plain
 * boolean for its own session.
 *
 * `useSyncExternalStore` compares snapshots with `Object.is`, so a per-session boolean means
 * a title bar re-renders only when *its* answer moves, however many other sessions are
 * transitioning. Handing out the `Map` instead would re-render every pane in the window on
 * every tool call of every conversation.
 */
function changed(): void {
  for (const notify of subscribers) notify()
}

function trackOf(session: string): Track {
  return tracks.get(session) ?? UNSEEN
}

/** Tell Rust, but only when the answer actually moved. */
function report(session: string, before: Track, after: Track): void {
  if (before.awaiting === after.awaiting) return
  void awaitingApi.set(session, after.awaiting).catch(() => {
    /* The title is a courtesy; a window that has gone mid-report is the ordinary case, and a
     * console line per transition would be noisier than the feature is useful. */
  })
}

function applyState(session: string, phase: SessionPhase): void {
  const before = trackOf(session)
  const after = onState(before, phase)
  if (after.gone) {
    // Dropped, not kept as `awaiting: false`. A dead session's entry would otherwise sit in
    // this map for the life of the window, and if the same pane later adopts a new session
    // the stale `ranATurn` would make its very first `Idle` claim to be waiting.
    tracks.delete(session)
  } else {
    tracks.set(session, after)
  }
  report(session, before, after)
  changed()
}

/**
 * Note that the user has dealt with this session.
 *
 * Called from a pointer-down on the pane frame and from a keystroke into the terminal —
 * genuine acts, not "the pane is on screen". A detached window sitting behind another window
 * still shows a focused pane, so anything weaker would clear the marker for the exact case
 * the feature exists to cover.
 */
export function acknowledge(session: string | null | undefined): void {
  if (!session) return
  const before = trackOf(session)
  if (!before.awaiting) return
  const after = onAcknowledge(before)
  tracks.set(session, after)
  report(session, before, after)
  changed()
}

// --- installation ---------------------------------------------------------------------

let installed = false

function install(): void {
  if (installed) return
  installed = true

  // Nothing else can supply the transitions, so a failure here leaves the feature off in this
  // window rather than wrong: no marker, and no report that could clear another window's.
  void events
    .onSessionState((session, state) => applyState(session, phaseOf(state)))
    .catch(() => {})

  void onSessionAwaiting((sessions) => {
    tracks = mergeAuthoritative(tracks, sessions)
    changed()
  })
    .catch(() => {
      /* Without the broadcast this window still tracks what it hears itself; it just will not
       * follow another window acknowledging a session both of them are showing. */
    })

  // The catch-up, and it is the other half of the broadcast rather than a nicety: that event
  // fires when the set *moves*, so a window that opens between two moves has heard nothing —
  // and a window built by a detach is precisely that. Rust has already written `Awaiting: 1`
  // into its OS title by now; without this ask the pane inside it would show no marker.
  //
  // `adopt`, not `mergeAuthoritative`: the reply describes the set as it was when the question
  // landed, so a session that started waiting during the round trip is in this window's table
  // and not in the answer. Replacing would discard it, and for a session that has just
  // finished its turn there is no later transition to raise it again.
  void awaitingApi
    .current()
    .then((sessions) => {
      tracks = adopt(tracks, sessions)
      changed()
    })
    .catch(() => {
      /* A window that cannot ask still tracks everything it hears from here on. */
    })
}

function subscribe(notify: () => void): () => void {
  install()
  subscribers.add(notify)
  return () => {
    subscribers.delete(notify)
  }
}

/**
 * Subscribe a component to one session's answer.
 *
 * Takes the session rather than the pane so a mirrored pane — two panes, one child — lights
 * up in both places, which is what the user sees anyway: one conversation, waiting.
 */
export function useAwaiting(session: string | null | undefined): boolean {
  const key = session ?? ''
  return useSyncExternalStore(
    subscribe,
    () => (key === '' ? false : trackOf(key).awaiting),
    () => false,
  )
}

/**
 * Every session held by a tab's panes, as the pane markers themselves resolve them.
 *
 * `paneSessionId ?? pane.session`, in that order, is `PaneFrame`'s rule verbatim and is
 * copied rather than approximated on purpose: if the tab's count and the pane's marker
 * disagreed about which child a pane holds, the failure is a tab that says `1` over a tab
 * full of panes with no marker — worse than no tab marker at all. The host registry answers
 * for a child that started in this process and whose id the domain records one round trip
 * later; `pane.session` answers before any host has claimed the pane, which is the first
 * paint after boot and the whole of a window that has been handed tabs it has not rendered.
 *
 * Note which branch does the work for a *background* tab: the host registry, not the
 * fallback. `TabContent` mounts every tab and hides the inactive ones with `visibility:
 * hidden` — `paneHosts` rule 2 forbids anything else — so a background pane has a live host
 * with a live session id. The fallback is about time, not about visibility.
 */
function sessionsInTab(tab: Tab): PaneSession[] {
  return Object.values(tab.tree.panes).map((pane) => paneSessionId(pane.id) ?? pane.session)
}

/**
 * How many of one tab's sessions are waiting on the user.
 *
 * Subscribing per tab rather than handing the strip one map: the snapshot is a number, so
 * `Object.is` lets a tab re-render only when *its own* count moves — six tabs and one
 * transition is one re-render, not six.
 *
 * There is deliberately **no separate acknowledgement for a tab**. Activating a tab is not
 * "I have dealt with these": a tab can hold four panes with one of them waiting, and
 * clearing on activation destroys the only pointer to that one at the moment the user could
 * finally act on it. The count is derived from the same per-session acknowledgements the
 * pane markers use — a click or a keystroke into a pane — so it falls to zero exactly when
 * the last waiting pane has actually been seen, and it cannot drift from the pane markers
 * because there is no second piece of state to drift.
 *
 * That holds because `visibility: hidden` also removes a panel from hit-testing and drops
 * the focus inside it, so a background pane cannot be pointed at or focused and cannot
 * acknowledge itself; and nothing in this app programmatically refocuses a pane when its tab
 * is activated. If either ever changes, activating a tab starts silently decrementing this
 * count for panes the user has not read, which is the failure the paragraph above is about.
 */
export function useAwaitingInTab(tab: Tab): number {
  return useSyncExternalStore(
    subscribe,
    () => awaitingIn(tracks, sessionsInTab(tab)),
    () => 0,
  )
}

