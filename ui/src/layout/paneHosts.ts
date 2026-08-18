/**
 * Pane host registry — the load-bearing thirty lines of this application.
 *
 * A pane's DOM must outlive its position in the React tree. Panes move between splits,
 * between workspace tabs, and into detached windows; a terminal whose DOM node is
 * destroyed loses its scrollback, its selection, its renderer and its in-flight turn.
 * xterm.js has no serialise-and-restore that survives that.
 *
 * So hosts live in a module-level map outside React entirely, and `PaneSlot` merely
 * *moves* them with `appendChild`. Unmounting parks a host in a detached div; it is never
 * removed. React re-renders reposition an element that has existed since the pane was
 * created.
 *
 * Five rules, each of which is a bug if broken:
 *
 * 1. Inactive workspace tabs use `visibility: hidden`, never `display: none`. With
 *    `display: none`, xterm's IntersectionObserver reports non-intersecting *and* every
 *    measurement reads zero, so `fit()` computes garbage and the next resize corrupts the
 *    child's idea of the terminal size.
 * 2. Never conditionally render a pane, and never `replaceChildren` a slot.
 * 3. `term.open()` runs exactly once per pane, ever. The only exception is a host that was
 *    evicted, which is counted separately so a second open can be told apart from a bug.
 * 4. Live hosts are capped; eviction only ever takes idle panes, and evicted panes
 *    rehydrate from the Rust screen mirror.
 * 5. `releaseHost` is for a pane that has left this window's tree with its session still
 *    running; `destroyHost` is for a pane that is finished. Destroying the first kind
 *    throws away the view of a live turn and the id it would have re-attached with.
 */
import { createTerminal, promoteWebgl, releaseWebgl, type TerminalHandle } from '@/terminal/xterm'
import { attachInputProbe, attachInputRouting } from '@/terminal/inputHost'
import { attachPathLinks } from '@/terminal/pathLinks'
import type { TerminalPaneKind } from '@/terminal/keys'
import { diag } from '@/ipc/client'

export interface PaneHost {
  readonly paneId: string
  readonly el: HTMLDivElement
  terminal?: TerminalHandle
  /** True once `term.open()` has run. Checked so it can never run twice. */
  opened: boolean
  /**
   * Set once `— exited —` has been written into this pane.
   *
   * On the host rather than in React state because the host outlives every mount: a pane
   * that is switched away from and back, detached, or evicted and rehydrated must not print
   * the marker a second time.
   */
  exitMarked?: boolean
  /** Set when a released host must clear its terminal before re-reading the mirror. */
  needsReset: boolean
  /**
   * True once the Rust screen mirror has been written into this terminal.
   *
   * The mirror is the whole scrollback, and it is correct exactly once — when a terminal
   * first adopts a session it did not watch from the start. Every later mount already holds
   * those bytes, so writing them again appends a second copy of the transcript. A split or
   * a close remounts the surviving leaf, so without this flag the routine act of splitting
   * a pane duplicates the neighbouring pane's history on screen.
   */
  hydrated: boolean
  /**
   * The session this pane is attached to *in this process*.
   *
   * `| undefined` explicitly rather than by omission, because under
   * `exactOptionalPropertyTypes` those are different types and this one has to be *assignable*
   * to `undefined`: `forgetSession` clears it when a pane restarts.
   */
  sessionId?: string | undefined
  cleanup: Array<() => void>
  /** True while the element sits in a live slot. A mounted host is never evicted. */
  mounted: boolean
  /**
   * `performance.now()` at the last mount *or park* — the moment this host last stopped
   * being on screen. Stamping only at mount would order eviction by when a pane was opened
   * rather than by when it was last used, so a console pane held open all session would be
   * the first victim the moment it was briefly parked.
   */
  lastUsed: number
  /**
   * Whether this pane's session is mid-turn, set by whoever tracks session state (M7's
   * `cide://session-state`). A busy host is never evicted: rehydrating from the screen
   * mirror is lossless for a settled pane, but a turn in flight is bytes arriving now.
   */
  busy: boolean
  /** True once the pane left this window's tree while its session stayed alive. */
  released: boolean
  /**
   * The geometry last pushed at this pane's child.
   *
   * `syncSize` runs from a `ResizeObserver`, which fires on every frame of a window drag, and
   * `session_resize` is deliberately synchronous — `vt100::Screen::set_size` reflows the whole
   * scrollback under a lock on the thread that receives IPC messages, so a drag was queueing
   * one full reflow per frame per pane behind the keystrokes. Most of those callbacks report
   * the *same* cell size, because a pixel change smaller than one cell is not a resize at all.
   * Comparing here is what turns a drag into one call per actual size change.
   *
   * On the host rather than in the component: the host is what survives a remount, and a
   * re-mounted pane whose size has not changed must not re-send either.
   *
   * **It means "what this host last told the child, with no other writer since", and every
   * other writer must clear it.** There are three besides `syncSize`: `session_attach`
   * resizes the child unconditionally (`TerminalPane`'s attach path clears it), a font change
   * resizes every terminal (`settings/useSettings.ts` clears it), and any *other* pane — a
   * mirror, another window, a re-dock — can resize the same session while this host is out of
   * the tree (`parkHost` and `releaseHost` clear it). A cache that outlives a write it did not
   * make does not merely go stale: it makes the next `syncSize` skip the correction, so the
   * child keeps drawing frames for a geometry the viewport has not got until the pane's pixel
   * size happens to change.
   */
  lastGeometry?: { cols: number; rows: number; cellWidth: number; cellHeight: number } | undefined
}

