/**
 * A pane showing a live PTY: a Claude Code session, or a plain shell.
 *
 * The React component owns none of the terminal. It owns the *wiring* — spawn-or-reattach,
 * keystrokes out, resize both ways — and lets `paneHosts` own the DOM.
 *
 * Spawning happens on this side rather than in Rust because only this side knows how big
 * the pane is. A child born before its slot has been laid out gets the fallback geometry
 * below and, for a fullscreen TUI, draws a ruined first frame.
 */
import { useEffect, useRef } from 'react'
import { PaneSlot } from '@/layout/PaneSlot'
import { getHost, openTerminal } from '@/layout/paneHosts'
import { session as sessionApi, type Geometry, type Pane } from '@/ipc/client'

export interface TerminalSpec {
  program: string
  args: string[]
  cwd: string
  /** Decides which IDE server this child is told about. See `session.spawn`. */
  project?: string | undefined
  /** Continue an existing conversation; with `fork`, branch from it. */
  resume?: string | undefined
  fork?: boolean | undefined
}

export interface TerminalPaneProps {
  pane: Pane
  /** Where a fresh child is spawned — the project's primary root. */
  cwd: string
  /** The project this pane belongs to, so its child reaches the right IDE server. */
  project?: string | undefined
  /** Called once, when a spawn succeeds, so the domain can record the binding. */
  onSessionBound?: ((session: string) => void) | undefined
  className?: string | undefined
  onExit?: (() => void) | undefined
}

/**
 * Spawn-or-reuse, guarded per pane.
 *
 * React 19 runs effects twice in development StrictMode. Without this guard that means two
 * `claude` processes per pane, one of them orphaned — the kind of bug that only shows up as
 * a doubled API bill.
 */
const pending = new Map<string, Promise<string>>()

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
const FALLBACK: Geometry = { cols: 80, rows: 24, cellWidth: 8, cellHeight: 17 }

/**
 * The login shell to spawn.
 *
 * `$SHELL` is not readable from a webview, so this is the portable default rather than the
 * user's own choice; Settings → Terminal takes it over in M11.
 */
const DEFAULT_SHELL = '/bin/bash'

function plausible(cols: number, rows: number): boolean {
  return cols >= 20 && rows >= 5
}

/** What a pane of each kind runs. A diff pane has no process at all. */
function specFor(pane: Pane, cwd: string, project?: string): TerminalSpec | null {
  switch (pane.kind) {
    case 'claude':
      return { program: 'claude', args: [], cwd, project }
    case 'shell':
      return { program: DEFAULT_SHELL, args: ['-l'], cwd, project }
    default:
      return null
  }
}

/**
 * Push the pane's current pixel size down to the child as a cell geometry.
 *
 * Called from two places, and it needs both. `ResizeObserver` fires once immediately when
 * you `observe()` an element — which happens in `PaneSlot`'s layout effect, *before*
 * `TerminalPane`'s effect has created the terminal. That first callback is therefore
 * dropped, and if the pane is never resized again it is also the only one: every child
 * would sit at the fallback 80x24 forever, in a window that is plainly much larger.
 * So the spawn path calls this explicitly once the session exists.
 */
function syncSize(paneId: string): void {
  const host = getHost(paneId)
  const handle = host.terminal
  if (!handle) return
  try {
    handle.fit.fit()
  } catch {
    return
  }
  // Never push a degenerate size at a live child. A pane that is momentarily unlaid-out
  // fits to 2x1, and forwarding that would reflow the TUI into garbage for no reason.
  if (!plausible(handle.term.cols, handle.term.rows)) return
  if (!host.sessionId) return

  const cell = handle.cellSize()
  void sessionApi.resize(host.sessionId, {
    cols: handle.term.cols,
    rows: handle.term.rows,
    cellWidth: cell.width,
    cellHeight: cell.height,
  })
}

async function sessionFor(paneId: string, spec: TerminalSpec, geometry: Geometry): Promise<string> {
  const host = getHost(paneId)
  // The host may already know its session: this pane is re-mounting, or it was evicted and
  // rehydrated from the ledger. Spawning again would orphan a live child — for a Claude
  // pane, a duplicated conversation.
  if (host.sessionId) return host.sessionId

  let p = pending.get(paneId)
  if (!p) {
    p = sessionApi
      .spawn({ ...spec, geometry })
      .then((id) => {
        getHost(paneId).sessionId = id
        return id
      })
      .finally(() => pending.delete(paneId))
    pending.set(paneId, p)
  }
  return p
}

