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
import { useContextMenu, type MenuEntry } from '@/menus'
import { exitMarkerBytes, markFor } from './exitMarker'
import { acknowledge } from './awaiting'
import {
  clipboard,
  diag,
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
  /**
   * This pane is continuing session X.
   *
   * For `claude` that is `--resume X` — with `fork`, `--fork-session` beside it. **For
   * anything else it is a screen replay**, because a shell has no such flag: `session_spawn`
   * seeds the new child's screen mirror with the screen X left behind at the last quit, and
   * writes a line under it saying the text is dead. The two readings are the same sentence;
   * only what a program can do about it differs. See `cmd/session.rs`.
   */
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
 * A shell never resumes a *conversation* — `--resume` means nothing to bash — but a restored
 * one does name the session it is continuing, and Rust replays that session's parting screen
 * into the new child's mirror. See `TerminalSpec.resume`, and the decision in
 * `lifecycle::restore_notice`: what comes back is the visible screen, it is dead text, and the
 * line printed under it says so.
 *
 * `pane.session` rather than anything in `restore`, because `SessionRestore` deliberately
 * answers `Fresh` for every shell — `plan_restore` is about *conversations*, and a shell has
 * none. Guarded on `restore` being present at all, which is the only thing that says this
 * pane's `session` names a child from a previous process rather than a live one.
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
    case 'shell': {
      const prior = restore !== undefined ? (pane.session ?? undefined) : undefined
      return { program: DEFAULT_SHELL, args: ['-l'], cwd, project, resume: prior }
    }
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
    return
  }
  host.lastGeometry = geo

  void sessionApi.resize(host.sessionId, geo).catch((e) => {
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
function reportWriteFailure(paneId: string, error: unknown): void {
  if (writeFailures.has(paneId)) return
  writeFailures.add(paneId)
  const line = `pane ${paneId}: session write failed — keystrokes are being dropped: ${String(error)}`
  console.error(`[cide] ${line}`)
  void diag.log(line).catch(() => {})
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

/**
 * Send text to the child as if it had been typed.
 *
 * This is the whole of Paste, and it is why the item can exist at all: WebKit refuses
 * `document.execCommand('paste')` from page script, so the native menu's Paste is the only
 * one that works on a plain input — see `menus/model.ts`. A terminal is the one surface where
 * that limitation does not bite, because "paste" here means *write bytes to a pty*, and this
 * app already writes bytes to a pty on every keystroke.
 *
 * Bracketed paste is deliberately not added around it. Whether the child wants
 * `ESC[200~ … ESC[201~` is the child's own DECSET 2004 state, which lives in the vt100 mirror
 * on the Rust side and is not on the wire; wrapping unconditionally would make a plain `bash`
 * print the literal escape codes. The cost of not wrapping is that a multi-line paste into
 * `claude` submits at the first newline, which is the same thing typing it would do.
 */
async function pasteInto(paneId: string): Promise<void> {
  const id = getHost(paneId).sessionId
  if (!id) return
  const text = await clipboard.readText()
  if (text === '') return
  await sessionApi.write(id, text)
}

/**
 * Put the terminal's selection on the system clipboard.
 *
 * Through the Tauri plugin rather than `navigator.clipboard.writeText`. The async Clipboard
 * API needs a secure context and a user-gesture-adjacent permission decision, and a menu item
 * activated by keyboard is on the wrong side of that in WebKitGTK — the write silently
 * resolves against nothing. The plugin writes through the compositor's own selection, which
 * is what every other application on the desktop reads.
 */
async function copySelection(paneId: string): Promise<void> {
  const text = getHost(paneId).terminal?.term.getSelection() ?? ''
  if (text === '') return
  await clipboard.writeText(text)
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

  /*
   * The terminal body's own menu: copy, paste, clear, select all.
   *
   * Built at open time by `useContextMenu`, which is what lets Copy report *why* it is
   * unavailable — there is no selection — instead of appearing enabled and doing nothing.
   *
   * `items` reads the live host rather than closing over anything: this component owns no
   * part of the terminal (`paneHosts` does), and the host outlives every mount.
   */
  const menuItems = (): MenuEntry[] => {
    const host = getHost(paneId)
    const term = host.terminal?.term
    const selection = term?.getSelection() ?? ''
    return [
      {
        id: 'copy',
        label: 'Copy',
        // No `command`: there is no `edit.copy` in `cide-core::commands`, and naming one that
        // does not exist would put an empty chip slot beside the item for ever. Copy in a
        // terminal is Ctrl+Shift+C by convention and is the terminal's own binding, not the
        // key gate's.
        run:
          selection === ''
            ? undefined
            : () => {
                void copySelection(paneId).catch((error: unknown) => {
                  console.error('[cide] copy failed', error)
                })
              },
        disabledReason: selection === '' ? 'Nothing is selected in this terminal' : undefined,
      },
      {
        id: 'paste',
        label: 'Paste',
        command: 'terminal.paste',
        run:
          host.sessionId === undefined
            ? undefined
            : () => {
                void pasteInto(paneId).catch((error: unknown) => {
                  console.error('[cide] paste failed', error)
                  void diag
                    .log(`pane ${paneId}: paste failed — ${String(error)}`)
                    .catch(() => {})
                })
              },
        disabledReason: host.sessionId === undefined ? 'This pane has no session yet' : undefined,
      },
      { kind: 'separator' },
      {
        id: 'select-all',
        label: 'Select all',
        run: term === undefined ? undefined : () => term.selectAll(),
      },
      {
        id: 'clear',
        label: 'Clear',
        command: 'terminal.clear',
        // xterm's own scrollback, not the child's and not the Rust screen mirror.
        //
        // Clearing the mirror would need a command, and that command would be wrong: the
        // mirror is per *session*, so one pane clearing it would blank the mirrored pane
        // beside it and the detached window showing the same conversation. What this cannot
        // promise is that the transcript stays gone if this host is later evicted and
        // rehydrated — that path re-reads the mirror by design, because it is the only way a
        // rehydrated pane has any history at all.
        run: term === undefined ? undefined : () => term.clear(),
      },
      { kind: 'separator' },
      {
        /*
         * Interrupt, as a real thing rather than a menu entry for a command nobody wired.
         *
         * `claude.restart`, `claude.fork` and `claude.mirror` are the other three the brief
         * names, and none of them is reachable from here: each needs the project and tab a
         * pane sits in, which only `App.tsx` holds. They are reported rather than drawn —
         * an item that logs "command not handled by this window" is the dead control this
         * project keeps finding.
         *
         * This one needs none of that. Interrupting is `ETX` on the pty, which is the same
         * write every keystroke in this pane already performs.
         */
        id: 'interrupt',
        label: 'Interrupt',
        run:
          host.sessionId === undefined
            ? undefined
            : () => {
                const id = host.sessionId
                if (id === undefined) return
                acknowledge(id)
                void sessionApi.write(id, '\x03').catch((error: unknown) => {
                  reportWriteFailure(paneId, error)
                })
              },
        disabledReason: host.sessionId === undefined ? 'This pane has no session yet' : undefined,
      },
    ]
  }

  const { openAt, menu } = useContextMenu({ label: 'Terminal', items: menuItems })
  // The effect below binds a *native* listener to a DOM node React does not own, and it is
  // keyed on the pane rather than on every render — so the handler it captures has to be a
  // ref, or the menu would be built from the first render's closure for ever.
  const openMenu = useRef(openAt)
  openMenu.current = openAt

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

      const host = getHost(paneId)
      const alt = await sessionApi.inAlternateScreen(id)
      if (disposed) return

      // The ack goes in `term.write`'s completion callback, not here. Reaching this line
      // only means the bytes arrived; the callback fires once xterm has actually parsed
      // them, which is the rate the session should be pacing itself against. Acking on
      // arrival would report a speed this renderer cannot sustain and would turn credit
      // control back into no control at all.
      const deliver = (bytes: Uint8Array) => {
        term.write(bytes, () => paneSession.ack(paneId, id, bytes.byteLength))
      }

      // Live frames that arrive before the snapshot has been written.
      //
      // They can, and the ordering matters: the sink is registered on the Rust side the
      // moment `session_attach` runs, so the channel can deliver its first frame while the
      // command's own reply — the screen those frames continue from — is still in flight.
      // Writing them in arrival order would paint the continuation and then paint the screen
      // it continues from on top of it. Queued rather than dropped, because they are already
      // charged against this sink's credit and only `deliver` pays that back.
      let painted = false
      const queued: Uint8Array[] = []

      // Attaching *by pane*, not by window. Two panes mirroring one session in one window
      // used to be one attachment on the Rust side, so opening a mirror detached the pane
      // being mirrored — see `paneSession` in the IPC client.
      //
      // The screen comes back from `attach` itself, taken at the instant this sink was
      // registered. It used to be a separate `session.scrollback` beforehand, and the gap
      // between the two calls is a window in which the Rust mirror runs ahead of the sinks:
      // bytes landing there were painted from the snapshot and then delivered again as live
      // output. See `cide_pty::PtySession::attach_with_snapshot`.
      const screen = await paneSession.attach(paneId, id, geo, (data) => {
        const bytes = new Uint8Array(data)
        if (painted) deliver(bytes)
        else queued.push(bytes)
      })
      if (disposed) return

      // Exactly once per host. A split or a close remounts the surviving leaf — React swaps
      // a leaf node for a split node at that position — and this terminal already holds
      // those bytes; writing them again appends a second copy of the whole transcript. The
      // host survives the remount, so the flag on it is what makes "once" mean once.
      if (!host.hydrated) {
        // A host that was released and is coming back still holds its pre-detach screen.
        // The mirror replaces that rather than following it.
        if (host.needsReset) {
          term.reset()
          host.needsReset = false
        }
        const prior = new Uint8Array(screen)
        if (prior.byteLength > 0) term.write(prior)
        host.hydrated = true
      }

      painted = true
      for (const bytes of queued) deliver(bytes)
      queued.length = 0

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
      if (!id) return
      // Typing at a session is the clearest possible statement that the user has seen it, so
      // it clears the awaiting marker and this window's contribution to `Awaiting: X`.
      acknowledge(id)
      // `void` with no `catch` was swallowing the one failure a user cannot diagnose: a
      // keystroke that never reached the child looks exactly like a keystroke the child
      // ignored. Once per pane, because if writes are failing they are failing on every
      // character and a per-keystroke log is its own outage.
      sessionApi.write(id, data).catch((e) => reportWriteFailure(paneId, e))
    })

    /*
     * The terminal's own context menu, bound natively.
     *
     * React does not own this element — `paneHosts` builds it once and `PaneSlot` moves it
     * between slots — so there is no JSX node to hang `onContextMenu` on. A wrapper `<div>`
     * around `PaneSlot` was the alternative and it loses: the host inside resolves
     * `inset: 0` against the slot, so an extra box in that chain is one more place for a
     * height to come out zero, which is the failure mode `PaneSlot`'s own comment is about.
     *
     * `data-native-menu="false"` opts the whole subtree out of the webview's menu. The global
     * suppression already covers everything that is not a text entry, and xterm's helper
     * *is* a `<textarea>` — sitting under the cursor, so a right-click at the caret would
     * otherwise get WebKit's menu, with "Inspect element" in it, over our own.
     */
    const hostEl = getHost(paneId).el
    hostEl.setAttribute('data-native-menu', 'false')
    const onMenu = (ev: MouseEvent) => {
      ev.preventDefault()
      // The innermost surface's menu wins — `useContextMenu`'s stated rule, enforced here by
      // hand because this is a native listener rather than a React one. React 19 delegates to
      // the root container, so stopping the native event is what keeps an outer surface from
      // also opening one. (The pane title bar's own menu is not among them: its handler is on
      // the 26px bar, which the terminal is not inside.)
      ev.stopPropagation()
      const target = ev.target instanceof HTMLElement ? ev.target : null
      openMenu.current(ev.clientX, ev.clientY, target)
    }
    hostEl.addEventListener('contextmenu', onMenu)

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
      // The host outlives this mount, so the listener has to come off with it — otherwise a
      // pane remounted by a split would accumulate one right-click handler per mount and open
      // as many menus.
      hostEl.removeEventListener('contextmenu', onMenu)
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

  return (
    <>
      <PaneSlot paneId={paneId} className={className} onResize={() => syncSize(paneId)} />
      {/* Portals out of here entirely; it is in the tree so React owns its lifetime. */}
      {menu}
    </>
  )
}