/**
 * How many hosts may be resident at once, mounted or parked.
 *
 * Each one holds an xterm instance and may hold a WebGL context, and WebKitGTK caps
 * concurrent contexts at roughly 8-16. Twelve is above any plausible working set — the
 * mock's grid is four — and low enough that a session spent splitting does not accumulate
 * renderers until the pane that matters stops painting.
 */
export const HOST_CAP = 12

const hosts = new Map<string, PaneHost>()

/**
 * What is known about a pane beyond the lifetime of its host.
 *
 * The instrumented claim is about every open that *ever* happened for a pane id, so the
 * counters cannot live on the host they are counting. Pane ids are UUIDs and never
 * reused, so an entry here is meaningful for as long as the window lives; it is three
 * numbers per pane ever created, which is not a size worth managing.
 */
interface PaneRecord {
  opens: number
  evictions: number
  /** Survives eviction; cleared when the pane is genuinely finished. */
  sessionId?: string | undefined
}

const ledger = new Map<string, PaneRecord>()

/** Things that should never happen, counted rather than thrown so the audit can read them. */
const faults = { destroyedWhileMounted: 0, openedWhileParked: 0 }

function record(paneId: string): PaneRecord {
  const existing = ledger.get(paneId)
  if (existing) return existing
  const fresh: PaneRecord = { opens: 0, evictions: 0 }
  ledger.set(paneId, fresh)
  return fresh
}

/** Monotonic, so a wall-clock jump mid-session cannot invert the eviction order. */
const now = () => performance.now()

/**
 * Parked hosts live here. This div is deliberately never appended to `document.body`:
 * its children keep their DOM identity and their state, but cost nothing to lay out.
 */
const parking = document.createElement('div')
parking.id = 'cide-parked-panes'

export function getHost(paneId: string): PaneHost {
  const existing = hosts.get(paneId)
  if (existing) return existing

  const el = document.createElement('div')
  el.dataset.paneId = paneId
  el.style.position = 'absolute'
  el.style.inset = '0'
  el.style.overflow = 'hidden'

  const host: PaneHost = {
    paneId,
    el,
    opened: false,
    hydrated: false,
    needsReset: false,
    cleanup: [],
    mounted: false,
    lastUsed: now(),
    busy: false,
    released: false,
  }

  // A host that was evicted comes back knowing its session. The child is owned by Rust and
  // is still running; a re-created host that had forgotten the id would spawn a second one
  // and orphan the first, which for a Claude pane is a duplicated conversation and bill.
  const prior = ledger.get(paneId)
  if (prior?.sessionId !== undefined) host.sessionId = prior.sessionId

  hosts.set(paneId, host)
  parking.appendChild(el)
  scheduleEviction()
  return host
}

