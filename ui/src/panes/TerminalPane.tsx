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
import { takeSpawnPlan } from '@/layout/spawnPlans'
import { exitMarkerBytes, markFor } from './exitMarker'
import {
  events,
  paneSession,
  session as sessionApi,
  type Geometry,
  type Pane,
  type PaneRestore,
} from '@/ipc/client'

export interface TerminalSpec {
  program: string
  args: string[]
  cwd: string
  /** Decides which IDE server this child is told about. See `session.spawn`. */
  project?: string | undefined
  /** Continue an existing conversation; with `fork`, branch from it instead. */
  resume?: string | undefined
  fork?: boolean | undefined
}

export interface TerminalPaneProps {
  pane: Pane
  /** Where a fresh child is spawned — the project's primary root. */
  cwd: string
  /** The project this pane belongs to, so its child reaches the right IDE server. */
  project?: string | undefined
  /**
   * The project's primary session, which is what a `forkPrimary` split branches from.
   *
   * Passed in rather than looked up here, so this component keeps knowing nothing about the
   * shape of the workspace.
   */
  primarySession?: string | undefined
  /**
   * This pane's entry in the launch plan, when the workspace was restored.
   *
   * Absent for a pane created during this run — those always spawn fresh, because the user
   * just asked for them. Present, it is the only thing that knows the pane's `session` names
   * a conversation from a *previous* process: `Resumable` becomes `claude --resume <id>`,
   * and `Fresh` becomes an ordinary new session. Without it a restored pane attached to a
   * `SessionId` no process has ever heard of and sat there blank.
   */
  restore?: PaneRestore | undefined
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

/**
 * What a pane of each kind runs. A diff pane has no process at all.
 *
 * `restore` is what makes a restored Claude pane pick its conversation up rather than start
 * a new one. `plan_restore` has already checked that Claude Code holds a transcript for that
 * id under this cwd, so `Resumable` is a claim about the filesystem and not a guess; a
 * `Fresh` entry — a shell, or a Claude pane whose transcript is gone — spawns as usual.
 *
 * A shell never resumes. `--resume` means nothing to bash, and its scrollback died with its
 * process.
 */
function specFor(
  pane: Pane,
  cwd: string,
  project?: string,
  restore?: PaneRestore | undefined,
): TerminalSpec | null {
  switch (pane.kind) {
    case 'claude': {
      const resume = restore?.restore.kind === 'resumable' ? restore.restore.session : undefined
      return { program: 'claude', args: [], cwd, project, resume }
    }
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

/**
 * Write `— exited —` into the pane whose child has gone.
 *
 * M2's acceptance criterion is "kill the child: EOF arrives and the pane shows `— exited —`",
 * and that string existed only in a comment. `TerminalPane` declared an `onExit` prop and no
 * caller ever passed one, so a dead child produced no visible change anywhere: the cursor
 * sat there and the pane looked idle. The criterion is not decoration — it is the only
 * user-visible proof that `drop(pair.slave)` works, because without that drop the master
 * never sees EOF and this branch is never reached.
 *
 * Written into the terminal rather than rendered as React chrome: see `exitMarker.ts`, which
 * owns the wording and the framing so that a check script can run them without a DOM.
 *
 * `code` is the child's real status — 1 for a failure, 137 for the OOM reaper, 143 for the
 * SIGTERM this app sends on quit — and it is omitted on the one path that cannot know it.
 * Rust reports these honestly now; before this it published a flat -1 for every dead pane,
 * and the pane printed nothing but `— exited —` either way, so a session the machine killed
 * looked exactly like one that finished.
 *
 * Returns whether this call is the one that wrote it. Exit reaches a pane from two
 * directions — the `cide://session-state` event, and the one-shot check for a session that
 * was already dead before this pane attached — and `onExit` must fire once however many
 * arrive. The first one to arrive is the one whose code is shown; that is the event whenever
 * the pane was mounted at the time, which is every case but rehydration.
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
 * Returns the argument for `markExited`, or `null` for "do not mark" — see `markFor`, which
 * owns that decision and is the part a check script can run.
 *
 * The `SessionExit` the command returns is handed straight to `markFor`, whose parameter is
 * `exitMarker.ts`'s own `ExitAnswer`. That assignment is the equivalence check between the two:
 * `exitMarker.ts` cannot import the generated bindings, because it is deliberately a pure
 * module that `check-exit-marker.mjs` compiles standalone, so if the Rust enum ever gains a
 * variant or renames a field, this line stops typechecking rather than silently falling through
 * `markFor`'s switch.
 *
 * A rejection is `null`: a question that could not be asked is not evidence the child is gone,
 * and marking a live pane `— exited —` is the worse of the two mistakes.
 */
async function exitMark(session: string): Promise<{ code?: number } | null> {
  return sessionApi.exit(session).then(markFor, (e) => {
    console.error('[cide] could not ask about session exit', e)
    return null
  })
}

/**
 * Whether the registry still holds a *running* child for this session.
 *
 * Asked before adopting a session id the domain already has, so the answer decides whether
 * this pane attaches to an existing conversation or spawns its own.
 *
 * `running` is the only yes, and `reaping` deliberately is not: that child is already dead and
 * merely not yet reaped, so adopting it would attach a pane to a corpse and leave it blank.
 * `unknown` — the ordinary shape of a `SessionId` restored from `workspace.json`, whose process
 * died with the previous run — is a no for the same reason.
 *
 * Over `session.exit` rather than `session.hasExited` so this file asks the registry one
 * question in one shape. The predicate command still exists for the window audit, which wants
 * a bool and has no use for the code.
 */
async function sessionIsLive(session: string): Promise<boolean> {
  return sessionApi.exit(session).then(
    (answer) => answer.kind === 'running',
    () => false,
  )
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
  primarySession,
  restore,
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
  const primaryRef = useRef(primarySession)
  primaryRef.current = primarySession
  // A ref, not a dependency: the plan is read once at launch and never refreshed, so its
  // identity changing means the parent re-rendered, not that this pane should respawn.
  const restoreRef = useRef(restore)
  restoreRef.current = restore
  const domainSession = pane.session

  useEffect(() => {
    let disposed = false
    const handle = openTerminal(paneId)
    const { term, fit } = handle

    const restoreEntry = restoreRef.current

    // The domain may already hold a session for this pane — a pane that was detached and
    // re-docked, or one re-mounting after a split. Adopt it before considering a spawn.
    //
    // Not when this pane is in the launch plan. There, `pane.session` names a child from the
    // process that wrote `workspace.json`, and adopting it attaches to a session the registry
    // has never heard of: the pane comes up blank and stays that way. Such a pane spawns
    // instead, resuming the old conversation when the plan says it can — but only
    // after `sessionIsLive` has said the old id really is dead, because the plan is read once
    // and a pane that has already spawned in this run still carries its entry. Re-docking one
    // into a second window must not fork a rival child.
    if (domainSession && !getHost(paneId).sessionId && restoreEntry === undefined) {
      getHost(paneId).sessionId = domainSession
    }

    const spec = specFor(
      { ...pane, kind: kindRef.current },
      cwdRef.current,
      projectRef.current,
      restoreEntry,
    )
    if (spec === null) return

    // A split may have asked for something a pane's `kind` cannot express. Taken here, once:
    // StrictMode mounts this effect twice in development, and a plan that survived a read
    // would fork twice and orphan one of the children.
    const plan = takeSpawnPlan(paneId)
    if (plan?.kind === 'mirror') {
      // No process at all. A mirror is a second sink on a session that already exists, so
      // adopting the id is the whole operation — spawning here would start a rival child
      // and the two panes would diverge instead of showing one conversation.
      if (!getHost(paneId).sessionId) getHost(paneId).sessionId = plan.session
    } else if (plan?.kind === 'forkPrimary' && primaryRef.current) {
      spec.resume = primaryRef.current
      spec.fork = true
    }

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
      // A planned pane's `pane.session` was not adopted above, because at launch it names a
      // dead child. It is adopted here if the registry proves otherwise — which is the case
      // for a pane that already spawned in this run and is now being re-docked or re-mounted
      // in a window that still holds the plan.
      if (restoreEntry !== undefined && domainSession && !getHost(paneId).sessionId) {
        if (await sessionIsLive(domainSession)) getHost(paneId).sessionId = domainSession
        if (disposed) return
      }

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
      //
      // Attaching *by pane*, not by window. Two panes mirroring one session in one window
      // used to be one attachment on the Rust side, so opening a mirror detached the pane
      // being mirrored — see `paneSession` in the IPC client.
      await paneSession.attach(paneId, id, geo, (data) => {
        const bytes = new Uint8Array(data)
        term.write(bytes, () => paneSession.ack(paneId, id, bytes.byteLength))
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

      // A session that was already dead when this pane attached will never produce an event
      // — the watcher fired before anyone was listening. One check, not a poll: this is the
      // rehydration case (a host evicted and re-created after its child had gone), and it is
      // answered once at attach time rather than every second for the life of the pane.
      //
      // The answer carries the code now. It used to be a bool, so this path printed a bare
      // `— exited —` over a status the registry was still holding — the same pane, the same
      // dead child, a different sentence depending only on whether it happened to be mounted.
      const mark = await exitMark(id)
      if (mark && markExited(paneId, mark.code)) exitCb.current?.()
    })().catch((e) => console.error('[cide] terminal pane failed to start', e))

    const onData = term.onData((data) => {
      const id = getHost(paneId).sessionId
      if (id) void sessionApi.write(id, data)
    })

    // Exit arrives as an event now. It used to be a `session.hasExited` round trip per pane
    // per second, forever, in every window — twelve panes was twelve IPC calls a second to
    // learn nothing, and it still took up to a second to notice. `cide://session-state` is
    // emitted once, by the watcher the session's own spawn started.
    //
    // Listening is asynchronous, so a pane disposed before the subscription lands has to
    // unsubscribe the handle it never got to store.
    let unlistenExit: (() => void) | null = null
    void events
      .onSessionState((session, state) => {
        if (state.state !== 'exited') return
        // Not `id` from the async block above: this handler outlives it, and a pane that
        // respawned holds a different session by now.
        if (session !== getHost(paneId).sessionId) return
        // `state.code` is the whole reason the Rust side threads the status out of `wait()`.
        // Dropping it here was the last link in the chain, and it made the change invisible.
        if (markExited(paneId, state.code)) exitCb.current?.()
      })
      .then((fn) => {
        if (disposed) fn()
        else unlistenExit = fn
      })
      .catch((e) => console.error('[cide] terminal pane cannot hear about exits', e))

    return () => {
      disposed = true
      onData.dispose()
      unlistenExit?.()
      const id = getHost(paneId).sessionId
      // Detach the sink, never the session: the child keeps running and this pane can be
      // re-attached from another window without the process noticing. By pane, so closing
      // one mirror leaves the other attached.
      if (id) void paneSession.detach(paneId, id)
    }
    // Deliberately keyed on the pane id and its domain session alone. Including the
    // callbacks or the pane object would tear down and re-attach the terminal on every
    // parent render, which is exactly the churn the host registry exists to avoid.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paneId, domainSession])

  return <PaneSlot paneId={paneId} className={className} onResize={() => syncSize(paneId)} />
}
