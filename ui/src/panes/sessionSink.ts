/**
 * A pane's session sink, owned by the pane *host* rather than by the React mount.
 *
 * The distinction is the whole module. `TerminalPane`'s effect used to attach the sink and
 * its cleanup used to detach it, which read as symmetry and was a data-loss bug: a project
 * switch unmounts every pane of the outgoing project (`App.tsx` renders only the active
 * project's tabs), so the sink died with the mount while the host — and its xterm buffer —
 * survived in parking with `hydrated` still true. Every byte the child printed while the
 * user was in the other project reached only the Rust screen mirror; on the way back the
 * fresh snapshot `session_attach` returns was discarded by the hydration gate, and the pane
 * sat frozen on the frame it was parked on, taking keystrokes, with no sign anything was
 * missing.
 *
 * So the sink now lives exactly as long as the host's claim on the session, and a parked
 * pane keeps *parsing*: xterm's write pipeline is macrotask-driven and entirely decoupled
 * from its paused renderer, so a detached-from-document terminal keeps taking writes, keeps
 * firing `term.write` completion callbacks, and therefore keeps paying credit back. What
 * stops while parked is only the painting, and `layout/paneHosts.ts`'s mount-path repaint
 * covers that.
 *
 * The lifetime ends in exactly four places, none of which is a React cleanup:
 *
 * * `releaseHost` — the pane left for another window, which attaches its own sink;
 * * `destroyHost`/eviction (`teardown`) — the terminal itself is going away;
 * * `forgetSession` — a restart; the next `ensureAttached` opens a new link;
 * * a failed attach — the link is removed so the recovery path can retry cleanly.
 *
 * All three host-side moments call `PaneHost.sinkClose`, a closure this module installs —
 * a closure rather than an import so `paneHosts.ts` never depends on this module (this
 * module imports it, and the pair would otherwise be a cycle).
 */
import {
  getHost,
  hasSavedScrollback,
  noteParsed,
  noteResized,
  setHostBusy,
  takeSavedScrollback,
} from '@/layout/paneHosts'
import {
  diag,
  events,
  paneSession,
  session as sessionApi,
  type Geometry,
} from '@/ipc/client'
import type { TerminalHandle } from '@/terminal/xterm'
import { attachSequencer, historyRequest, hydrationPlan } from './attachModel'
import { exitMarkerBytes, markFor } from './exitMarker'

/**
 * Fallback geometry for a pane that has not been laid out yet.
 *
 * `fitAddon.fit()` on a zero-sized element does not fail — it returns 2x1, and a child
 * spawned at 2x1 draws a ruined first frame and, for a fullscreen TUI, can wedge until
 * something forces a repaint. Observed directly: `TIOCGWINSZ` on a real `claude` child
 * reported `rows=1 cols=2`.
 *
 * So an implausible fit is treated as "no measurement yet": spawn at a conventional
 * terminal size and let the first real `ResizeObserver` callback correct it via SIGWINCH,
 * which every terminal program already handles.
 */
export const FALLBACK: Geometry = { cols: 80, rows: 24, cellWidth: 8, cellHeight: 17 }

export function plausible(cols: number, rows: number): boolean {
  return cols >= 20 && rows >= 5
}

/**
 * Fit the terminal to its box, and say whether that moved its cell geometry.
 *
 * `fit()` alone cannot answer the second half — it resizes or does nothing, silently — and
 * two callers downstream need the answer for different reasons: `syncSize` spends a
 * `session_resize` on it, and `noteResized` records that a frame is owed. Asking `fit()` twice
 * or comparing after the fact at each call site would be two copies of the same three lines.
 *
 * The viewport is deliberately left alone. xterm keeps `viewportY === baseY` across a resize
 * itself — `Buffer.resize` and both `_reflow*` paths decrement or increment `ydisp` in step
 * with `ybase` precisely so a terminal that was following its output still is — so a
 * `scrollToBottom` here would be a no-op with a comment claiming it prevented something.
 */
function refit(handle: TerminalHandle): boolean {
  const { term } = handle
  const cols = term.cols
  const rows = term.rows
  handle.fit.fit()
  return term.cols !== cols || term.rows !== rows
}