/** Look a host up without creating one. */
export function peekHost(paneId: string): PaneHost | undefined {
  return hosts.get(paneId)
}

/**
 * The session this pane holds *in this process*, host resident or not.
 *
 * The ledger as well as the map, deliberately. `peekHost` alone answers "no" for a pane
 * whose host was evicted while parked, and the one caller that asks — `PaneBody`, deciding
 * whether to show the Resume splash — would then offer to resume a conversation that is
 * already running in a terminal one tab away. `destroyHost` clears the ledger's copy, so a
 * pane the user actually closed answers `undefined` again.
 *
 * Distinct from the domain's `pane.session`, which survives a restart and therefore names a
 * child from a *previous* process. This is the one that means "a child is running now".
 */
export function paneSessionId(paneId: string): string | undefined {
  return hosts.get(paneId)?.sessionId ?? ledger.get(paneId)?.sessionId
}

/**
 * Create this pane's terminal and open it into its host, once.
 *
 * Returns the existing handle on every later call. xterm's own guidance is that `open()`
 * only needs repeating when the element moves to a different *browser window* — moving
 * within one document, which is all we ever do, keeps the renderer intact.
 */
export function ensureTerminal(paneId: string, kind: TerminalPaneKind): TerminalHandle {
  const host = getHost(paneId)
  if (host.terminal) return host.terminal

  const handle = createTerminal(kind, paneId)
  host.terminal = handle
  return handle
}

/** Open the terminal into the host element. Safe to call repeatedly; acts once. */
export function openTerminal(paneId: string, kind: TerminalPaneKind): TerminalHandle {
  const host = getHost(paneId)
  const handle = ensureTerminal(paneId, kind)
  if (host.opened) return handle

  if (!host.el.isConnected) {
    // Opening into the parking div gives xterm a zero-sized element to measure, and rule 3
    // means there is no second open to correct it — the pane would sit at whatever the
    // first `fit()` invented. Counted rather than refused, because a host that never opens
    // is a blank pane with no explanation.
    faults.openedWhileParked += 1
    console.warn(`[cide] pane ${paneId}: term.open() on a parked host`)
  }

  handle.term.open(host.el)
  host.opened = true
  record(paneId).opens += 1

  // After `open()`, because both of these need the textarea xterm creates there. Registered
  // in `cleanup` so `teardown` takes them down — a listener left on an evicted host's element
  // would keep the whole terminal reachable.
  //
  // The probe goes first, and that ordering is load-bearing: both listen on this element in
  // the capture phase, listeners on one element fire in registration order, and the guard
  // empties the textarea and calls `stopPropagation()`. Registered the other way round, the
  // probe would report an already-cleared textarea and would never see a swallowed event.
  host.cleanup.push(
    attachInputProbe(handle, host.el, paneId, (line) => void diag.log(line).catch(() => {})),
  )
  host.cleanup.push(attachInputRouting(handle, host.el))

  /*
   * File links, third and last on this element. Its `mousedown` listener is also in capture and
   * also relies on being an ancestor of everything xterm owns, but it touches no keyboard event
   * and the two above touch no mouse event, so the order between them is free.
   *
   * `session` is read through the host rather than captured: the pane may not have spawned yet
   * when this runs, and a pane that respawns holds a different id by the time somebody hovers a
   * path in it. What the *project* is, and what a click should do, arrive separately from React
   * through `setPathLinkEnv` — those are props, and props change without the host changing.
   */
  host.cleanup.push(
    attachPathLinks(handle, host.el, {
      paneId,
      session: () => getHost(paneId).sessionId ?? null,
    }),
  )

  promoteWebgl(handle)
  return handle
}

