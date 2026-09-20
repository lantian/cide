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
import { SerializeAddon } from '@xterm/addon-serialize'
import { attachInputProbe, attachInputRouting } from '@/terminal/inputHost'
import { attachPathLinks } from '@/terminal/pathLinks'
import { attachRunLinks } from '@/terminal/runLinkProvider'
import { attachTaskLinks } from '@/terminal/taskLinkProvider'
import type { TerminalPaneKind } from '@/terminal/keys'
import {
  STALL_MS,
  isRenderStalled,
  shouldRepairRender,
  type RenderStallInput,
} from '@/terminal/renderStall'
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
  /**
   * True when this pane adopted a session it does not own — a `SplitIntent::Mirror`, and
   * (once M18 lands) every subagent run opened from the Agents panel.
   *
   * It lives **on the host** because the host is what survives a remount, and because the
   * spawn plan is consumed exactly once — by the time anything asks, the pane no longer
   * knows how it got its id.
   *
   * It exists for exactly one reader, `closePane`, which kills `host.sessionId` on the way
   * out. Without it, closing a mirror pane kills the child in the pane being *mirrored*,
   * which is what shipped.
   */
  mirrored?: boolean | undefined
  /**
   * Close this pane's session sink: stop delivering output and take the attachment off the
   * Rust side.
   *
   * Installed by `panes/sessionSink.ts`, which owns the sink for exactly as long as the
   * host holds its claim on the session — **not** for as long as a React mount, which is
   * the distinction this field exists for. A project switch unmounts the pane but only
   * *parks* the host, and a parked pane keeps its sink so the buffer keeps filling; the
   * sink ends at the three moments this module knows about first, which are the three
   * callers: `releaseHost` (another window shows the pane now), `teardown` (the terminal
   * itself is going away), and `forgetSession` (a restart — the closure captured the old
   * session id, so the detach names the session the sink was registered against by
   * construction). A closure rather than an import, so this module never depends on the
   * controller that already depends on it.
   */
  sinkClose?: (() => void) | undefined
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
  /**
   * `performance.now()` when bytes were last *parsed* into this terminal, and when it last
   * painted a frame.
   *
   * The pair is the whole of the render watchdog's evidence — see `terminal/renderStall.ts`
   * for the failure it exists to notice. Parsed rather than delivered, for the same reason
   * the credit ack is taken from `term.write`'s completion callback: arrival says only that
   * the bytes reached the webview, and the claim being made here is that xterm has them in
   * its buffer and still has not drawn.
   */
  lastParsedAt?: number | undefined
  lastRenderedAt?: number | undefined
  /**
   * `performance.now()` when this terminal was last fitted to a *different* cell geometry.
   *
   * The second debt, beside `lastParsedAt`. A resize owes a frame exactly as parsed bytes do —
   * every row the user is looking at has moved — and it is the only one an idle pane can
   * incur, which is why maximising a quiet pane could leave it blank with nothing in the
   * system able to notice. Stamped from `syncSize`'s changed-geometry path only, so a drag
   * that reports the same cell size a hundred times arms nothing.
   */
  lastResizedAt?: number | undefined
  /** When this host was last unstuck, so a repair that did not work cannot become a loop. */
  lastRepairAt?: number | undefined
  /** The armed stall check, so at most one is outstanding per host. */
  stallTimer?: ReturnType<typeof setTimeout> | undefined
}

/**
 * How many hosts may be resident at once, mounted or parked — the **floor** of the budget.
 *
 * Each one holds an xterm instance and may hold a WebGL context, and WebKitGTK caps
 * concurrent contexts at roughly 8-16 — but the GL contexts are pooled separately
 * (`promoteWebgl`/`releaseWebgl`; a parked host holds none), so what this bounds is xterm
 * instances and their buffers. Twelve is above any plausible single-project working set —
 * the mock's grid is four — and low enough that a session spent splitting does not
 * accumulate renderers until the pane that matters stops painting.
 */
