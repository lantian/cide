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
 * The decision is a fold over `cide://session-state` (see `awaitingRule.ts`) — Rust emits one
 * event per transition, to every window, so any window can compute the answer for any session
 * without the pane being mounted, which is what makes a background project's session countable
 * at all. And what only this end can supply is *acknowledgement*: a click or a keystroke into
 * a pane is a webview event Rust never sees.
 *
 * The aggregation needs the *workspace* — which pane is in which window — and putting that
 * in the frontend would mean the shell window guessing about the contents of a detached
 * window it cannot see. Rust already holds the tree, so Rust does the arithmetic and sets
 * the titles. A webview cannot rename its own OS window in any case.
 *
 * # What was actually missing, since the obvious diagnosis is wrong
 *
 * This is worth writing down plainly, because "the hook is not installed" is what everyone
 * guesses and it is not true. The `Stop` hook **is** registered in the inline `--settings`
 * every `claude` is spawned with (`cide_claude::settings`), the CLI **does** call it, the
 * `cide-hook` binary **does** deliver it over the UDS, `hooks.rs` turns it into a
 * `cide://session-state` transition, this module turns that into "waiting", Rust aggregates
 * it and writes `Awaiting: N` into the OS title. Verified end to end against a real CLI by
 * `cide-claude`'s own `#[ignore]`d integration test. Every link held.
 *
 * What was missing was on the other end: **surfaces**. The signal arrived and had almost
 * nowhere to be seen —
 *
 * * a pane could only show it as a 7px dot, because `data-awaiting` was set on the floating
 *   control cluster rather than on the pane frame, so no stylesheet could reach the panel;
 * * a *project* had no surface at all — `useAwaitingInProject` below did not exist, and
 *   `App.tsx` renders tab strips and pane trees for the active project only, so a background
 *   project's sessions were counted in the OS title and drawn nowhere;
 * * and the OS title was set correctly into a task bar that, on the default Plasma layout,
 *   does not render titles at all — nothing ever asked for the urgency hint that a task bar
 *   *does* render (`windows::demand_attention`).
 *
 * That is this project's recurring defect wearing a slightly different coat: not a feature
 * that was never built, but a complete one whose output reached no control the user could
 * see. Diagnose in that direction first.
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
  type Pane,
  type SessionState,
  type Tab,
} from '@/ipc/client'
import { paneSessionId } from '@/layout/paneHosts'
import {
  UNSEEN,
  adopt,
  awaitingIn,
  isAwaiting,
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
    // this map for the life of the window — and a pane's session id is reused when its child
    // is relaunched (that is what makes resume free), so a stale entry would be the new
    // child's starting state rather than `UNSEEN`.
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
 *
 * Goes through `isAwaiting` rather than reading `trackOf(key).awaiting` directly, which is
 * the same value by two paths and deliberately no longer written twice: the pane highlight,
 * the tab badge and the project badge are one function over a pane set, and a pane that read
 * the table its own way is the first step back towards three flags that half-clear. See the
 * module comment on `awaitingAmong`.
 */
export function useAwaiting(session: string | null | undefined): boolean {
  return useSyncExternalStore(
    subscribe,
    () => isAwaiting(tracks, session),
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

/**
 * How many of one *project's* sessions are waiting on the user.
 *
 * # This is the surface that did not exist, and it is the load-bearing one
 *
 * `App.tsx` renders `TabStrip` for the **active project only**, and mounts `TabContent` for
 * the active project only. So for a project the user is not currently in, neither the pane
 * marker nor the tab badge is painted anywhere at all — both live inside a subtree that is
 * not on screen. Rust was already putting `Awaiting: N` into the OS title, which is why the
 * gap was easy to miss from the inside and impossible to miss from the outside: the task bar
 * said one session wanted attention and nothing in the window said which project it was in.
 *
 * The header's project tab is the only element of a background project that is drawn at all,
 * so it is the only place this answer can go.
 *
 * # Detached panes count
 *
 * A pane torn into its own window is still this project's conversation, and its window can be
 * behind the shell or minimized. Its *own* window's title carries it too — `retitle` gives a
 * `DetachedPane` window its one pane and no more, so nothing is announced twice at the OS
 * level — but within this window's header the project is the whole project. A user looking at
 * the header is asking "is anything of mine waiting", not "is anything in this particular
 * arrangement of windows waiting".
 *
 * # Nothing here clears
 *
 * Same rule as the tab, one level up and for a stronger reason: activating a project is not
 * "I have dealt with these", and a project can hold six panes across four tabs with one of
 * them waiting. The count falls out of the same per-session acknowledgements — a click or a
 * keystroke into a pane — so it reaches zero exactly when the last waiting pane has been
 * *seen*, and there is no second piece of state that could disagree with the pane markers or
 * with the OS title.
 */
export function useAwaitingInProject(project: ProjectPanes): number {
  return useSyncExternalStore(
    subscribe,
    () => awaitingIn(tracks, sessionsInProject(project)),
    () => 0,
  )
}

/**
 * Everywhere a project keeps a pane. Satisfied by the generated `Project`.
 *
 * Both fields optional, so `chrome/AppHeader.tsx`'s `ProjectTab` — which a measurement
 * fixture builds from four scalars — is accepted too and answers zero. A required `tabs`
 * would make every fixture construct a whole pane tree in order to take a ruler to a 34px
 * bar, and the alternative that lost was a second, narrower pane type declared here, which is
 * a hand-written copy of a generated shape and drifts from Rust the first time `Pane` changes.
 */
export interface ProjectPanes {
  tabs?: readonly Tab[] | undefined
  detached?: Readonly<Record<string, Pane>> | undefined
}

/**
 * Every session a project holds, across its tabs and its torn-out panes.
 *
 * Reuses `sessionsInTab` verbatim so a project and its own tab strip cannot resolve a pane's
 * child differently — `awaitingAmong` deduplicates, so a session that appears in two tabs, or
 * in a tab and in a mirror, is one waiting conversation and not two.
 */
function sessionsInProject(project: ProjectPanes): PaneSession[] {
  return [
    ...(project.tabs ?? []).flatMap(sessionsInTab),
    ...Object.values(project.detached ?? {}).map(
      (pane) => paneSessionId(pane.id) ?? pane.session,
    ),
  ]
}