/**
 * The pane's size right now, as a cell geometry.
 *
 * A function rather than a value computed once, because it is asked at moments minutes
 * apart — a spawn, a restart, an attach — in a pane that may have been dragged to a
 * different size between them. Answering with a stale measurement draws the child's first
 * frame at the wrong width, which for a fullscreen TUI is a ruined screen until something
 * forces a repaint.
 */
export function measureGeometry(paneId: string): Geometry {
  const handle = getHost(paneId).terminal
  if (!handle) return FALLBACK
  try {
    refit(handle)
  } catch {
    /* the slot may not be laid out yet on the very first frame */
  }
  const { term } = handle
  const measured = plausible(term.cols, term.rows)
  const cell = measured
    ? handle.cellSize()
    : { width: FALLBACK.cellWidth, height: FALLBACK.cellHeight }
  return measured
    ? { cols: term.cols, rows: term.rows, cellWidth: cell.width, cellHeight: cell.height }
    : FALLBACK
}

/**
 * Push the pane's current pixel size down to the child as a cell geometry.
 *
 * Called from two places, and it needs both. `ResizeObserver` fires once immediately when
 * you `observe()` an element — which happens in `PaneSlot`'s layout effect, *before*
 * `TerminalPane`'s effect has created the terminal. That first callback is therefore
 * dropped, and if the pane is never resized again it is also the only one: every child
 * would sit at the fallback 80x24 forever, in a window that is plainly much larger.
 * So the attach path calls this explicitly once the session exists.
 *
 * Returns a promise that settles once the child has actually been told, so a caller that
 * pushes a size of its own afterwards — the alt-screen repaint nudge in `ensureAttached` —
 * can order itself behind this one. Two un-awaited `session_resize` calls in flight at once
 * is how a pane ends up parked at the nudge's transient `cols - 1`.
 */
export function syncSize(paneId: string): Promise<void> {
  const host = getHost(paneId)
  const handle = host.terminal
  if (!handle) return Promise.resolve()
  let refitted = false
  try {
    refitted = refit(handle)
  } catch {
    return Promise.resolve()
  }
  /*
   * A resize owes a frame exactly as parsed bytes do, and on an idle pane it is the *only*
   * debt that can be incurred — which is why maximising a quiet pane could leave it blank
   * with nothing in the system able to notice. `noteResized` asks for that frame and arms the
   * watchdog in case the ask goes unanswered.
   *
   * Only when the fit actually moved `cols`/`rows`: a `ResizeObserver` fires per frame of a
   * drag and most of those frames are the geometry the terminal already had.
   *
   * Before the plausibility and cache gates below, which are both about what to tell the
   * *child*. Whether this webview painted the reflow is a different question, and it is owed
   * for a fit this host declined to forward just as much as for one it sent.
   */
  if (refitted) noteResized(paneId)
  // Never push a degenerate size at a live child. A pane that is momentarily unlaid-out
  // fits to 2x1, and forwarding that would reflow the TUI into garbage for no reason.
  if (!plausible(handle.term.cols, handle.term.rows)) return Promise.resolve()
  if (!host.sessionId) return Promise.resolve()

  const cell = handle.cellSize()
  const geo = {
    cols: handle.term.cols,
    rows: handle.term.rows,
    cellWidth: cell.width,
    cellHeight: cell.height,
  }
  // A `ResizeObserver` fires per frame of a drag, and most of those frames are the same cell
  // geometry — a pixel change under one cell is not a resize. `session_resize` is synchronous
  // and reflows the whole scrollback under a lock (see `cmd/session.rs`), so re-sending an
  // unchanged size put one full reflow per frame per pane in front of the user's keystrokes
  // on the same thread. See `PaneHost.lastGeometry`.
  const last = host.lastGeometry
  if (
    last &&
    last.cols === geo.cols &&
    last.rows === geo.rows &&
    last.cellWidth === geo.cellWidth &&
    last.cellHeight === geo.cellHeight
  ) {
    return Promise.resolve()
  }
  host.lastGeometry = geo

  return sessionApi.resize(host.sessionId, geo).catch((e) => {
    // Cleared so the next callback retries: a dropped resize leaves the child at a size the
    // pane is not, and silently remembering the size we failed to send would make that
    // permanent.
    host.lastGeometry = undefined
    console.error('[cide] terminal resize failed', e)
  })
}