export const HOST_CAP = 12

/**
 * The working cap: the floor above, or the workspace's real terminal-pane population plus
 * slack, whichever is larger — ceilinged at 32.
 *
 * A flat `HOST_CAP` was tuned for leaked hosts (panes closed and forgotten) and it punished
 * the multi-project case instead: with two 8-pane projects open, every project switch parks
 * eight hosts, the sweep evicts the overflow — always from the project just left, since
 * eviction skips `mounted` — and switching back pays a full re-attach cycle per evicted
 * pane (a fresh `term.open()`, `session_attach`, snapshot rehydrate, nudge resizes) *and*
 * loses the scrollback, because the vt100 mirror a re-created host rehydrates from is one
 * screen. Panes that still exist in the workspace are panes the user asked for; the budget
 * follows them, and the cap's real job — bounding hosts whose panes are gone — is unchanged
 * because those never count toward `livePaneIds`. The ceiling keeps a pathological
 * workspace bounded: at ~5000 scrollback lines an xterm buffer is single-digit MiB, so 32
 * is tens of MiB, not hundreds.
 */
let hostBudget = HOST_CAP

/**
 * Every terminal pane id the workspace currently holds, across all open projects and
 * detached windows — fed from the mirror on every snapshot (`store/workspace.ts`). Read
 * twice: to size the budget above, and to prefer evicting hosts whose pane no longer
 * exists anywhere (closed in another window — before this they parked for ever).
 */
let livePaneIds: ReadonlySet<string> = new Set()

export function noteLivePanes(ids: ReadonlySet<string>): void {
  livePaneIds = ids
  hostBudget = Math.min(32, Math.max(HOST_CAP, ids.size + 2))
  scheduleEviction()
}