/** Move a host into a live slot. */
export function mountHost(paneId: string, slot: HTMLElement): void {
  const host = getHost(paneId)
  const redocked = host.released
  host.mounted = true
  host.released = false
  host.lastUsed = now()
  if (host.el.parentElement !== slot) slot.appendChild(host.el)

  // `releaseHost` handed this pane's WebGL context back while it was out of the tree, and
  // `openTerminal` grants one only on the first open ever — so without this a re-docked pane
  // stays on the DOM renderer for the rest of the session while panes nobody is looking at
  // keep theirs. After the append, not before: the addon builds its context against a
  // rendered element, and a line ago this one was detached in parking.
  if (redocked && host.terminal) promoteWebgl(host.terminal)
}

/**
 * Settle a host's input state before its element is re-parented.
 *
 * Re-parenting a focused textarea blurs it implicitly, and an implicit blur is the one path
 * that leaves xterm's `CompositionHelper._isComposing` set: it is cleared only from
 * `_finalizeComposition`, which is reachable from `compositionend` and from a non-229 keydown
 * while composing, and neither happens to an element being moved. A composition that is
 * never finalised keeps `onRender` calling `getBoundingClientRect()` on the composition view
 * every frame — a forced synchronous layout in the render path, on every pane, for the rest
 * of the session.
 *
 * Blurring *first* gives the browser somewhere to fire `compositionend`, and
 * `_handleTextAreaBlur` empties the textarea on the way through, which is the invariant
 * `inputRouting.ts` maintains anyway. The guard's own two bits are reset with it: they
 * describe a keystroke in flight, and a pane the user has navigated away from has none.
 */
function quiesce(host: PaneHost): void {
  if (!host.terminal) return
  host.terminal.input.reset()
  try {
    host.terminal.term.blur()
  } catch {
    // A terminal that was never opened has no textarea to blur; nothing to settle either.
  }
}

/** Return a host to parking. Never destroys it. */
export function parkHost(paneId: string): void {
  const host = hosts.get(paneId)
  if (!host) return
  host.mounted = false
  host.lastUsed = now()
  // The same rule `releaseHost` states below, and for the same reason: `lastGeometry` means
  // "what this host last told the child, with no other writer since", and a parked host has
  // no way of knowing. A mirror of the same session in another tab or another window, a
  // `session_attach` from any pane, or a font change all resize the child; the cache would
  // then let the next `syncSize` decide the child already has a size it does not, and a pane
  // that comes back to a mismatched geometry is a TUI drawing frames for rows the viewport
  // has not got. The cost is one `session_resize` the next time this host is mounted and
  // measured — a split, a project switch, a re-dock; *not* a tab switch, which never unmounts
  // a slot (`TabContent` hides panels with `visibility`) and so never reaches here.
  host.lastGeometry = undefined
  if (host.el.parentElement !== parking) {
    quiesce(host)
    parking.appendChild(host.el)
  }
}

/**
 * Park a host whose pane has left this window's tree but whose session is still alive —
 * a pane detached into another window, or moved to another tab that has not mounted it yet.
 *
 * Distinct from `destroyHost` because the two differ in what happens to the child: this
 * keeps the terminal, its buffer and its session id, so re-docking is a re-mount rather
 * than a respawn. The WebGL context does go back, since a pane this window is no longer
 * showing has no claim on a budget of roughly a dozen; xterm falls back to the DOM
 * renderer without touching the buffer.
 */
export function releaseHost(paneId: string): void {
  const host = hosts.get(paneId)
  if (!host) return
  host.released = true
  host.mounted = false
  host.lastUsed = now()
  // The pane has left this window: another one is showing it now, and its child keeps
  // printing where this terminal cannot see. Clearing the flag is what makes a re-docked
  // pane read the screen mirror again — without it the terminal comes back holding only
  // what it saw before the detach, and every byte from the detached period is gone with no
  // sign that anything is missing.
  host.hydrated = false
  // The mirror is the whole screen, not a delta, so re-hydrating has to *replace* what the
  // terminal holds rather than append to it. Without the reset the pane comes back showing
  // its pre-detach transcript followed by a second copy of the current screen.
  host.needsReset = true
  // Another window has been resizing this session in the meantime, so what this host last
  // sent says nothing about what the child currently thinks its size is.
  host.lastGeometry = undefined
  if (host.terminal) releaseWebgl(host.terminal)
  if (host.el.parentElement !== parking) {
    quiesce(host)
    parking.appendChild(host.el)
  }
}