/** Panes that have already reported a failed write, so the log cannot become the outage. */
const writeFailures = new Set<string>()

/**
 * Say something when a keystroke does not reach the child.
 *
 * Goes to the Rust log as well as the console because the console is not reachable from a
 * shell on Wayland — see `cmd/diag.rs`, which is where that limitation is written down.
 */
export function reportWriteFailure(paneId: string, error: unknown): void {
  if (writeFailures.has(paneId)) return
  writeFailures.add(paneId)
  const line = `pane ${paneId}: session write failed — keystrokes are being dropped: ${String(error)}`
  console.error(`[cide] ${line}`)
  void diag.log(line).catch(() => {})
}

/** A restarted pane gets a fresh report if its new child's writes fail too. */
export function clearWriteFailure(paneId: string): void {
  writeFailures.delete(paneId)
}

/**
 * Write `— exited —` into the pane whose child has gone.
 *
 * M2's acceptance criterion is "kill the child: EOF arrives and the pane shows `— exited —`",
 * and it is the only user-visible proof that `drop(pair.slave)` works — without that drop
 * the master never sees EOF and this is never reached. Written into the terminal rather
 * than rendered as React chrome: see `exitMarker.ts`, which owns the wording and framing so
 * a check script can run them without a DOM. A parked pane takes this write like any other;
 * the marker is simply already on screen when the user comes back.
 *
 * `code` is the child's real status, omitted on the one path that cannot know it. Returns
 * whether this call is the one that wrote it: exit reaches a pane from two directions — the
 * `cide://session-state` event and the one-shot check for a session already dead before the
 * attach — and the marker must be written once however many arrive.
 */
function markExited(paneId: string, code?: number): boolean {
  const host = getHost(paneId)
  if (host.exitMarked) return false
  host.exitMarked = true
  host.terminal?.term.write(exitMarkerBytes(code))
  return true
}

/**
 * What the registry can still say about a session this pane was not there to hear die.
 *
 * Returns the argument for `recordExit`, or `null` for "not dead" — see `markFor`, which
 * owns that decision and is the part a check script can run. A rejection is `null`: a
 * question that could not be asked is not evidence the child is gone, and marking a live
 * pane `— exited —` is the worse of the two mistakes.
 */
async function exitMark(session: string): Promise<{ code?: number } | null> {
  return sessionApi.exit(session).then(markFor, (e) => {
    console.error('[cide] could not ask about session exit', e)
    return null
  })
}

/**
 * One pane's live attachment: the channel, the session it was registered against, and what
 * this module has heard about it. Keyed by pane in `links`; at most one per pane.
 */
interface Link {
  readonly paneId: string
  /**
   * Captured at attach, never read from the host afterwards. That capture is what makes
   * "the detach names the session the sink was registered against" structural: a restart
   * clears `host.sessionId` before the new spawn, and a detach that read the host's
   * *current* id at that moment would name nothing and leave the old sink registered.
   */
  readonly session: string
  ready: Promise<void>
  /**
   * Set the moment `session_attach` resolves. It decides what a *failed* attach owes the
   * Rust side: a rejection after this point leaves a registered sink behind, and a sink
   * nobody acks is a permanently choked one — the pty watchdog would spend the rest of the
   * session forgiving it with catch-up screens. A rejection before it has nothing to
   * detach, and a `session_detach` for an attachment the registry refused would only add a
   * second error to the first.
   */
  attached: boolean
  /** Set by `closeSink`; every async continuation in the attach path checks it. */
  closed: boolean
  /** The recorded exit, or `null` while the child lives. Replayed to late subscribers. */
  exit: { code?: number } | null
  onData: { dispose(): void } | null
}

const links = new Map<string, Link>()

/**
 * Exit subscribers, per pane rather than per link: the component that renders the restart
 * bar survives a restart, and its subscription must survive the link turning over with it.
 */
const exitListeners = new Map<string, Set<(code?: number) => void>>()