/** The working cap right now, for the pane audit's residency assertion. */
export function hostBudgetNow(): number {
  return hostBudget
}

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
  /**
   * Carried beside `sessionId` and for the same reason.
   *
   * `getHost` hands an evicted pane its session back, so without this an evicted mirror pane
   * would come back holding somebody else's id and *not* knowing it — and `closePane` would
   * kill the mirrored child after all. The flag has to survive exactly as far as the id it
   * qualifies, or the fix has a hole in it the size of `HOST_CAP`.
   */
  mirrored?: boolean | undefined
  /**
   * The terminal's serialized buffer, taken at eviction, spent on the next attach.
   *
   * The vt100 mirror an evicted pane rehydrates from is **one screen**, so eviction used to
   * cost the pane its whole scrollback with nothing anywhere saying so. Serialized on the
   * way out (`evictBeyondCap`), replayed exactly once before the mirror snapshot on the
   * fresh host's first hydration (`attachModel.ts::hydrationPlan` owns the rule), and
   * cleared with `sessionId` when the pane is genuinely finished. Not taken for a terminal
   * sitting on the alternate buffer: the alt screen has no scrollback to save, and a
   * serialize mid-TUI would replay half-drawn frame state under the snapshot.
   */
  scrollback?: string | undefined
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
  /*
   * A gutter between the frame and the first column, on the left only. The focus ring is
   * drawn *inside* the frame (`.frameFocused::after`), overlaying the body's first pixels,
   * so a terminal flush with the body painted its text against — under, for those pixels —
   * the accent line.
   *
   * The `calc` is not decoration, and a flat `3px` shipped first and read as no change at
   * all. How much of the body the ring overlays depends on where the pane sits: the ring is
   * `2px - <frame border width>` on each edge (`PaneTitleBar.module.css`), and on a
   * chrome-flush leaf the frame's left border is 0 (`--pane-edge-left`), so the ring is 2px
   * of overlay there against 1px on an interior pane. A fixed inset therefore showed 2px of
   * ground on an interior pane and a single pixel on an edge one — and the pane a user
   * focuses most, a claude pane at the row's left, is an edge one. Reading the same custom
   * property the frame and the ring read makes the *visible* gap 2px everywhere; custom
   * properties inherit, so an inline `var()` resolves against the leaf's declaration, and a
   * detached window (no leaf) rides the same 1px fallback as the frame.
   *
   * Here and not as padding on `.body`: an `inset: 0` absolute child resolves against the
   * padding box, so padding there would sit *under* this element and move nothing. Here it
   * narrows the very element `fit()` measures, so the column count stays honest. Hosts are
   * terminal-only (only `TerminalPane` renders a `PaneSlot`), so no editor is touched —
   * CodeMirror carries its own gutter.
   */
  el.style.left = 'calc(4px - var(--pane-edge-left, 1px))'
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
  if (prior?.mirrored !== undefined) host.mirrored = prior.mirrored

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
   * The other half of the render watchdog's evidence.
   *
   * `onRender` fires from `RenderService._renderRows`, which is the one function a paused
   * renderer never reaches — so "no `onRender` since the last bytes" is exactly the state
   * `terminal/renderStall.ts` describes, observed rather than inferred.
   */
  const rendered = handle.term.onRender(() => {
    host.lastRenderedAt = now()
  })
  host.cleanup.push(() => rendered.dispose())

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

  /*
   * An agent run's tool lines, after the path links on purpose: xterm keeps the earlier
   * provider's link wherever two overlap, and a tool line's title is routinely a path the
   * user wants to open — so the path provider wins the title and this one takes only the
   * glyph and the handle token at the end. (M42)
   */
  host.cleanup.push(
    attachRunLinks(handle, {
      session: () => getHost(paneId).sessionId ?? null,
    }),
  )

  /*
   * Task codes, third and last — and the order is the feature, not the tidying. (M60)
   *
   * xterm keeps the *earlier* provider's link wherever two overlap, so a project that really
   * contains a file named `t-503` keeps its path link, and a code inside a tool line's title
   * survives because the run provider above claims only the glyph and the `#7`. Nothing but
   * `check:task-links` pins this order, and the symptom of getting it wrong is a link that
   * works everywhere except the one line somebody complains about.
   *
   * What the pane's *project* is arrives separately from React through `setTaskLinkEnv`, for
   * the reason written above the path links: that is a prop, and props change without the host
   * changing.
   */
  host.cleanup.push(attachTaskLinks(handle, { paneId }))

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
  const moved = host.el.parentElement !== slot
  if (moved) slot.appendChild(host.el)

  /*
   * Repaint a host that was out of the document, or it comes back showing the frame it was
   * parked on.
   *
   * `parking` is never appended to `document.body` (see its declaration), so a parked host is
   * *detached* — and this module's own header states what that means to xterm: a
   * non-intersecting element pauses `RenderService`. The buffer keeps taking writes while
   * paused — xterm's write pipeline is macrotask-driven and does not pause with the renderer
   * — and since the sink moved onto the host (`PaneHost.sinkClose`, `panes/sessionSink.ts`)
   * a parked pane keeps *receiving* too: parking is reached by a split, a **project switch**
   * and a re-dock — not by a tab switch, which only flips `visibility` — and a project
   * switch used to detach the sink with the mount, so the buffer this refresh repainted was
   * itself stale by the whole away period and the snapshot that could have repaired it was
   * refused by the hydration gate. The terminal's state is correct throughout now; what
   * stops while parked is only the painting, and this is where it resumes.
   *
   * In a `requestAnimationFrame` so the element has been laid out and the observer has had a
   * chance to report it intersecting again. Safe in either order: `refreshRows` while still
   * paused sets xterm's own `_needsFullRefresh`, which the intersection callback then spends,
   * so this arms the repaint rather than racing it. Guarded on `moved` so an ordinary re-render
   * — `PaneSlot`'s effect runs whenever `paneId` changes identity — does not queue a full
   * repaint of every pane on screen for nothing.
   */
  if (moved) {
    requestAnimationFrame(() => {
      const term = hosts.get(paneId)?.terminal?.term
      if (term !== undefined) term.refresh(0, term.rows - 1)
    })
    /*
     * And put the watchdog on it. `armStallCheck` is only otherwise reachable from
     * `noteParsed`, so a pane that comes back owing a frame — bytes were parsed into it
     * while it was parked — with no further output on the way is never examined: the
     * refresh above only *arms* xterm's own repaint, and if the intersection callback does
     * not fire on re-attachment nothing ever spends it. One armed check costs nothing when
     * the frame gets paid, and is the only examiner when it does not.
     */
    if (host.opened && owesFrame(host)) armStallCheck(host)
  }

  // `releaseHost` handed this pane's WebGL context back while it was out of the tree, and
  // `openTerminal` grants one only on the first open ever — so without this a re-docked pane
  // stays on the DOM renderer for the rest of the session while panes nobody is looking at
  // keep theirs. After the append, not before: the addon builds its context against a
  // rendered element, and a line ago this one was detached in parking.
  if (redocked && host.terminal) promoteWebgl(host.terminal)
}

