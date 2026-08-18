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
import { useEffect, useRef, useState, type CSSProperties } from 'react'
import { PaneSlot } from '@/layout/PaneSlot'
import { forgetSession, getHost, openTerminal, peekHost, setHostBusy } from '@/layout/paneHosts'
import { setPathLinkEnv } from '@/terminal/pathLinks'
import { copyTerminalSelection, pasteIntoTerminal } from '@/terminal/clipboard'
import type { TerminalPaneKind } from '@/terminal/keys'
import { takeSpawnPlan } from '@/layout/spawnPlans'
import { useContextMenu, type MenuEntry } from '@/menus'
import {
  exitMarkerBytes,
  isRecoverableSessionError,
  markFor,
  spawnFailureBytes,
  spawnFailureText,
} from './exitMarker'
import { restartOffer, type RestartMode, type RestartOffer } from './restartRule'
import { TerminalFindBar } from './TerminalFindBar'
import { openTerminalFind } from '@/terminal/findStore'
import findStyles from './TerminalFindBar.module.css'
import { registerRestarter } from './paneRestart'
import { acknowledge } from './awaiting'
import {
  claudeSession,
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
  /**
   * The project's roots, for resolving a file path printed in this pane.
   *
   * Separate from `cwd` — which is only `roots[0]` — because resolution tries every root: a
   * multi-root project prints paths relative to whichever one the tool was run in. Absent means
   * this pane offers no file links at all, which is the honest state for a pane with no project.
   */
  roots?: readonly string[] | undefined
  /**
   * Ctrl+click on a file path in this pane's output.
   *
   * The pane knows the path and the line; it deliberately does not know what "open" means. That
   * is the caller's, because opening is a workspace mutation plus a caret request, and both of
   * those live above the pane tree. Absent means the links are inert — they will not even be
   * offered, because `pathLinks.ts` needs the whole environment or none of it.
   */
  onOpenPath?: ((path: string, at: { line: number; column: number } | null) => void) | undefined
  /**
   * Ctrl+click on a *directory* in this pane's output.
   *
   * Separate from [`onOpenPath`] because the two answers are different in kind and are offered
   * by different windows: a directory cannot be opened in an editor, so the gesture shows it in
   * the file tree instead — and a detached pane has no file tree, so it passes this and only
   * this as absent. `pathLinks.ts` then offers no directory links there rather than an underline
   * whose command would refuse.
   */
  onRevealPath?: ((path: string) => void) | undefined
  /** Called once, when a spawn succeeds, so the domain can record the binding. */
  onSessionBound?: ((session: string) => void) | undefined
  className?: string | undefined
  /*
   * `onExit` was here, and it is deliberately gone rather than merely unused.
   *
   * It was declared, held in a ref, and fired from both of the paths that learn a child has
   * died — and **no caller ever passed one**. `PaneBody` and `DetachedPaneWindow` are the only
   * two mount sites and neither supplied it, so the exit signal reached the transcript and
   * nothing else: no title bar state, no store, no chrome, and no way back. That is this
   * project's recurring defect wearing a prop.
   *
   * What replaced it is the pane acting on its own exit — see `restartRule.ts` and the bar this
   * component now renders — which is the thing the prop was presumably reserved for and never
   * connected to. A caller that needs to hear about exits should subscribe to
   * `cide://session-state`, which is where the fact actually comes from.
   */
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
 *
 * Returns a promise that settles once the child has actually been told, so a caller that
 * pushes a size of its own afterwards — the alt-screen repaint nudge in `start` — can order
 * itself behind this one. Two un-awaited `session_resize` calls in flight at once is how a
 * pane ends up parked at the nudge's transient `cols - 1`.
 */
function syncSize(paneId: string): Promise<void> {
  const host = getHost(paneId)
  const handle = host.terminal
  if (!handle) return Promise.resolve()
  try {
    handle.fit.fit()
  } catch {
    return Promise.resolve()
  }
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
 * was already dead before this pane attached — and the *marker* must be written once however
 * many arrive. The first one to arrive is the one whose code is shown; that is the event
 * whenever the pane was mounted at the time, which is every case but rehydration.
 *
 * The **offer** over the pane is deliberately not one-shot; see `noteExit`. A pane remounted by
 * a split learns of the exit again through the one-shot check, and it must come back showing
 * the way out even though the marker was written for the previous mount. Tying the control to
 * this return value is how it would have gone missing on exactly the panes that had been
 * around longest.
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
 * Paste into this pane, and copy out of it.
 *
 * Both bodies used to live here and both are now `terminal/clipboard.ts`, because the same two
 * gestures are reachable from three places — this menu, the `terminal.paste` command, and
 * Ctrl+C / Ctrl+V inside the terminal — and three implementations is how a menu item and a
 * keystroke with the same name come to do different things. In particular the old paste wrote
 * the clipboard straight at the pty, so a multi-line paste executed line by line in `bash` and
 * submitted at the first newline in `claude`; `term.paste` wraps in bracketed paste when, and
 * only when, the child has asked for it.
 *
 * The pane's kind goes through because it decides what happens when the clipboard holds no
 * *text*: in a Claude pane the `^V` byte is handed to the CLI so its own image paste runs. That
 * is why this menu item now reaches image paste too — it used to be the one route that could
 * only ever paste text, which was the half of the bug nobody had noticed.
 */
async function pasteInto(paneId: string, kind: TerminalPaneKind): Promise<void> {
  const term = getHost(paneId).terminal?.term
  if (!term) return
  await pasteIntoTerminal(term, kind)
}

async function copySelection(paneId: string): Promise<void> {
  const term = getHost(paneId).terminal?.term
  if (!term) return
  await copyTerminalSelection(term)
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
  roots,
  onOpenPath,
  onRevealPath,
  onSessionBound,
  className,
}: TerminalPaneProps) {
  const paneId = pane.id
  const boundRef = useRef<string | null>(null)
  // Held in refs so a changed callback identity cannot tear the session down and respawn it.
  const boundCb = useRef(onSessionBound)
  boundCb.current = onSessionBound
  const cwdRef = useRef(cwd)
  cwdRef.current = cwd
  const projectRef = useRef(project)
  projectRef.current = project
  const kindRef = useRef(pane.kind)
  kindRef.current = pane.kind
  const primaryRef = useRef(primarySession)
  primaryRef.current = primarySession
  // Refs, like every other prop here, and for the reason at the foot of the effect: the effect
  // is keyed on the pane and its session alone, so anything read from inside it has to be read
  // *late*. A root list or an open handler captured at mount would keep answering with the
  // project this pane was first rendered under.
  const rootsRef = useRef(roots)
  rootsRef.current = roots
  const openPathRef = useRef(onOpenPath)
  openPathRef.current = onOpenPath
  const revealPathRef = useRef(onRevealPath)
  revealPathRef.current = onRevealPath
  // A ref, not a dependency: the plan is read once at launch and never refreshed, so its
  // identity changing means the parent re-rendered, not that this pane should respawn.
  const restoreRef = useRef(restore)
  restoreRef.current = restore
  const domainSession = pane.session

  /** What this pane runs, in the two-valued form the clipboard rule and the offer both use. */
  const runKind: TerminalPaneKind = pane.kind === 'claude' ? 'claude' : 'shell'

  /*
   * This pane's child has gone, and with what status.
   *
   * React state as well as `host.exitMarked`, because the two answer different questions: the
   * host flag is "has the marker been written into this terminal" and must survive every mount,
   * and this is "does the pane currently offer a way back", which is a render. Seeded *from*
   * the host so that a pane remounted by a split — React swaps a leaf node for a split node in
   * the same position — comes back still offering it, rather than showing a dead terminal with
   * the control gone because the one-shot that set it had already fired.
   */
  const [exit, setExit] = useState<{ code?: number } | null>(() =>
    peekHost(paneId)?.exitMarked === true ? {} : null,
  )
  /**
   * Whether Claude Code still holds a transcript for the session that just died.
   *
   * Asked of Rust once, when the pane learns it has exited, because it is a fact about the
   * filesystem. `false` until the answer arrives, which is the safe direction: the offer gains
   * the Resume button a round trip later rather than showing one that cannot work.
   */
  const [resumable, setResumable] = useState(false)
  /**
   * The restart itself, installed by the effect.
   *
   * A ref rather than state: it closes over the terminal handle and the pane's spec, both of
   * which belong to the effect, and re-rendering because a function identity changed would be
   * churn on a component whose whole design is to re-render as little as possible.
   */
  const restartRef = useRef<((mode: RestartMode) => Promise<void>) | null>(null)
  const runRestart = (mode: RestartMode): void => {
    void restartRef.current?.(mode).catch((error: unknown) => {
      console.error('[cide] restart failed', error)
      void diag.log(`pane ${paneId}: restart failed — ${spawnFailureText(error)}`).catch(() => {})
      getHost(paneId).terminal?.term.write(spawnFailureBytes(spawnFailureText(error)))
    })
  }

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
                void pasteInto(paneId, runKind).catch((error: unknown) => {
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
        /*
         * *Find…*, and it is here for discoverability rather than for convenience.
         *
         * Ctrl+F is resolved focus-scoped in `terminal/keys.ts` rather than bound in
         * `cide_core::keymap` — the argument is in the registry entry for `terminal.find` — and
         * the price of that is a chord Settings → Keymap does not list and the palette draws no
         * chip for. A menu item is the answer to the same question a chip answers: *what can I
         * do to this pane?* The palette row exists too, for the keyboard.
         *
         * `command:` all the same. It costs nothing today (there is no binding, so no chip is
         * drawn) and it is what makes the chip appear by itself the moment a user binds the
         * command in their own `keymap.json`, which is the whole point of it being a command.
         *
         * No session is required, unlike Paste: a pane whose child has exited still holds its
         * transcript, and that is one of the times somebody most wants to search it.
         */
        id: 'find',
        label: 'Find…',
        command: 'terminal.find',
        run: term === undefined ? undefined : () => openTerminalFind(paneId),
        disabledReason: term === undefined ? 'This pane has no terminal yet' : undefined,
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
         * `claude.fork` and `claude.mirror` are two the brief names and neither is reachable
         * from here: each needs the project and tab a pane sits in, which only `App.tsx` holds.
         * They are reported rather than drawn — an item that logs "command not handled by this
         * window" is the dead control this project keeps finding.
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
      {
        /*
         * Restart, which the comment above used to explain the absence of.
         *
         * `claude.restart` was `Command::unavailable("needs a respawn path in the pane host;
         * kill alone is not a restart")`, and that reason was accurate: only this component
         * knows the pane's geometry and holds the terminal a new child must attach to. The
         * respawn path exists now, so the item is drawn — and it is drawn *here* as well as in
         * the palette because the palette is not where a user with a dead pane looks.
         *
         * No `command:` chip. The registry entry is `claude.restart` and this item restarts a
         * shell pane too, so naming the command would put a Claude-only shortcut beside an
         * action that is not only Claude's.
         *
         * Withheld when the pane has no respawn to run — `specFor` answers `null` for a kind
         * that runs no process, so the effect returns before installing one. Drawing it there
         * would be the dead control this whole item exists to remove.
         */
        id: 'restart',
        label: runKind === 'claude' ? 'Restart session' : 'Restart',
        run: restartRef.current === null ? undefined : () => runRestart('fresh'),
        disabledReason:
          restartRef.current === null ? 'This pane does not run a process' : undefined,
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
    // The kind reaches the terminal at construction, because it decides a keystroke: a Claude
    // pane must not claim Ctrl+V, or the CLI's own image paste stops working. See
    // `terminal/keys.ts`.
    const handle = openTerminal(paneId, runKind)
    const { term, fit } = handle

    /*
     * What a file path printed in this pane means, handed to the link provider `openTerminal`
     * just attached.
     *
     * Getters rather than captured values, because this object is read at *hover* time and this
     * effect runs once per pane: a project switched under a parked pane, or a root added, has to
     * be visible to the next hover and not to the next remount. The provider itself is on the
     * host and outlives every mount, which is exactly why the two are registered separately.
     *
     * A pane with no project, no roots or no handler registers all the same and simply offers
     * nothing — `pathLinks.ts` refuses to provide a link unless the whole environment is there,
     * which is what stops a pane from ever resolving a path against a project it is not in.
     */
    setPathLinkEnv(paneId, {
      get project() {
        return projectRef.current ?? ''
      },
      get roots() {
        return rootsRef.current ?? []
      },
      get cwd() {
        return cwdRef.current
      },
      // `null` rather than a no-op closure when the pane was given no handler: `pathLinks.ts`
      // refuses to offer a link it could not act on, and a closure that quietly returns would
      // look identical to one that works right up until somebody clicked it.
      get open() {
        return openPathRef.current ?? null
      },
      // Same `?? null` and the same reason: a window with no file tree has to be told apart from
      // one whose reveal quietly does nothing, and only the first of those is allowed to draw an
      // underline.
      get reveal() {
        return revealPathRef.current ?? null
      },
    })

    const restoreEntry = restoreRef.current

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

    /**
     * The pane's size right now, as a cell geometry.
     *
     * A function rather than a value computed once at mount, because a restart happens
     * whenever the user asks — minutes later, in a pane that has been dragged to a different
     * size since. Spawning the replacement at the geometry the *first* child was born with
     * would draw its first frame at the wrong width, which for a fullscreen TUI is a ruined
     * screen until something forces a repaint.
     */
    const measure = (): Geometry => {
      try {
        fit.fit()
      } catch {
        /* the slot may not be laid out yet on the very first frame */
      }
      const measured = plausible(term.cols, term.rows)
      const cell = measured
        ? handle.cellSize()
        : { width: FALLBACK.cellWidth, height: FALLBACK.cellHeight }
      return measured
        ? { cols: term.cols, rows: term.rows, cellWidth: cell.width, cellHeight: cell.height }
        : FALLBACK
    }

    /**
     * This pane's child has gone: write the marker, and offer a way back.
     *
     * Reached from both directions — the `cide://session-state` event, and the one-shot check
     * for a child that was already dead when this pane attached — because a pane must offer the
     * control however it learns. `markExited` stays one-shot (the marker is written once per
     * host); the offer is not, so a pane remounted by a split still shows it.
     */
    const noteExit = (code?: number): void => {
      markExited(paneId, code)
      setHostBusy(paneId, false)
      if (disposed) return
      setExit(code === undefined ? {} : { code })

      // Only Claude has a conversation to resume, and only Rust can say whether the transcript
      // is still there — it is a file under `~/.claude/projects`, and a Resume button that
      // spawns `--resume` for a transcript that is gone is a control that fails after it is
      // pressed. A rejection leaves the answer at `false`, which offers one button instead of
      // two rather than offering a broken one.
      const id = getHost(paneId).sessionId
      if (runKind !== 'claude' || id === undefined) return
      void claudeSession
        .resumable(cwdRef.current, id)
        .then((yes) => {
          if (!disposed) setResumable(yes)
        })
        .catch(() => {})
    }

    /**
     * Attach this pane to a session, spawning one if it does not already hold one.
     *
     * Everything from "which session" to "the screen is painted" lives here, as a function
     * rather than as the body of the mount effect, because a restart is exactly this sequence
     * run again against the same terminal. Doing it by unmounting and remounting the pane was
     * the alternative and it is forbidden: `layout/paneHosts.ts` rule 2, and it would throw
     * away the transcript the user is reading the exit code off.
     */
    const start = async (spawnSpec: TerminalSpec): Promise<void> => {
      const geo = measure()
      const id = await sessionFor(paneId, spawnSpec, geo)
      if (disposed) return

      // Tell the domain which session this pane holds, once. The binding is what survives a
      // restart: the id is the value passed to `claude --session-id`.
      if (boundRef.current !== id) {
        boundRef.current = id
        boundCb.current?.(id)
      }

      const host = getHost(paneId)

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

      /*
       * `session_attach` resized the child before it registered this sink — unconditionally,
       * to the `geo` measured at the top of this function, which is now several IPC round
       * trips old. So the cache no longer describes what the child thinks its size is.
       *
       * Clearing it is not belt and braces. A `ResizeObserver` callback landing in any of
       * the awaits above runs `syncSize`, which sends the pane's *true* size and records it
       * here; the attach then quietly puts the child back to the stale one, and the recovery
       * `syncSize` below measures the true size, finds it equal to what the cache says it
       * already sent, and returns without sending anything. The child is then left drawing
       * frames for a geometry the viewport does not have, with no path back until the pane's
       * pixel size changes — which is exactly why maximising the pane "fixed" it.
       *
       * `releaseHost` (see `layout/paneHosts.ts`) already applies this rule for the same
       * reason; the cache means "what this host last told the child, with no other writer
       * since", and `session_attach` is another writer. The cost is one extra resize per
       * attach, and the cache exists to collapse a *drag*, not an attach.
       */
      host.lastGeometry = undefined

      /*
       * Which of the terminal's two buffers the child is painting into.
       *
       * Asked *after* the attach, deliberately: the answer must never be older than the
       * snapshot, or the check below decides against a screen it cannot see. It costs no
       * extra latency — this is the same three sequential round trips as before, reordered.
       */
      const alt = await sessionApi.inAlternateScreen(id)
      if (disposed) return

      /*
       * A hydrated terminal can still be on the wrong buffer, and that is its own bug.
       *
       * `host.hydrated` means "this terminal already holds the mirror's bytes". It says
       * nothing about *which* buffer it holds them in, and the two are not the same claim:
       * this pane's sink is detached in the effect cleanup and re-registered here, so a
       * `\x1b[?1049h` or `\x1b[?1049l` the child wrote in between reached neither this
       * terminal nor — because `hydrated` is still true — the snapshot this branch would
       * otherwise discard. From then on xterm and the child paint different buffers, for
       * good. Both directions were in one bug report: stuck on the alternate buffer the pane
       * loses its scrollbar and xterm turns the wheel into cursor keys aimed at the child, so
       * scrolling edits the agent's prompt; stuck on the normal one the child's
       * cursor-addressed repaints land in rows that have scrolled away, so a question it drew
       * is never seen until a resize forces a full repaint.
       *
       * Dropping `hydrated` rather than writing a bare `\x1b[?1049h` is what makes this safe:
       * the switch on its own would show an empty alternate screen, whereas the snapshot
       * carries the switch *and* the screen that belongs to it (see
       * `cide_pty::reattach_bytes`). It cannot duplicate anything either — the snapshot opens
       * with `\x1b[H\x1b[J`, so it replaces the visible screen — and `needsReset` is false on
       * this path, so the terminal keeps the scrollback the user may have scrolled into.
       */
      if (host.hydrated && alt !== (term.buffer.active.type === 'alternate')) {
        host.hydrated = false
        // Said out loud, because this repair is otherwise invisible and the bug it repairs
        // was reported as "rendering is broken". A line here means the two halves had
        // genuinely drifted; silence over a long session is the honest evidence that the
        // snapshot is now carrying the buffer it belongs to.
        void diag
          .log(
            `pane ${paneId}: terminal was on the ${alt ? 'primary' : 'alternate'} buffer while session ${id} is on the ${alt ? 'alternate' : 'primary'} one; repainting from the mirror`,
          )
          .catch(() => {})
      }

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

      // The pane is attached and painting, so any offer over it is spent.
      //
      // Cleared *here* rather than when the button was pressed, and that is the difference
      // between a control and a trapdoor: a restart whose spawn fails would otherwise take the
      // only way out of the pane away with it, leaving the same dead end the button exists to
      // fix — this time with a `— could not start —` line instead of `— exited —`. React bails
      // out of a re-render when the state is already `null`, so this costs a live pane nothing.
      setExit(null)
      setResumable(false)

      /*
       * Now that the session exists, adopt the pane's real size — and only then nudge a
       * fullscreen TUI into repainting.
       *
       * The order is the fix. The nudge used to run *before* `syncSize` and to use `geo` —
       * the geometry measured before this function's three awaits — so it re-imposed a size
       * the pane may no longer have, on top of the same stale value `session_attach` had
       * already written. Nothing then told `syncSize` the child had been resized behind its
       * back, so it found its cache in agreement with the pane's real size and sent nothing,
       * and the child spent the rest of its life drawing frames for rows the viewport had
       * not got. Maximising the pane changed the pixel size, which is the only thing that
       * broke the agreement — which is why maximising "repaired the rendering".
       *
       * Still inside one `requestAnimationFrame`, and still the same three size pushes in the
       * ordinary case, so this is not another resize bolted on to win a race — it is the same
       * pushes, ordered so that the last one is the size the pane actually has. (A fourth is
       * sent only when a resize genuinely raced the nudge; the guard at the end of the block
       * says why, and it is silent when nothing raced.) Layout has certainly settled by here:
       * several IPC round trips have happened since mount.
       */
      requestAnimationFrame(() => {
        void (async () => {
          await syncSize(paneId)
          // The pane may have been unmounted while the resize was in flight — a re-dock, a
          // split, a closed tab. Its child is somebody else's now.
          if (disposed || !alt) return
          // A fullscreen TUI's own model is authoritative for everything the screen mirror
          // does not track — OSC 8 hyperlinks, OSC 52 clipboard traffic, DEC 2026 sync
          // framing. Nudging the size by one column makes it repaint from that model.
          //
          // Read after the await, so it is the size `syncSize` has just settled on rather
          // than the one measured before this function's three round trips. `geo` is the
          // fallback for the case where `syncSize` had nothing plausible to measure and so
          // recorded nothing.
          const before = getHost(paneId).lastGeometry
          const at = before ?? geo
          await sessionApi.resize(id, { ...at, cols: Math.max(1, at.cols - 1) })
          await sessionApi.resize(id, at)

          /*
           * The nudge is a writer like every other, so it owes the cache the same honesty
           * `session_attach` does — and the two awaits above are a window a `ResizeObserver`
           * callback can land in. When one does, `syncSize` sends the pane's *new* size and
           * records it here, and then the line above quietly puts the child back to `at`. The
           * cache then agrees with a size the child has not got, which is the exact state this
           * whole path exists to make unreachable: nothing corrects it until the pane's pixel
           * size changes again, and for the last frame of a divider drag that may be never.
           *
           * Identity and not equality, because `syncSize` stores a fresh object on every write:
           * a different object means somebody else wrote, whatever the numbers say. Undefined
           * on both sides means nothing was recorded before *or* after — `syncSize` measured
           * nothing plausible, or its resize was rejected and it cleared itself — and in that
           * case there is nothing to correct back to and the next callback retries anyway.
           *
           * Costs nothing in the common case (no callback, so no second resize) and one
           * resize in the case that would otherwise have been permanently wrong.
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

      // A session that was already dead when this pane attached will never produce an event
      // — the watcher fired before anyone was listening. One check, not a poll: this is the
      // rehydration case (a host evicted and re-created after its child had gone), and it is
      // answered once at attach time rather than every second for the life of the pane.
      //
      // The answer carries the code now. It used to be a bool, so this path printed a bare
      // `— exited —` over a status the registry was still holding — the same pane, the same
      // dead child, a different sentence depending only on whether it happened to be mounted.
      const mark = await exitMark(id)
      if (mark) noteExit(mark.code)
    }

    /**
     * Start, and recover once from the one failure that is recoverable.
     *
     * `— no such session —` was what a user saw, printed into an otherwise blank pane, and it
     * is the registry correctly refusing to answer for an id it has never held — which is not
     * something the person reading it can do anything about. It happens when this pane adopted
     * a `SessionId` out of `workspace.json` whose owning process died with the previous run.
     * The adoption itself is now guarded (see below), so this is the second line of defence
     * rather than the first: forget the id and start a session instead of reporting a refusal.
     *
     * Once, and only for `noSuchSession`. A retry loop on a spawn that genuinely cannot work —
     * no such program, a conversation already open in another pane — would spin, and those two
     * failures are ones the user *must* read.
     */
    const startOrRecover = async (spawnSpec: TerminalSpec, retried: boolean): Promise<void> => {
      try {
        await start(spawnSpec)
      } catch (error: unknown) {
        if (disposed) return
        if (!retried && isRecoverableSessionError(error)) {
          void diag
            .log(`pane ${paneId}: the session it was holding is gone; starting one instead`)
            .catch(() => {})
          forgetSession(paneId)
          writeFailures.delete(paneId)
          await startOrRecover(spawnSpec, true)
          return
        }
        // Into the pane, not only the console. A pane whose spawn failed is blank, and blank is
        // also what "still connecting" and "the renderer died" look like — so the reason has to
        // land where the user is already looking. `SessionError::AlreadyOpen` was invisible for
        // exactly this reason: refused for a good cause, said so precisely, unreadable.
        console.error('[cide] terminal pane failed to start', error)
        void diag.log(`pane ${paneId} failed to start: ${spawnFailureText(error)}`).catch(() => {})
        term.write(spawnFailureBytes(spawnFailureText(error)))
      }
    }

    /**
     * Wait until the registry agrees this child is gone.
     *
     * Only for a resume, and it is not optional there: `session_spawn` refuses to adopt an id
     * whose entry is still `running` (`SessionError::AlreadyOpen`, which exists so two panes
     * cannot resume one conversation), and `has_exited()` flips on EOF from a *different thread*
     * than the one this kill returned on. Restarting a live pane would otherwise race and fail
     * with a refusal that reads like a bug. A fresh restart mints a new id and needs none of
     * this.
     *
     * Bounded, because a child that ignores the whole signal ladder is a real possibility and
     * hanging the button for ever is worse than a spawn that reports why it was refused.
     */
    const settled = async (id: string): Promise<void> => {
      const deadline = Date.now() + 3000
      while (Date.now() < deadline) {
        if (await sessionIsLive(id).then((live) => !live)) return
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
    }

    /**
     * Kill what this pane is running and start again — the respawn path `claude.restart` was
     * marked `unavailable` for the want of.
     *
     * `fresh` mints a new session; `resume` hands the old id back to `claude --resume`, which
     * keeps it, so the pane's binding and the project's primary session do not move. The
     * terminal is never torn down: the transcript stays on screen and the new child prints
     * below it, exactly as re-running a command in a shell does.
     */
    const restart = async (mode: RestartMode): Promise<void> => {
      const old = getHost(paneId).sessionId
      if (old !== undefined) {
        await sessionApi.kill(old).catch((error: unknown) => {
          // Not fatal. A child that is already dead is the ordinary case here, and the spawn
          // below is what the user asked for either way.
          void diag.log(`pane ${paneId}: kill before restart failed — ${String(error)}`).catch(() => {})
        })
        if (mode === 'resume') await settled(old)
        // The sink goes with the session it was registered against. Without this the pane's
        // attachment record would name a session it no longer shows, and the detach in this
        // effect's cleanup — which reads the host's *current* id — would never take it off.
        void paneSession.detach(paneId, old)
      }
      if (disposed) return

      forgetSession(paneId)
      writeFailures.delete(paneId)

      await startOrRecover(
        { ...spec, resume: mode === 'resume' ? old : undefined, fork: false },
        false,
      )
    }
    restartRef.current = restart
    const unregisterRestarter = registerRestarter(paneId, {
      restart,
      canResume: async () => {
        const id = getHost(paneId).sessionId
        if (runKind !== 'claude' || id === undefined) return false
        return claudeSession.resumable(cwdRef.current, id).catch(() => false)
      },
    })

    void (async () => {
      /*
       * The domain may already hold a session for this pane — one detached and re-docked, one
       * re-mounting after a split, or one restored from `workspace.json`. Adopt it **only if
       * the registry still holds a running child for it**.
       *
       * The liveness check used to be skipped whenever the pane had no entry in the launch
       * plan, on the reasoning that a plan entry is the only thing that says `pane.session`
       * comes from a previous process. That reasoning has two holes, and both of them printed
       * `— no such session —` into a blank pane:
       *
       * * a detached-pane window never received a plan at all — `App.tsx` computed one, fetched
       *   it, and rendered `DetachedPaneWindow` on a branch that did not pass it — so every
       *   torn-out pane came back at launch, adopted its dead id and failed, every time; and
       * * the plan is fetched in one effect and the workspace hydrated in another, so a pane
       *   painted before the plan resolved read `restore === undefined` and did the same thing.
       *
       * Asking the registry costs one round trip on a re-dock and closes both, whatever the
       * plan says and whenever it arrives. `restore` keeps the one thing only it knows: whether
       * to pass `--resume`.
       */
      if (domainSession && !getHost(paneId).sessionId) {
        if (await sessionIsLive(domainSession)) getHost(paneId).sessionId = domainSession
        if (disposed) return
      }
      await startOrRecover(spec, false)
    })()

    const onData = term.onData((data) => {
      const id = getHost(paneId).sessionId
      if (!id) return
      // No `acknowledge` here, and that is a fix rather than an omission — see `onKeyDown`
      // below, which is where it moved to.
      //
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
      // also opening one. (The pane's own menu IS among them now, and this line is the only
      // thing holding it off: `PaneFrame` moved `onContextMenu` from the deleted 26px title
      // bar onto the whole frame, which the terminal very much is inside.)
      ev.stopPropagation()
      const target = ev.target instanceof HTMLElement ? ev.target : null
      openMenu.current(ev.clientX, ev.clientY, target)
    }
    hostEl.addEventListener('contextmenu', onMenu)

    /*
     * "The user has seen this session" — on a *keystroke*, not on terminal output.
     *
     * This used to hang off `term.onData`, which is wrong in a way that only shows up in the
     * one situation the whole feature exists for. `onData` is xterm's outbound stream, and the
     * terminal answers queries on it **by itself**: a Device Attributes report, a cursor
     * position report, a focus in/out report, a bracketed-paste wrapper. Claude Code probes the
     * terminal at the end of a turn, so the sequence was reliably: the turn finishes, the
     * marker goes up, the CLI asks the terminal something, xterm replies on `onData`, and the
     * marker clears itself half a second later with nobody at the keyboard. Then the user comes
     * back to a task bar that says nothing is waiting.
     *
     * A `keydown` on the pane host cannot be forged by the child: it is the user pressing a
     * key inside this pane, which is the strongest statement available that they are here and
     * looking. Bare modifiers are excluded — holding Ctrl to read a chord, or Alt to reach a
     * menu, is not reading a conversation, and on some layouts a modifier is pressed on the way
     * to somewhere else entirely.
     *
     * Deliberately *not* routed through the key gate. The gate decides what a chord *does*; the
     * question here is only whether a human touched this pane, and a keystroke the gate swallows
     * for a global binding is still a human touching this pane.
     */
    const onKeyDown = (ev: KeyboardEvent) => {
      if (ev.key === 'Shift' || ev.key === 'Control' || ev.key === 'Alt' || ev.key === 'Meta') {
        return
      }
      const id = getHost(paneId).sessionId
      if (id) acknowledge(id)
    }
    hostEl.addEventListener('keydown', onKeyDown)

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
        // Not `id` from the async block above: this handler outlives it, and a pane that
        // respawned holds a different session by now.
        if (session !== getHost(paneId).sessionId) return
        /*
         * Mid-turn, which is the one state that must keep this host out of the eviction sweep.
         *
         * `setHostBusy` had no caller anywhere: `PaneHost.busy` was documented as being set from
         * this very event and nobody was setting it, so it was permanently `false` and the pane
         * with a turn in flight was as evictable as an idle one. Rehydrating from the screen
         * mirror is lossless for a settled pane and is not for one whose bytes are arriving now.
         */
        setHostBusy(paneId, state.state === 'busy')
        if (state.state !== 'exited') return
        // `state.code` is the whole reason the Rust side threads the status out of `wait()`.
        // Dropping it here was the last link in the chain, and it made the change invisible.
        noteExit(state.code)
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
      unregisterRestarter()
      restartRef.current = null
      // The provider stays on the host — it belongs to the terminal, not to this mount — so
      // what has to go is the environment it reads. Without this, a pane unmounted from a
      // closing project would keep resolving paths against that project's roots and opening
      // tabs in it.
      setPathLinkEnv(paneId, null)
      // The host outlives this mount, so the listeners have to come off with it — otherwise a
      // pane remounted by a split would accumulate one right-click handler per mount and open
      // as many menus.
      hostEl.removeEventListener('contextmenu', onMenu)
      hostEl.removeEventListener('keydown', onKeyDown)
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

  /*
   * What this pane offers now that its child has gone. `null` while it is alive, which is
   * every pane in ordinary use.
   *
   * The decision is `restartRule.ts`'s and not this component's, deliberately: it is the rule
   * — what is offered, when, and in which words — and a rule inside a render is a rule no check
   * script can compile. `check:restart` runs every branch of it.
   */
  const offer = restartOffer({
    kind: runKind,
    exited: exit !== null,
    code: exit?.code,
    resumable,
  })

  return (
    <>
      <PaneSlot paneId={paneId} className={className} onResize={() => void syncSize(paneId)} />
      {/*
        * Both bottom bars, in one absolutely-positioned stack that is a *sibling* of the slot.
        *
        * Every half of that matters. Rendering either bar above the terminal **in flow** is what
        * the restored-shell banner did: `PaneSlot` still claims `height: 100%`, so the terminal
        * was pushed its own height past the bottom of the pane frame and painted over the row
        * below. And the slot itself is never conditionally rendered — rule 2 of
        * `layout/paneHosts.ts` — so the terminal, its scrollback and the `— exited —` line the
        * user is reading all stay exactly where they were while a control sits over the last two
        * lines.
        *
        * One stack rather than two `bottom: 0` boxes, because both states can be true at once: a
        * child that has exited leaves a transcript, and searching a dead pane's transcript is
        * exactly the moment somebody reaches for Ctrl+F. Stacked in this order the restart bar
        * keeps the bottom edge it has always had and the find bar sits above it, so neither
        * moves the other's pixels and neither is covered. `TerminalFindBar.module.css` carries
        * the argument for why this is at the bottom of the pane at all — the top-right is the
        * floating control cluster's, and the editor already paid for finding that out.
        */}
      <div className={findStyles.stack ?? ''}>
        <TerminalFindBar paneId={paneId} />
        {offer !== null && <ExitedBar offer={offer} onRun={runRestart} />}
      </div>
      {/* Portals out of here entirely; it is in the tree so React owns its lifetime. */}
      {menu}
    </>
  )
}

/*
 * Styles inline rather than in a CSS module, on `ResumeSplash`'s own argument: hover is the only
 * state and a stylesheet for one bar and two buttons buys nothing. If this grows, it wants a
 * module.
 */
const barStyle: CSSProperties = {
  /*
   * In flow inside the bottom stack, which is the box that is absolutely positioned.
   *
   * This used to be `position: absolute; left/right/bottom: 0; z-index: 1` itself, and the
   * reasons for that are unchanged and are now the stack's — see the JSX above and
   * `TerminalFindBar.module.css`. What moved is only *which* box claims the pane's bottom edge,
   * and it had to move the moment a second bar wanted the same edge: two children both at
   * `bottom: 0` do not stack, they overlap, and the one drawn second wins silently.
   */
  display: 'flex',
  alignItems: 'center',
  gap: 10,
  padding: '6px 10px',
  background: 'var(--panel-2)',
  borderTop: '1px solid var(--border)',
  fontFamily: 'var(--font-mono)',
  fontSize: 12,
  lineHeight: '19px',
  color: 'var(--dim)',
}

const statusStyle: CSSProperties = {
  flex: 1,
  minWidth: 0,
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  whiteSpace: 'nowrap',
}

function actionStyle(accent: boolean): CSSProperties {
  return {
    padding: '3px 10px',
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: accent ? 'var(--accent)' : 'var(--border)',
    background: 'transparent',
    color: accent ? 'var(--text-hi)' : 'var(--text)',
    font: 'inherit',
    cursor: 'pointer',
    flex: 'none',
  }
}

/**
 * The way out of a pane whose child has died.
 *
 * Drawn in the pane rather than only in the command palette, and that is the whole point: the
 * user is looking at a dead terminal, and an affordance they have to already know the name of
 * is the unreachable feature wearing a different hat. The palette rows exist too
 * (`claude.restart`, `claude.resume`), for the user who prefers the keyboard.
 *
 * It repeats the exit status rather than relying on the `— exited (n) —` line it may be sitting
 * over, so the one thing a user needs off that transcript is never the thing this covers.
 */
function ExitedBar({
  offer,
  onRun,
}: {
  offer: RestartOffer
  onRun: (mode: RestartMode) => void
}) {
  // Read out before the JSX: TypeScript drops a narrowing of `offer.secondary` inside the
  // click handler's closure, and a non-null assertion there would be the kind of thing that
  // survives a refactor that makes it false.
  const secondary = offer.secondary
  return (
    <div style={barStyle} data-audit="exitedPane">
      <span style={statusStyle}>{offer.status}</span>
      {secondary !== null && (
        <button type="button" style={actionStyle(false)} onClick={() => onRun(secondary.mode)}>
          {secondary.label}
        </button>
      )}
      <button type="button" style={actionStyle(true)} onClick={() => onRun(offer.primary.mode)}>
        {offer.primary.label}
      </button>
    </div>
  )
}