/**
 * Tear a pane down for good, e.g. when the pane itself is closed.
 *
 * Disposing the terminal is what returns the WebGL context. Leaking one here is how the
 * renderer budget is exhausted after a few dozen splits, and the symptom is not an error:
 * a pane simply stops painting, on WebKitGTK, on some drivers.
 */
export function destroyHost(paneId: string): void {
  // Cleared before the resident check, not after `teardown`. A pane whose host was evicted
  // has no entry in `hosts` at all, but the ledger still holds the session id eviction
  // parked there — and `TerminalPane` calls `getHost` from async continuations, so a late
  // spawn or exit poll would resurrect a host already carrying the session of a pane the
  // user has closed. The counters stay: the claim they support is about the pane's whole
  // life, not about hosts that happen to be resident.
  const entry = ledger.get(paneId)
  if (entry) entry.sessionId = undefined

  const host = hosts.get(paneId)
  if (!host) return
  teardown(host)
}

function teardown(host: PaneHost): void {
  if (host.mounted) {
    // The one thing this module exists to prevent: the element is on screen, so removing
    // it destroys a visible terminal mid-frame. Whoever called this should have released
    // the pane, or waited for React to unmount its slot.
    faults.destroyedWhileMounted += 1
    console.error(`[cide] pane ${host.paneId}: host destroyed while mounted`)
  }
  for (const fn of host.cleanup) fn()
  host.cleanup.length = 0
  host.terminal?.dispose()
  host.el.remove()
  hosts.delete(host.paneId)
}

let sweepQueued = false

/**
 * Run the sweep once the current commit has finished, never inside `getHost`.
 *
 * React re-parents a pane by deleting its slot and inserting a new one in the same commit,
 * and it runs the deleted subtree's layout cleanups during the mutation phase — before any
 * inserted subtree's layout effect. So at the instant a split creates the new pane's host,
 * the pane being split is parked and `mounted` is false. A sweep running there judges the
 * middle of a commit rather than the screen, and picks the terminal the user is looking at:
 * with twelve panes open, splitting one would evict that very pane every time. A microtask
 * runs after the whole commit, by which point every `mountHost` has been called and
 * `mounted` means what it says.
 *
 * The cost is that `hostCount()` may sit one or two above `HOST_CAP` until the stack
 * unwinds. Nothing reads it inside a commit, and the audit reads it after `settle()`.
 */
function scheduleEviction(): void {
  if (sweepQueued) return
  sweepQueued = true
  queueMicrotask(() => {
    sweepQueued = false
    evictBeyondCap()
  })
}

/**
 * Drop the least recently used idle host until the registry is back within the cap.
 *
 * Eviction is safe because the Rust core, not the webview, owns the session and its screen
 * mirror: a re-mounted pane repaints from `session.scrollback`. It is not free — that is a
 * round trip and a second `term.open()` — so it only ever takes a host that is neither
 * mounted nor mid-turn, preferring ones whose pane has already left the tree.
 *
 * If every resident host is mounted or busy the cap simply yields. Evicting one of those
 * to satisfy a number would be the bug, not the fix.
 */
function evictBeyondCap(): void {
  while (hosts.size > HOST_CAP) {
    const victim = evictionCandidate()
    if (!victim) return
    const entry = record(victim.paneId)
    entry.evictions += 1
    if (victim.sessionId !== undefined) entry.sessionId = victim.sessionId
    teardown(victim)
  }
}

function evictionCandidate(): PaneHost | undefined {
  let best: PaneHost | undefined
  for (const host of hosts.values()) {
    if (host.mounted || host.busy) continue
    if (!best) {
      best = host
      continue
    }
    // A released host is a view of a pane this window no longer shows, so it goes before
    // any merely-parked host however recently it was used.
    if (host.released !== best.released) {
      if (host.released) best = host
      continue
    }
    if (host.lastUsed < best.lastUsed) best = host
  }
  return best
}