/* ----------------------------------------------------------------------------------------
 * The render watchdog.
 *
 * `terminal/renderStall.ts` carries the failure this exists for and the reasoning behind the
 * rule; this half is the wiring, which is the part that needs a DOM. In one sentence: xterm
 * pauses its own renderer when `.xterm-screen` reports non-intersecting and un-pauses it only
 * when that same observer reports intersecting again, so a pause caused by something that was
 * never a DOM change — a minimised window, another virtual desktop, an occluded surface — can
 * outlive the condition and freeze the picture over a buffer that is still correct.
 *
 * `mountHost` already repairs the one path with a DOM change on both sides (park, mount). This
 * is the generalisation: notice that a frame is owed and has not come, and put the element
 * through a real layout change, which is precisely what maximising the pane does and the only
 * gesture that has ever been reported to fix it.
 * -------------------------------------------------------------------------------------- */

/**
 * Report that bytes have been parsed into this pane's terminal.
 *
 * Called from `TerminalPane`'s `term.write` completion callback, beside the credit ack, and
 * from nowhere else: that callback is the moment xterm has the bytes in its buffer, which is
 * the only moment at which "and it has not drawn them" is a claim about the renderer rather
 * than about the transport.
 */
export function noteParsed(paneId: string): void {
  const host = hosts.get(paneId)
  if (!host) return
  host.lastParsedAt = now()
  armStallCheck(host)
}

/**
 * Report that this pane's terminal has been fitted to a new cell geometry.
 *
 * Called from `panes/sessionSink.ts`'s `syncSize`, on the path where the fit actually changed
 * `cols`/`rows` — the same path that decides to spend a `session_resize`, and for the same
 * reason: a `ResizeObserver` callback that reports the geometry the terminal already had is
 * not a resize, and stamping one would arm a timer per frame of a drag for a terminal that
 * owes nothing.
 *
 * Separate from [`noteParsed`] rather than folded into it because the two debts are guarded
 * differently — a scrolled-back viewport excuses unpainted bytes and does not excuse an
 * unpainted resize. `terminal/renderStall.ts` carries that argument.
 */
