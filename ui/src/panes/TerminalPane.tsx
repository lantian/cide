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
import { forgetSession, getHost, openTerminal, peekHost } from '@/layout/paneHosts'
import { setPathLinkEnv } from '@/terminal/pathLinks'
import { copyTerminalSelection, pasteIntoTerminal } from '@/terminal/clipboard'
import type { TerminalPaneKind } from '@/terminal/keys'
import { takeSpawnPlan } from '@/layout/spawnPlans'
import { useContextMenu, type MenuEntry } from '@/menus'
import { isRecoverableSessionError, spawnFailureBytes, spawnFailureText } from './exitMarker'
import {
  clearWriteFailure,
  ensureAttached,
  measureGeometry,
  onSinkExit,
  reportWriteFailure,
  sinkExit,
  syncSize,
} from './sessionSink'
import { restartOffer, type RestartMode, type RestartOffer } from './restartRule'
import { TerminalFindBar } from './TerminalFindBar'
import { openTerminalFind } from '@/terminal/findStore'
import findStyles from './TerminalFindBar.module.css'
import { registerRestarter } from './paneRestart'
import { acknowledge } from './awaiting'
import { acknowledgesKey } from './awaitingRule'
import {
  claudeSession,
  diag,
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
 * What a shell pane asks for as its program: nothing, meaning *the user's login shell*.
 *
 * A webview cannot read `$SHELL`, and this file used to answer that by naming `/bin/bash`
 * outright — which on macOS is a shell nobody configures (see `cide_core::shell` for the whole
 * report), so the pane ran without the user's `~/.zshrc` and therefore without nvm or
 * Homebrew on its `PATH`. The decision belongs where the fork is; `session_spawn` reads an
 * empty program as this request and supplies the login flag with it.
 */
const LOGIN_SHELL = ''

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
      return { program: LOGIN_SHELL, args: [], cwd, project, resume: prior }
    }
    default:
      return null
  }
}

/*
 * `syncSize`, `measureGeometry`, the write-failure report, the `— exited —` marker and the
 * whole attach path all live in `sessionSink.ts` now — the sink they serve belongs to the
 * pane *host* rather than to this mount, which is what keeps a parked pane's buffer filling
 * across a project switch. This component keeps only what genuinely belongs to a mount:
 * spawn-or-adopt, restart wiring, menus, listeners on the host element, and the two bars.
 */

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
 * Whether the registry still holds this session at all — running, **or exited with its screen
 * retained**, which `report_exit` keeps precisely so a pane can still paint the last thing the
 * child printed.
 *
 * The adoption gate below used to ask [`sessionIsLive`], and the gap was reported before it was
 * reasoned about: a mirror pane's spawn plan lives in the *clicking* window's JS realm, so in a
 * multi-window layout the pane can render in a window that never heard of the plan and fall
 * through to this gate — where a **running** run's session was adopted (Open on a live agent
 * worked, by accident of this very fallback) and a **finished** run's session was refused, so
 * the pane spawned a fresh `claude` over the transcript the user clicked Open to read. An
 * opencode run's rendered event log, replaced by a claude splash.
 *
 * `exited` therefore adopts too: the attach paints the retained screen plus the `— exited —`
 * marker, and closing the pane later `kill`s a process that is already gone, which is a no-op —
 * so the missing `mirrored` flag costs nothing on this arm. `reaping` and `unknown` still
 * refuse: the first is a corpse mid-reap that becomes `exited` moments later (the plan-carrying
 * window never hits this race), and the second is an id from a previous process, which is the
 * restore path's business.
 */