/**
 * Mark a pane's session as mid-turn, which makes its host ineligible for eviction.
 *
 * Called from `TerminalPane`'s `cide://session-state` subscription. It had **no caller at all**
 * until then: the flag was documented as "set by whoever tracks session state (M7's
 * `cide://session-state`)" and nobody did, so `busy` was permanently `false` and
 * `evictionCandidate` was free to take the one pane with a turn in flight — the exact case the
 * field exists to protect, and the one where rehydrating from the screen mirror is not lossless
 * because the bytes are arriving now.
 */
export function setHostBusy(paneId: string, busy: boolean): void {
  const host = hosts.get(paneId)
  if (host) host.busy = busy
}

/**
 * Forget the session this pane was holding, and ready the host for the next one.
 *
 * For a restart and for the recovery path in `TerminalPane`: the child is gone (or is being
 * killed on purpose) and this pane is about to spawn or resume another one, in the *same*
 * terminal. Both copies of the id have to go — the host's and the ledger's — or `getHost` would
 * hand the id straight back after an eviction and `paneSessionId` would keep naming a corpse to
 * `PaneBody` and the command dispatcher.
 *
 * `exitMarked` is cleared so the next exit prints its own marker with its own code; leaving it
 * set is how a pane restarted twice ends up showing one `— exited —` for two dead children.
 *
 * **`needsReset` is deliberately *not* set**, which is the difference between this and
 * [`releaseHost`]. A released pane is coming back to a session that ran on without it, so its
 * terminal holds a stale screen that the mirror must replace. A restarted pane's terminal holds
 * the transcript of the child that just died — including the `— exited —` line and its status,
 * which is the thing the user is looking at when they press the button — and the new child's
 * screen belongs *below* it, exactly as re-running a command in a shell leaves the previous run
 * on screen. `hydrated` still clears, so a session whose mirror is non-empty (a restarted shell
 * replaying its predecessor's screen) still paints.
 */
export function forgetSession(paneId: string): void {
  const host = hosts.get(paneId)
  if (host) {
    host.sessionId = undefined
    host.exitMarked = false
    host.hydrated = false
    host.busy = false
    host.lastGeometry = undefined
  }
  const entry = ledger.get(paneId)
  if (entry) entry.sessionId = undefined
}

export function liveHosts(): Iterable<PaneHost> {
  return hosts.values()
}

export function hostCount(): number {
  return hosts.size
}

export interface HostStats {
  /** Hosts currently sitting in a slot. */
  live: number
  /** Hosts resident but detached; `live + parked` is what `HOST_CAP` bounds. */
  parked: number
  /**
   * How many times `term.open()` has run, per pane, including panes whose host has since
   * been evicted or destroyed. The M4 check asserts this stays at 1 after a hundred
   * split/close/maximize/tab-switch cycles.
   */
  opens: Record<string, number>
}

export function hostStats(): HostStats {
  let live = 0
  for (const host of hosts.values()) if (host.mounted) live += 1

  const opens: Record<string, number> = {}
  for (const [paneId, entry] of ledger) if (entry.opens > 0) opens[paneId] = entry.opens

  return { live, parked: hosts.size - live, opens }
}

/**
 * How many times each pane has been evicted.
 *
 * Kept out of `hostStats` because it is not a claim about the current state but the
 * allowance the open counter is read against: a pane that has been evicted `n` times is
 * entitled to `n + 1` opens, and without this number a legitimate rehydration is
 * indistinguishable from the bug the counter exists to catch.
 */
export function evictionCounts(): Record<string, number> {
  const out: Record<string, number> = {}
  for (const [paneId, entry] of ledger) if (entry.evictions > 0) out[paneId] = entry.evictions
  return out
}

/** Registry-detected violations, counted since start-up. */
export function hostFaults(): { destroyedWhileMounted: number; openedWhileParked: number } {
  return { ...faults }
}

/** Kept as the name that predates `hostStats`; the same numbers. */
export function openCounts(): Record<string, number> {
  return hostStats().opens
}