/**
 * One `cide://session-state` subscription for the whole module, armed lazily by the first
 * attach and never torn down.
 *
 * One rather than one per pane, deliberately: the event is a broadcast carrying the session
 * in its payload, so a single handler dispatching over at most `HOST_CAP` links replaces N
 * per-mount `listen` calls and the async-unlisten races that came with them. It is also
 * what keeps `busy` honest for a *parked* pane — eviction protection for a mid-turn host
 * used to freeze at whatever the last mounted moment said, because the per-mount
 * subscription died with the mount.
 */
let stateDispatchArmed = false

function armStateDispatch(): void {
  if (stateDispatchArmed) return
  stateDispatchArmed = true
  void events
    .onSessionState((session, state) => {
      for (const link of links.values()) {
        if (link.closed || link.session !== session) continue
        setHostBusy(link.paneId, state.state === 'busy')
        if (state.state === 'exited') recordExit(link, state.code)
      }
    })
    .catch((e) => {
      // Re-armable: the next attach tries again rather than leaving every pane deaf to
      // exits for the life of the window.
      stateDispatchArmed = false
      console.error('[cide] session sinks cannot hear about exits', e)
    })
}

function recordExit(link: Link, code?: number): void {
  if (link.closed) return
  link.exit = code === undefined ? {} : { code }
  markExited(link.paneId, code)
  // The turn is over however it ended; a dead pane must not be pinned out of eviction.
  setHostBusy(link.paneId, false)
  const subs = exitListeners.get(link.paneId)
  if (subs) for (const cb of subs) cb(code)
}

/**
 * Close a link: stop delivering, stop listening to the keyboard, and take the sink off the
 * Rust side. Also the body of the `host.sinkClose` closure `paneHosts.ts` calls.
 *
 * `detach` is false only when the attach itself failed — no sink was ever registered, and
 * `session_detach` against a session the registry refused to attach would just add a second
 * error to the first.
 */
function closeSink(link: Link, detach: boolean): void {
  if (link.closed) return
  link.closed = true
  link.onData?.dispose()
  link.onData = null
  // Guarded, because a late close — an attach failing after a replacement link was already
  // installed — must not tear the replacement's wiring off the host.
  if (links.get(link.paneId) === link) {
    links.delete(link.paneId)
    const host = getHost(link.paneId)
    host.sinkClose = undefined
    setHostBusy(link.paneId, false)
  }
  // Detach the sink, never the session: the child keeps running and this pane can be
  // re-attached later without the process noticing. By pane, so closing one mirror leaves
  // the other attached.
  if (detach) void paneSession.detach(link.paneId, link.session).catch(() => {})
}

/** The recorded exit of this pane's current attachment, or `null` while the child lives. */
export function sinkExit(paneId: string): { code?: number } | null {
  const link = links.get(paneId)
  if (!link || link.closed) return null
  return link.exit
}

/**
 * Hear about this pane's child exiting, however this module learns of it.
 *
 * Fires immediately when an exit is already recorded — that is the pane coming back from a
 * park during which the child died, and the component registering this at mount must show
 * the way back without waiting for an event that already happened.
 */
export function onSinkExit(paneId: string, cb: (code?: number) => void): () => void {
  let subs = exitListeners.get(paneId)
  if (!subs) {
    subs = new Set()
    exitListeners.set(paneId, subs)
  }
  subs.add(cb)
  const link = links.get(paneId)
  if (link && !link.closed && link.exit !== null) cb(link.exit.code)
  return () => {
    subs.delete(cb)
  }
}

/**
 * Attach this pane's sink to `session` and keep it attached until the host is released,
 * destroyed, evicted, or the pane restarts — **not** until a mount ends.
 *
 * Idempotent per (pane, session): a remount of a parked live pane finds its link and
 * resolves immediately, because the buffer already holds everything the sink delivered
 * while it was away — the whole reason the sink outlives the mount. Rejects exactly as
 * `session_attach` does, with the link already removed, so `TerminalPane`'s recovery path
 * can forget the session and retry cleanly.
 */