export function noteResized(paneId: string): void {
  const host = hosts.get(paneId)
  if (!host) return
  host.lastResizedAt = now()
  /*
   * Ask for the frame, then arm the watchdog in case the ask goes unanswered — the same pair
   * `mountHost` uses on the way back in from parking, and for the same reason. A reflow
   * *should* produce a frame on its own: `RenderService.resize` calls `_fullRefresh`, and if
   * the renderer is paused that sets xterm's `_needsFullRefresh` for the intersection callback
   * to spend. Both halves of that sentence have a way of not happening — the callback may not
   * come, and a pane maximised while idle has no later output to force the issue — and the
   * cost of asking anyway is one full repaint of one terminal per actual size change, which
   * is what a resize is.
   *
   * In a `requestAnimationFrame` because the caller is inside a `ResizeObserver` callback,
   * which runs after layout and before paint: a refresh queued from there belongs to the
   * frame that has not been painted yet, and the terminal has just been told its new size on
   * the line above. Safe in either order, exactly as at `mountHost`.
   */
  requestAnimationFrame(() => {
    const term = hosts.get(paneId)?.terminal?.term
    if (term !== undefined) term.refresh(0, term.rows - 1)
  })
  armStallCheck(host)
}

/**
 * Arm one stall check for this host.
 *
 * A timer per burst of output rather than a polling interval: output is bursty, and a session
 * spent reading a file should not carry a heartbeat that walks every host twice a second for
 * the life of the window. At most one is outstanding — a second frame of the same burst finds
 * the timer already armed and rides on it.
 */
function armStallCheck(host: PaneHost): void {
  if (host.stallTimer !== undefined) return
  host.stallTimer = setTimeout(() => {
    host.stallTimer = undefined
    checkStall(host)
  }, STALL_MS)
}

/**
 * Whether this host's element has a real box in a document the compositor is drawing.
 *
 * `getBoundingClientRect` forces a synchronous layout, which is why it is asked here and not
 * in the rule: this runs at most once per `STALL_MS` per pane, and only for a pane that has
 * already gone that long owing a frame. The healthy path never reaches it.
 */
function onScreen(host: PaneHost): boolean {
  if (document.visibilityState === 'hidden') return false
  if (!host.el.isConnected) return false
  const box = host.el.getBoundingClientRect()
  return box.width > 0 && box.height > 0
}

function stallInput(host: PaneHost, atBottom: boolean): RenderStallInput {
  return {
    now: now(),
    lastParsedAt: host.lastParsedAt ?? null,
    lastRenderedAt: host.lastRenderedAt ?? null,
    lastRepairAt: host.lastRepairAt ?? null,
    mounted: host.mounted,
    onScreen: onScreen(host),
    atBottom,
    lastResizedAt: host.lastResizedAt ?? null,
  }
}

/**
 * The unanswered events this terminal is carrying, or `undefined` if it owes nothing.
 *
 * The scheduling half of the rule in `terminal/renderStall.ts`, which is deliberately not
 * exported from there: this one is about whether to keep a timer alive, and it therefore
 * ignores the guards — a pane that is parked, off screen or scrolled back still *owes* the
 * frame, it just must not be nudged for it.
 *
 * Both ends, because the two re-arm branches below ask different questions of the same debt.
 * *How long has a frame been owed* is asked of `first`, so a trickle of output cannot keep
 * pushing the deadline out in front of a renderer that died a minute ago. *Has the last
 * repair answered this* is asked of `last`, so bytes that arrived after a repair are a fresh
 * debt and not one that has already had its nudge.
 */
function frameDebt(host: PaneHost): { first: number; last: number } | undefined {
  const painted = host.lastRenderedAt
  const owed = [host.lastParsedAt, host.lastResizedAt].filter(
    (at): at is number => at !== undefined && (painted === undefined || painted < at),
  )
  return owed.length === 0 ? undefined : { first: Math.min(...owed), last: Math.max(...owed) }
}

/** Bytes were parsed or the terminal was resized, and no frame has answered yet. */
function owesFrame(host: PaneHost): boolean {
  return frameDebt(host) !== undefined
}