async function sessionIsHeld(session: string): Promise<boolean> {
  return sessionApi.exit(session).then(
    (answer) => answer.kind === 'running' || answer.kind === 'exited',
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
    const { term } = handle

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
      //
      // `mirrored` is set beside the id and means "this pane did not spawn what it holds".
      // `closePane` is the one reader: without it, closing this pane kills the child in the
      // pane being mirrored, because the two panes are indistinguishable from the host map.
      // Set under the same guard as the id, so StrictMode's second mount — which finds the
      // plan already taken and the id already there — cannot disagree with the first.
      if (!getHost(paneId).sessionId) {
        getHost(paneId).sessionId = plan.session
        getHost(paneId).mirrored = true
      }
    } else if (plan?.kind === 'resume') {
      /*
       * A **new child** continuing a conversation whose old one is gone. (M28)
       *
       * Deliberately not the mirror branch above it, and the difference is the ownership flag,
       * not the flag's spelling: this pane spawns the process, so it owns it and closing the
       * pane must end it. `mirrored` is therefore left unset — the mistake in the other
       * direction killed an agent mid-turn once, and it is recorded at length beside
       * `agent_open_pane` in `cmd/agents.rs`.
       *
       * And not `forkPrimary`, whose whole point is that it mints a *different* id so the two
       * histories diverge. A `SessionId` is the value cide passes to `claude --session-id`, so
       * keeping it is what makes this the same conversation — and what keeps every record that
       * names it, a task's `session` field above all, naming the right one.
       *
       * `session_spawn` is told the id through `resume`; the pane's own `session` already holds
       * it, put there by `pane_for`'s `Resume` arm, so the two agree by construction.
       */
      spec.resume = plan.session
    } else if (plan?.kind === 'forkPrimary' && primaryRef.current) {
      spec.resume = primaryRef.current
      spec.fork = true
    }

    /**
     * This pane's child has gone: offer a way back.
     *
     * The *marker* is `sessionSink.ts`'s — it is written into the terminal, which outlives
     * this mount, and an exit heard while the pane was parked writes it into the parked
     * buffer so it is already on screen when the user returns. What belongs here is only
     * the render state: the bar with the way back, which `onSinkExit` replays immediately
     * when an exit was recorded before this mount, so a pane remounted by a split — or
     * returning from the project the child died under — comes back offering it.
     */
    const offExit = onSinkExit(paneId, (code) => {
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
    })

    /**
     * Attach this pane to a session, spawning one if it does not already hold one.
     *
     * As a function rather than the body of the mount effect, because a restart is exactly
     * this sequence run again against the same terminal. Doing it by unmounting and
     * remounting the pane was the alternative and it is forbidden: `layout/paneHosts.ts`
     * rule 2, and it would throw away the transcript the user is reading the exit code off.
     *
     * The attach itself — the sink, the snapshot, the hydration decision, the resize nudge
     * — is `ensureAttached`'s, and it is idempotent per (pane, session): a pane remounting
     * after a park finds its sink still attached and its buffer already current, so this
     * resolves without touching the terminal. Only a pane whose host holds no link — fresh,
     * released, evicted, restarted — actually attaches.
     */
    const start = async (spawnSpec: TerminalSpec): Promise<void> => {
      const geo = measureGeometry(paneId)
      const id = await sessionFor(paneId, spawnSpec, geo)
      if (disposed) return

      // Tell the domain which session this pane holds, once. The binding is what survives a
      // restart: the id is the value passed to `claude --session-id`.
      if (boundRef.current !== id) {
        boundRef.current = id
        boundCb.current?.(id)
      }

      await ensureAttached(paneId, id)
      if (disposed) return

      /*
       * The pane is attached and painting, so any offer over it is spent — unless the sink
       * already knows this child is dead, which is a pane coming back from a park (or a
       * rehydration) the child did not survive. Clearing there would blank the bar the
       * `onSinkExit` replay above just drew, with nothing on the way to re-draw it.
       *
       * Cleared *here* rather than when the restart button was pressed, and that is the
       * difference between a control and a trapdoor: a restart whose spawn fails would
       * otherwise take the only way out of the pane away with it, leaving the same dead end
       * the button exists to fix — this time with a `— could not start —` line instead of
       * `— exited —`. React bails out of a re-render when the state is already `null`, so
       * this costs a live pane nothing.
       */
      if (sinkExit(paneId) === null) {
        setExit(null)
        setResumable(false)
      }

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
          clearWriteFailure(paneId)
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
      }
      if (disposed) return

      // `forgetSession` also closes the sink (`PaneHost.sinkClose`), whose closure captured
      // `old` at attach — which is what guarantees the detach names the session the sink was
      // registered against, however the ids have moved since.
      forgetSession(paneId)
      clearWriteFailure(paneId)

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
       * re-mounting after a split, one restored from `workspace.json`, or a mirror whose spawn
       * plan lives in another window's realm. Adopt it **only if the registry still holds it**
       * — a running child, or an exited one whose screen `report_exit` retained.
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
        // `sessionIsHeld`, not `sessionIsLive`: an exited session the registry retains is a
        // transcript this pane exists to show — a finished run opened from History, a pane
        // whose child ended while it was parked. Spawning over it replaced an opencode run's
        // rendered log with a fresh claude; the function's own doc carries the report.
        if (await sessionIsHeld(domainSession)) getHost(paneId).sessionId = domainSession
        if (disposed) return
      }
      await startOrRecover(spec, false)
    })()

    /*
     * Keystrokes out — `term.onData` → `session.write` — are registered by `ensureAttached`
     * for the *link's* lifetime, not here for the mount's: a parked pane keeps its keyboard
     * wiring with its sink, and the two turn over together on a restart. The `keydown`
     * acknowledge below stays here deliberately — see its comment; `check:awaiting` pins
     * both halves of that split.
     */

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
     * "The user has seen this session" — on a *keystroke* or a *scroll*, not on terminal output.
     *
     * This used to hang off `term.onData`, which is wrong in a way that only shows up in the
     * one situation the whole feature exists for. `onData` is xterm's outbound stream, and the
     * terminal answers queries on it **by itself**: a Device Attributes report, a cursor
     * position report, a focus in/out report, a bracketed-paste wrapper. Claude Code probes the
     * terminal at the end of a turn, so the sequence was reliably: the turn finishes, the
     * marker goes up, the CLI asks the terminal something, xterm replies on `onData`, and the
     * marker clears itself half a second later with nobody at the keyboard. Then the user comes
     * back to a task bar that says nothing is waiting. `sessionSink.ts` carries the other half
     * of that, where the keystrokes actually leave for the pty.
     *
     * Neither of these can be forged by the child: they are a person pressing a key or turning
     * a wheel inside this pane, which is the strongest statement available that they are here
     * and looking. `acknowledgesKey` is the rule about which strokes count.
     *
     * # Capture, and it is the whole of the reported bug
     *
     * Both listeners are registered in the **capture** phase, and a bubble listener here is
     * silently dead rather than merely late. xterm's own handlers live on `term.textarea`,
     * which is a descendant of this element (`paneHosts.ts` gives `host.el` to `term.open()`),
     * and `_keyDown` ends every key it consumes with `cancel(ev, true)` — which is
     * `preventDefault()` **and `stopPropagation()`**. So a bubble listener never sees a
     * printable character, `Enter`, or an arrow key: it is alive for exactly the keys that mean
     * nothing and dead for every key that means a person is here. This shipped, and the report
     * was that the marker could only be cleared by clicking a pane the user was already typing
     * in — `PaneTitleBar`'s pointer-down and focus handlers are React *capture* handlers, which
     * is the only reason those two worked. `terminal/inputHost.ts` states the ordering rule at
     * length and relies on it for the IME guard; this is the same rule, obeyed here too.
     *
     * The removals must carry the same flag: `removeEventListener` matches on the capture flag,
     * and the host outlives this mount, so a mismatched removal leaks one acknowledger per
     * mount onto an element that keeps the whole terminal reachable.
     *
     * # What does *not* acknowledge, and why the previous version of this comment was wrong
     *
     * A chord the key gate consumes — `ctrl+shift+p` and everything else in `keymap.json`.
     * This used to claim that "a keystroke the gate swallows for a global binding is still a
     * human touching this pane", which was a statement about intent that the code could not
     * carry out: `keys/gate.ts`'s entry 2 is a capture listener on `window`, above every pane,
     * and it calls `stopPropagation()` there — so the stroke never reaches this element in
     * either phase and no listener here could ever have seen it. Opening the palette is not
     * reading a conversation, so the boundary is left where the mechanism puts it.
     */
    const onKeyDown = (ev: KeyboardEvent) => {
      if (!acknowledgesKey(ev.key)) return
      const id = getHost(paneId).sessionId
      if (id) acknowledge(id)
    }
    hostEl.addEventListener('keydown', onKeyDown, true)

    // Scrolling this pane's scrollback is reading it — the one act that is unambiguously
    // "I am looking at this output" and involves no keyboard at all, which is how a finished
    // `make` is usually read. Passive: nothing here calls `preventDefault`, and a non-passive
    // wheel listener on a scrolling surface makes the browser wait for this handler before it
    // may scroll. Removed with a bare `true` — removal matches on the capture flag alone.
    const onWheel = () => {
      const id = getHost(paneId).sessionId
      if (id) acknowledge(id)
    }
    hostEl.addEventListener('wheel', onWheel, { capture: true, passive: true })

    // Exit, busy and the eviction-protecting `setHostBusy` all arrive through
    // `sessionSink.ts`'s single module-level `cide://session-state` dispatch now, which is
    // what keeps them true for a *parked* pane too: the per-mount subscription that used to
    // live here died with the mount, so a pane parked mid-turn froze at whatever `busy` said
    // last, and an exit during a project switch was only discovered by the next attach's
    // one-shot check. The component's share is the `onSinkExit` subscription above — the
    // render state, which is the only part of an exit that belongs to a mount.

    return () => {
      disposed = true
      offExit()
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
      hostEl.removeEventListener('keydown', onKeyDown, true)
      hostEl.removeEventListener('wheel', onWheel, true)
      // Deliberately no `paneSession.detach` here — the sink belongs to the host now
      // (`sessionSink.ts`), and an unmount is usually a *park*: a project switch, a split,
      // a re-dock in transit. Detaching with the mount is exactly the frozen-pane bug this
      // arrangement replaced — output printed while the pane's project was in the
      // background reached only the Rust mirror, and the recovery snapshot was refused by
      // the hydration gate on the way back. The moments the sink really must go are all
      // host-side and all covered: `releaseHost`, `teardown` (close and eviction) and
      // `forgetSession` each call `PaneHost.sinkClose`.
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