export function ensureAttached(paneId: string, session: string): Promise<void> {
  const existing = links.get(paneId)
  if (existing && !existing.closed) {
    if (existing.session === session) return existing.ready
    // Defensive: `forgetSession` closes the old link before a pane changes session, so an
    // open link for another id here means a caller skipped it. Close rather than throw —
    // the pane the user is looking at should attach either way.
    closeSink(existing, true)
  }

  const host = getHost(paneId)
  const link: Link = {
    paneId,
    session,
    ready: Promise.resolve(),
    attached: false,
    closed: false,
    exit: null,
    onData: null,
  }
  links.set(paneId, link)
  host.sinkClose = () => closeSink(link, true)
  armStateDispatch()

  const handle = host.terminal
  if (!handle) {
    // `TerminalPane` opens the terminal at the top of its effect, before anything can call
    // this; reaching here is a caller out of order, and attaching a sink with nowhere to
    // write would silently discard output instead.
    closeSink(link, false)
    return Promise.reject(new Error(`pane ${paneId}: ensureAttached before openTerminal`))
  }

  // Keystrokes for the link's lifetime, not the mount's. The id is read from the host at
  // each keystroke rather than captured: the read is a claim about *now*, and between a
  // restart's `forgetSession` and its respawn there is deliberately no id to write to.
  //
  // No `acknowledge` here, and that is a fix rather than an omission — see `TerminalPane`'s
  // `onKeyDown`, which is where it moved to: `onData` is xterm's outbound stream and the
  // terminal answers Device Attributes and cursor-position queries on it by itself, so the
  // CLI probing the terminal at the end of a turn was clearing the awaiting marker with
  // nobody at the keyboard.
  //
  // `.catch` rather than `void`, because a keystroke that never reached the child looks
  // exactly like a keystroke the child ignored. Once per pane — if writes are failing they
  // are failing on every character, and a per-keystroke log is its own outage.
  link.onData = handle.term.onData((data) => {
    const id = getHost(paneId).sessionId
    if (!id) return
    sessionApi.write(id, data).catch((e) => reportWriteFailure(paneId, e))
  })

  link.ready = attach(link).catch((e: unknown) => {
    // See `Link.attached`: a failure after the sink was registered must still take it off
    // the Rust side, or the sink sits there unacked for the life of the session.
    closeSink(link, link.attached)
    throw e
  })
  return link.ready
}