function checkStall(host: PaneHost): void {
  const term = host.terminal?.term
  if (!term || !host.opened) return
  const buffer = term.buffer.active
  const input = stallInput(host, buffer.viewportY === buffer.baseY)
  const debt = frameDebt(host)
  if (!isRenderStalled(input)) {
    /*
     * Not stalled — but "not stalled" and "healthy" are different claims, and returning
     * without re-arming used to conflate them. Bytes that arrived after this timer was
     * armed pushed the stall window past it: armed at t=0 for t=STALL_MS, more bytes at
     * t=STALL_MS-100 make this check compute a recency under the threshold and return —
     * and if that burst was the last, no `noteParsed` ever arms another timer, so a stall
     * that began there was never noticed for the rest of the session. Re-armed only when
     * recency is the plausible blocker, so a parked, off-screen or scrolled-back pane does
     * not buy a per-STALL_MS heartbeat (and `stallInput`'s forced layout) for as long as
     * it sits in that state.
     */
    if (host.mounted && debt !== undefined && input.now - debt.first < STALL_MS) {
      armStallCheck(host)
    }
    return
  }
  if (!shouldRepairRender(input)) {
    // Stalled but inside the repair cooldown. Re-armed only while no repair has answered
    // these bytes, so a genuinely dead renderer gets its one deferred repair per burst and
    // then goes quiet, instead of nudging and logging every cooldown for ever.
    if (
      host.mounted &&
      debt !== undefined &&
      (host.lastRepairAt === undefined || host.lastRepairAt < debt.last)
    ) {
      armStallCheck(host)
    }
    return
  }
  host.lastRepairAt = input.now
  repaintHost(host)
  // Said out loud, because the repair is otherwise invisible and the bug it repairs was
  // reported as "it just stops outputting". A line here means a pane genuinely sat on a stale
  // frame; silence over a long session is the evidence that the renderer is keeping up on its
  // own, which is what this is supposed to become.
  //
  // Which debt, and where the viewport was sitting, because the two failures this answers are
  // told apart by exactly those two facts: a paused renderer owes bytes, and a maximise that
  // did not repaint owes a resize. A report of either should be diagnosable from the log
  // alone rather than from a second round of questions.
  const unanswered = (at: number | undefined): boolean =>
    at !== undefined && (host.lastRenderedAt === undefined || host.lastRenderedAt < at)
  const owes = [
    unanswered(host.lastParsedAt) ? 'bytes' : '',
    unanswered(host.lastResizedAt) ? 'a resize' : '',
  ].filter(Boolean)
  void diag
    .log(
      `pane ${host.paneId}: terminal owes a frame for ${owes.join(' and ') || 'an event'} and painted none for ${STALL_MS}ms while on screen (${term.cols}x${term.rows}, viewport ${buffer.viewportY}/${buffer.baseY}); forcing the renderer back`,
    )
    .catch(() => {})
}

/**
 * Put a host through a layout change and ask for a full repaint.
 *
 * Both halves, and neither is sufficient alone. `term.refresh` while xterm is still paused
 * does not paint — it records `_needsFullRefresh`, which the intersection callback spends when
 * it next fires — so it *arms* the repaint. The inset nudge is what makes that callback fire:
 * the host is `position: absolute; inset: 0` with `overflow: hidden`, so its box is the clip
 * rect every descendant's intersection is computed against, and moving one edge of it by a
 * pixel is a genuine layout change that forces WebKit to recompute. One pixel at the bottom
 * edge for one frame is not visible; a `display: none` frame would be a stronger signal and is
 * deliberately not used, because it blurs a focused textarea and rule 1 of this module's
 * header exists to keep panes out of that state.
 *
 * Restored to `0px` rather than to `''`. `inset: 0` is a shorthand that sets four longhands,
 * so clearing `bottom` would leave it `auto` — and an absolutely positioned box with `top: 0`
 * and `bottom: auto` collapses to its content height, which for a host whose only child is an
 * absolutely sized terminal is zero. That would turn a repair into the very failure it is
 * repairing.
 */