export function TerminalPane({
  pane,
  cwd,
  project,
  onSessionBound,
  className,
  onExit,
}: TerminalPaneProps) {
  const paneId = pane.id
  const boundRef = useRef<string | null>(null)
  // Held in refs so a changed callback identity cannot tear the session down and respawn it.
  const boundCb = useRef(onSessionBound)
  boundCb.current = onSessionBound
  const exitCb = useRef(onExit)
  exitCb.current = onExit
  const cwdRef = useRef(cwd)
  cwdRef.current = cwd
  const projectRef = useRef(project)
  projectRef.current = project
  const kindRef = useRef(pane.kind)
  kindRef.current = pane.kind
  const domainSession = pane.session

  useEffect(() => {
    let disposed = false
    const handle = openTerminal(paneId)
    const { term, fit } = handle

    // The domain may already hold a session for this pane — a restored workspace, or a pane
    // that was detached and re-docked. Adopt it before considering a spawn.
    if (domainSession && !getHost(paneId).sessionId) {
      getHost(paneId).sessionId = domainSession
    }

    const spec = specFor({ ...pane, kind: kindRef.current }, cwdRef.current, projectRef.current)
    if (spec === null) return

    try {
      fit.fit()
    } catch {
      /* the slot may not be laid out yet on the very first frame */
    }

    const measured = plausible(term.cols, term.rows)
    const cell = measured
      ? handle.cellSize()
      : { width: FALLBACK.cellWidth, height: FALLBACK.cellHeight }
    const geo: Geometry = measured
      ? { cols: term.cols, rows: term.rows, cellWidth: cell.width, cellHeight: cell.height }
      : FALLBACK

    ;(async () => {
      const id = await sessionFor(paneId, spec, geo)
      if (disposed) return

      // Tell the domain which session this pane holds, once. The binding is what survives a
      // restart: the id is the value passed to `claude --session-id`.
      if (boundRef.current !== id) {
        boundRef.current = id
        boundCb.current?.(id)
      }

      // The screen mirror, so a pane opening onto an already-running session paints
      // immediately instead of waiting for the child's next byte.
      //
      // Exactly once per host. A split or a close remounts the surviving leaf — React swaps
      // a leaf node for a split node at that position — and this terminal already holds
      // those bytes; writing them again appends a second copy of the whole transcript. The
      // host survives the remount, so the flag on it is what makes "once" mean once.
      const host = getHost(paneId)
      const [state, alt] = await Promise.all([
        host.hydrated ? Promise.resolve(new ArrayBuffer(0)) : sessionApi.scrollback(id),
        sessionApi.inAlternateScreen(id),
      ])
      if (disposed) return
      if (!host.hydrated) {
        // A host that was released and is coming back still holds its pre-detach screen.
        // The mirror replaces that rather than following it.
        if (host.needsReset) {
          term.reset()
          host.needsReset = false
        }
        const prior = new Uint8Array(state)
        if (prior.byteLength > 0) term.write(prior)
        host.hydrated = true
      }

      // The ack goes in `term.write`'s completion callback, not here. Reaching this line
      // only means the bytes arrived; the callback fires once xterm has actually parsed
      // them, which is the rate the session should be pacing itself against. Acking on
      // arrival would report a speed this renderer cannot sustain and would turn credit
      // control back into no control at all.
      await sessionApi.attach(id, geo, (data) => {
        const bytes = new Uint8Array(data)
        term.write(bytes, () => sessionApi.ack(id, bytes.byteLength))
      })

      if (alt) {
        // A fullscreen TUI's own model is authoritative for everything the screen mirror
        // does not track — OSC 8 hyperlinks, OSC 52 clipboard traffic, DEC 2026 sync
        // framing. Nudging the size by one column makes it repaint from that model.
        await sessionApi.resize(id, { ...geo, cols: Math.max(1, geo.cols - 1) })
        await sessionApi.resize(id, geo)
      }

      // Now that the session exists, adopt the pane's real size. Layout has certainly
      // settled by this point — several IPC round trips have happened since mount.
      requestAnimationFrame(() => syncSize(paneId))
    })().catch((e) => console.error('[cide] terminal pane failed to start', e))

    const onData = term.onData((data) => {
      const id = getHost(paneId).sessionId
      if (id) void sessionApi.write(id, data)
    })

    // Exit polling is a placeholder: M7 replaces it with the `cide://session-state` event
    // driven by Claude Code hooks, which knows the difference between "idle" and "gone".
    const poll = window.setInterval(async () => {
      const id = getHost(paneId).sessionId
      if (!id) return
      if (await sessionApi.hasExited(id)) {
        window.clearInterval(poll)
        exitCb.current?.()
      }
    }, 1000)

    return () => {
      disposed = true
      onData.dispose()
      window.clearInterval(poll)
      const id = getHost(paneId).sessionId
      // Detach the sink, never the session: the child keeps running and this pane can be
      // re-attached from another window without the process noticing.
      if (id) void sessionApi.detach(id)
    }
    // Deliberately keyed on the pane id and its domain session alone. Including the
    // callbacks or the pane object would tear down and re-attach the terminal on every
    // parent render, which is exactly the churn the host registry exists to avoid.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paneId, domainSession])

  return <PaneSlot paneId={paneId} className={className} onResize={() => syncSize(paneId)} />
}