async function attach(link: Link): Promise<void> {
  const { paneId, session } = link
  const host = getHost(paneId)
  const handle = host.terminal
  if (!handle) throw new Error(`pane ${paneId}: terminal disappeared mid-attach`)
  const { term } = handle
  const geo = measureGeometry(paneId)

  // The ack is taken in `term.write`'s completion callback, not at delivery. Delivery only
  // means the bytes arrived; the callback fires once xterm has actually parsed them, which
  // is the rate the session should be pacing itself against. Acking on arrival would report
  // a speed this renderer cannot sustain and would turn credit control back into no control
  // at all. A parked pane keeps parsing — the write pipeline does not pause with the
  // renderer — so a parked pane keeps acking, which is what lets output flow while the
  // user is in another project.
  //
  // Taken in the callback, *reported* per task: xterm's write loop parses several queued
  // chunks per ~12ms slice and fires their completion callbacks in one JS task, and each
  // ack is an `invoke` — a full trip across the GTK main loop. At `FLUSH_INTERVAL`'s 125
  // frames/s per busy session that overhead is real, so the byte counts accumulate in the
  // callback and one microtask flushes them: identical totals, one invoke per parse slice
  // instead of one per chunk, and an ack never delayed past the task that earned it.
  // `queueMicrotask` and nothing looser, deliberately — an occluded window throttles rAF
  // and WebKit throttles hidden-window timers, a parked pane must keep acking by design,
  // and `cide-pty`'s credit watchdog reads a 5s ack silence as a wedged sink.
  let unacked = 0
  let flushQueued = false
  const flushAck = () => {
    flushQueued = false
    const bytes = unacked
    unacked = 0
    if (bytes > 0) paneSession.ack(paneId, session, bytes)
  }
  const deliver = (bytes: Uint8Array) => {
    term.write(bytes, () => {
      unacked += bytes.byteLength
      if (!flushQueued) {
        flushQueued = true
        queueMicrotask(flushAck)
      }
      // The same moment, told to the render watchdog. The bytes are in xterm's buffer
      // here; whether they ever reach the screen is a separate question, and the one
      // `terminal/renderStall.ts` exists to ask.
      noteParsed(paneId)
    })
  }

  // Live frames can arrive before the snapshot has been written — the sink is registered
  // on the Rust side the moment `session_attach` runs, while the command's reply is still
  // in flight. `attachModel.ts` owns the ordering rule; the map holds the bytes it told us
  // to hold. It stays small: after `onSnapshotWritten` every frame is `deliver`ed directly.
  const seq = attachSequencer()
  const held = new Map<number, Uint8Array>()
  let frames = 0

  // Attaching *by pane*, not by window. Two panes mirroring one session in one window used
  // to be one attachment on the Rust side, so opening a mirror detached the pane being
  // mirrored — see `paneSession` in the IPC client.
  //
  // The screen comes back from `attach` itself, taken at the instant this sink was
  // registered, so the mirror and the sink cannot disagree. See
  // `cide_pty::PtySession::attach_with_snapshot`.
  //
  // Whether to ask for the mirror's retained scrollback in front of that screen (M42) —
  // only for a host that holds no transcript of its own. `attachModel.historyRequest` is the
  // rule; the saved-buffer half is *peeked*, not taken, because the take below must still
  // find it after the round trip.
  const history = historyRequest({
    hydrated: host.hydrated,
    savedScrollback: hasSavedScrollback(paneId),
  })
  const screen = await paneSession.attach(
    paneId,
    session,
    geo,
    (data) => {
      // A closed link's frames are dropped, not queued: the Rust side has already detached
      // (or refused the attach), and a terminal that may be mid-teardown must not be written.
      if (link.closed) return
      const bytes = new Uint8Array(data)
      const at = frames++
      if (seq.onFrame(at) === 'deliver') deliver(bytes)
      else held.set(at, bytes)
    },
    history,
  )
  link.attached = true
  if (link.closed) return

  /*
   * `session_attach` resized the child before it registered this sink — unconditionally,
   * to the `geo` measured above, which is now an IPC round trip old. So the cache no
   * longer describes what the child thinks its size is, and a `ResizeObserver` callback
   * that landed in the await would otherwise have its correction silently undone with no
   * path back until the pane's pixel size changes. The cache means "what this host last
   * told the child, with no other writer since", and `session_attach` is another writer.
   */
  host.lastGeometry = undefined

  /*
   * Which of the terminal's two buffers the child is painting into. Asked *after* the
   * attach, deliberately: the answer must never be older than the snapshot, or the check
   * below decides against a screen it cannot see.
   */
  const alt = await sessionApi.inAlternateScreen(session)
  if (link.closed) return

  /*
   * What to do with the snapshot — `attachModel.ts` owns the rule, and the two failure
   * modes it separates are both on the record: writing when the terminal already holds the
   * bytes appended a second copy of the transcript on every split, and skipping when it
   * does not showed a frozen stale screen after every project switch.
   *
   * The alt-drift repair stays even though, after the sink moved to the host, every path
   * that genuinely attaches also arrives with `hydrated` false (fresh, released, evicted
   * and restarted hosts all clear it — a parked pane never detaches, so it can no longer
   * miss a buffer switch). That invariant is emergent rather than enforced, and this branch
   * is the safety net that made the last buffer-drift bug shippable-without-recurrence:
   * stuck on the alternate buffer a pane loses its scrollbar and the wheel edits the
   * child's prompt; stuck on the normal one, cursor-addressed repaints land in rows that
   * have scrolled away. It costs one round trip per genuine attach.
   */
  // Read-and-clear, before the plan: eviction parked the terminal's serialized buffer in
  // the ledger so the scrollback the one-screen mirror cannot carry survives the round
  // trip. Taking it here (rather than inside the `if` below) is what makes "exactly once"
  // structural — a second attach finds nothing whatever the flags say.
  const saved = takeSavedScrollback(paneId)
  const plan = hydrationPlan({
    hydrated: host.hydrated,
    needsReset: host.needsReset,
    termOnAlt: term.buffer.active.type === 'alternate',
    mirrorOnAlt: alt,
    snapshotEmpty: screen.byteLength === 0,
    savedScrollback: saved !== null,
  })
  if (plan.dropHydratedForAltDrift) {
    host.hydrated = false
    // Said out loud, because this repair is otherwise invisible and the bug it repairs
    // was reported as "rendering is broken". Silence over a long session is the honest
    // evidence that the snapshot is carrying the buffer it belongs to.
    void diag
      .log(
        `pane ${paneId}: terminal was on the ${alt ? 'primary' : 'alternate'} buffer while session ${session} is on the ${alt ? 'alternate' : 'primary'} one; repainting from the mirror`,
      )
      .catch(() => {})
  }
  if (plan.hydrate) {
    // A host that was released and is coming back still holds its pre-detach screen. The
    // mirror replaces that rather than following it.
    if (plan.reset) term.reset()
    if (host.needsReset) host.needsReset = false
    // Before the snapshot, which opens with a clear-screen and paints the *current* rows
    // over the replayed viewport — see `HydrationPlan.replayScrollback` for the ordering
    // argument. The scrollback above the fold is what survives.
    if (plan.replayScrollback && saved !== null) term.write(saved)
    if (plan.writeSnapshot) term.write(new Uint8Array(screen))
    host.hydrated = true
  }

  for (const at of seq.onSnapshotWritten()) {
    const bytes = held.get(at)
    if (bytes) deliver(bytes)
  }
  held.clear()

  /*
   * Now that the sink is live, adopt the pane's real size — and only then nudge a
   * fullscreen TUI into repainting.
   *
   * The order is the fix. The nudge used to run *before* `syncSize` and to use `geo` — the
   * geometry measured before this function's awaits — so it re-imposed a size the pane may
   * no longer have, on top of the same stale value `session_attach` had already written.
   * Nothing then told `syncSize` the child had been resized behind its back, so it sent
   * nothing, and the child spent the rest of its life drawing frames for rows the viewport
   * had not got — which is why maximising the pane "repaired the rendering".
   *
   * Skipped for a pane that is parked (or was closed) by the time the frame comes around:
   * a parked pane cannot be measured, and the `syncSize` its next mount runs — every mount
   * path measures — is the same correction with a real box under it.
   */
  requestAnimationFrame(() => {
    void (async () => {
      await syncSize(paneId)
      if (link.closed || !getHost(paneId).mounted || !alt) return
      // A fullscreen TUI's own model is authoritative for everything the screen mirror
      // does not track — OSC 8 hyperlinks, OSC 52 clipboard traffic, DEC 2026 sync
      // framing. Nudging the size by one column makes it repaint from that model.
      //
      // Read after the await, so it is the size `syncSize` has just settled on rather
      // than the one measured before this function's round trips. `geo` is the fallback
      // for the case where `syncSize` had nothing plausible to measure and so recorded
      // nothing.
      const before = getHost(paneId).lastGeometry
      const at = before ?? geo
      await sessionApi.resize(session, { ...at, cols: Math.max(1, at.cols - 1) })
      await sessionApi.resize(session, at)

      /*
       * The nudge is a writer like every other, so it owes the cache the same honesty
       * `session_attach` does — and the two awaits above are a window a `ResizeObserver`
       * callback can land in. When one does, `syncSize` records the pane's *new* size and
       * the line above quietly puts the child back to `at`; the cache then agrees with a
       * size the child has not got, and nothing corrects it until the pixel size changes
       * again. Identity and not equality, because `syncSize` stores a fresh object on
       * every write: a different object means somebody else wrote, whatever the numbers
       * say. Costs nothing in the common case and one resize in the case that would
       * otherwise have been permanently wrong.
       */
      if (getHost(paneId).lastGeometry !== before) {
        getHost(paneId).lastGeometry = undefined
        await syncSize(paneId)
      }
    })().catch(() => {
      // A failed nudge costs a repaint, not correctness: the pane already holds the
      // snapshot, and the next real resize nudges it again.
    })
  })

  // A session that was already dead when this sink attached will never produce an event —
  // the watcher fired before anyone was listening. One check, not a poll: this is the
  // rehydration case (a host evicted and re-created after its child had gone), answered
  // once at attach time. The answer carries the code, so a dead-at-attach pane prints the
  // same sentence the event would have.
  const mark = await exitMark(session)
  if (mark !== null && !link.closed) recordExit(link, mark.code)
}