function repaintHost(host: PaneHost): void {
  const term = host.terminal?.term
  if (term) term.refresh(0, term.rows - 1)

  const el = host.el
  el.style.bottom = '1px'
  // Two frames, not one. A `requestAnimationFrame` callback runs *before* the rendering
  // update it belongs to, so restoring in the first one would undo the change before any
  // intersection was ever computed against it: the observer would see the box it already
  // believed in and stay silent. The nudge has to survive one whole update to be noticed.
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      el.style.bottom = '0px'
      const back = hosts.get(host.paneId)?.terminal?.term
      if (back) back.refresh(0, back.rows - 1)
    })
  })
}

/**
 * Repaint every mounted terminal when this document becomes visible again.
 *
 * The watchdog above can only fire while bytes are still arriving, and the commonest way to
 * meet this bug is to start something long, go elsewhere, and come back to a run that finished
 * while the window was hidden. There are no more bytes to notice by then — the pane is simply
 * showing the frame it was on when the compositor stopped drawing it — so returning to the
 * window has to be a repair in its own right.
 *
 * Registered once per module, not per pane: the event is on the document, and one listener
 * that walks the mounted hosts is cheaper than one listener per host that all fire together.
 * Cheap enough to run unconditionally — a repaint of the panes actually on screen is bounded
 * by `HOST_CAP` and by how many of those are mounted, and it costs one frame of one clipped
 * pixel each.
 */
if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState !== 'visible') return
    for (const host of hosts.values()) {
      if (!host.mounted || !host.opened) continue
      host.lastRepairAt = now()
      repaintHost(host)
    }
  })
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

/**
 * Return a host to parking. Never destroys it.
 *
 * Deliberately does **not** touch `hydrated`, `needsReset` or `sinkClose`: a parked pane
 * keeps its sink (`panes/sessionSink.ts`), so its buffer stays current for the whole away
 * period and the next mount has nothing to re-read. The previous arrangement — the sink
 * detached with the React mount while `hydrated` stayed true — is exactly the frozen-pane
 * bug: output printed during a project switch reached only the Rust mirror, and the
 * snapshot that could have repaired the buffer on the way back was refused by the
 * hydration gate. If parking ever detaches the sink again, it must take the two lines
 * `releaseHost` has, for the reasons written there.
 */
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
  // printing where this terminal cannot see. The sink goes first — it used to go in
  // `TerminalPane`'s effect cleanup, but the sink belongs to the host now and survives an
  // ordinary unmount on purpose, so the one unmount that really is a goodbye has to say so
  // here. Clearing the flag is what makes a re-docked pane read the screen mirror again —
  // without it the terminal comes back holding only what it saw before the detach, and
  // every byte from the detached period is gone with no sign that anything is missing.
  host.sinkClose?.()
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
  if (entry) {
    entry.sessionId = undefined
    entry.mirrored = undefined
    // A finished pane's saved buffer goes with its session: nothing can ever attach to
    // replay it, and a megabyte of serialized transcript per closed pane would be the
    // ledger's "not a size worth managing" claim quietly becoming false.
    entry.scrollback = undefined
  }

  const host = hosts.get(paneId)
  if (!host) return
  teardown(host)
}

function teardown(host: PaneHost): void {
  // The sink first, before anything touches the terminal it writes into: a frame delivered
  // between `terminal.dispose()` and the Rust-side detach would be written into a disposed
  // terminal. This is also what detaches an *evicted* pane — eviction reaches here through
  // `evictBeyondCap` — so the fresh host its next mount builds attaches a fresh sink and
  // rehydrates from the mirror, exactly as before the sink moved onto the host.
  host.sinkClose?.()
  if (host.mounted) {
    // The one thing this module exists to prevent: the element is on screen, so removing
    // it destroys a visible terminal mid-frame. Whoever called this should have released
    // the pane, or waited for React to unmount its slot.
    faults.destroyedWhileMounted += 1
    console.error(`[cide] pane ${host.paneId}: host destroyed while mounted`)
  }
  // Before the disposers, because one of them takes `onRender` down and a check that fires
  // afterwards would ask a disposed terminal for its buffer.
  if (host.stallTimer !== undefined) {
    clearTimeout(host.stallTimer)
    host.stallTimer = undefined
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
 * The cost is that `hostCount()` may sit one or two above the budget until the stack
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
  while (hosts.size > hostBudget) {
    const victim = evictionCandidate()
    if (!victim) return
    const entry = record(victim.paneId)
    entry.evictions += 1
    if (victim.sessionId !== undefined) entry.sessionId = victim.sessionId
    if (victim.mirrored !== undefined) entry.mirrored = victim.mirrored
    entry.scrollback = serializeForEviction(victim)
    teardown(victim)
  }
}

/**
 * The victim's buffer as a replayable byte string, or `undefined` when there is nothing
 * worth saving — see `PaneRecord.scrollback` for what this exists to stop losing.
 *
 * The addon is loaded for the one call and disposed: serialization is an eviction-time
 * event, not a per-frame concern, and a permanently loaded addon per host would be twelve
 * observers doing nothing. A serialize that throws (a disposed-mid-teardown terminal, an
 * addon/xterm version skew) degrades to exactly the old behaviour — the pane comes back
 * with the mirror's one screen — which is why the catch is empty on purpose.
 */
function serializeForEviction(victim: PaneHost): string | undefined {
  const term = victim.terminal?.term
  if (!term || term.buffer.active.type === 'alternate') return undefined
  try {
    const addon = new SerializeAddon()
    term.loadAddon(addon)
    const bytes = addon.serialize()
    addon.dispose()
    return bytes.length > 0 ? bytes : undefined
  } catch {
    return undefined
  }
}

/**
 * The serialized buffer eviction parked for this pane, read once and cleared.
 *
 * Read-and-clear so the drift repair — which re-runs the hydration phase on a terminal
 * that already holds a transcript — can never replay it a second time, whatever the
 * caller's flags say.
 */
/**
 * Whether eviction parked a serialized buffer for this pane — asked without taking it, by the
 * attach path before it decides what to request from Rust (M42). `takeSavedScrollback` stays
 * read-and-clear; this is the one read that must not clear, because the take happens after the
 * attach's round trip and a cleared entry there would mean the replay never happens.
 */
export function hasSavedScrollback(paneId: string): boolean {
  return ledger.get(paneId)?.scrollback !== undefined
}

export function takeSavedScrollback(paneId: string): string | null {
  const entry = ledger.get(paneId)
  if (!entry || entry.scrollback === undefined) return null
  const saved = entry.scrollback
  entry.scrollback = undefined
  return saved
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
    // Then a parked host whose pane no longer exists in any project's tree — closed in
    // another window, so nothing can ever mount it again — before one the user can switch
    // back to. Eviction is the only exit such a host has.
    const hostGone = !livePaneIds.has(host.paneId)
    const bestGone = !livePaneIds.has(best.paneId)
    if (hostGone !== bestGone) {
      if (hostGone) best = host
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
    // The sink goes with the session it was registered against. The closure captured that
    // id at attach, so the detach names the right session *by construction* — the explicit
    // `paneSession.detach(paneId, old)` this replaces had to be ordered before the id was
    // cleared, and a detach that read the host's current id after this line would name
    // nothing and leave the old sink registered for ever.
    host.sinkClose?.()
    host.sessionId = undefined
    // A restarted pane owns its new child: whatever it adopted is gone, and the id it is
    // about to hold is one this pane spawned. Leaving the flag set would make `closePane`
    // spare a child nobody else is watching.
    host.mirrored = undefined
    host.exitMarked = false
    host.hydrated = false
    host.busy = false
    host.lastGeometry = undefined
  }
  const entry = ledger.get(paneId)
  if (entry) {
    entry.sessionId = undefined
    entry.mirrored = undefined
  }
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
  /** Hosts resident but detached; `live + parked` is what the host budget bounds. */
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
