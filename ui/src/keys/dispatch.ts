/**
 * Command dispatch — what actually happens when the gate resolves a chord, or the palette
 * runs a row.
 *
 * One dispatcher for both, deliberately. A key and a palette entry that name the same
 * command must do the same thing; two switch statements is how they stop doing so, and the
 * palette is where the difference would be least visible because a user who runs a command
 * from a list rarely also has the key memorised.
 *
 * # Why this file is now the whole registry
 *
 * It used to own seven commands and forward the rest to `deps.fallback`, which is `App.tsx`'s
 * switch. `App.tsx` handled six more and logged a line for everything else — so of the 37
 * commands `cide_core::commands` declared, **24 resolved to a diagnostic**: every
 * `pane.navigate.*`, both tab-cycling commands, `tab.close`, `pane.close`, `pane.maximize`,
 * `theme.toggle`, `settings.open`, all four git commands. The keymap bound nine of them, so
 * the gate resolved the chord, swallowed the keystroke and called a dispatcher with no arm
 * for it — `ctrl+w` among them, which is how this was reported — and the palette listed all
 * 24 as rows you could pick.
 *
 * The reason the fallback existed was that the split family needs a project, a tab and a
 * pane, and those were believed to live only in `App.tsx`'s render. They do not: the
 * workspace mirror is a module-level zustand store, so `useWorkspace.getState()` reaches the
 * same facts from outside React — which is exactly what `file.save` already did here. So
 * every command is handled here, `deps.fallback` is left for ids nothing declares (a typo in
 * a user's `keymap.json`), and a window that passes nothing but `fallback` still gets all of
 * them working.
 *
 * # Two ways a command can decline
 *
 * * **`unavailable`** — the capability does not exist in this build. The reason travels in
 *   the registry (`Command::unavailable`), the palette draws the row greyed with it, and no
 *   key may be bound to one. Reached from here only if something runs it anyway.
 * * **`when`** — the precondition is not met right now. The palette hides the row, and the
 *   handler re-checks the same fact and reports it.
 *
 *   Both halves are needed, because **a `Command::when` does not gate the keyboard**. The
 *   gate resolves through `keys/keymap.ts`, which evaluates `Binding::when` — a different
 *   field, `None` on every default binding — so Ctrl+W arrives at `tab.close` whatever the
 *   registry's clause says. Treating the clause as a guard is how Ctrl+W on the pinned
 *   console came to raise a "close anyway?" dialog and then fail with `TabPinned`. So a
 *   handler whose clause names a precondition checks it here as well, through the same
 *   function `keys/context.ts` derives the flag from — `keys/target.ts` — so the two cannot
 *   drift into two answers.
 *
 * Nothing here takes its state from a prop. Each store is read with `getState()` at the
 * moment the command runs, because the key gate is a window listener living outside React —
 * a dispatcher closed over render output would act on whatever the last render saw.
 */
import { useOverlays } from '@/overlays/store'
import { useFileTree } from '@/sidebar/treeStore'
import { EXTERNAL_LIBRARIES, SCRATCHES } from '@/sidebar/groupRows'
import { useGitStatus } from '@/sidebar/gitStatusStore'
import { useWorkspace } from '@/store/workspace'
import { toggleTheme } from '@/settings/useSettings'
import { paneSessionId, peekHost } from '@/layout/paneHosts'
import { openBranchPopup } from '@/chrome/BranchSelector'
import { explain, pullReport, type RepoFetch } from '@/chrome/branchModel'
import { notify, notifyFailure } from '@/chrome/notices'
import { pasteIntoTerminal } from '@/terminal/clipboard'
import { paneRestarter } from '@/panes/paneRestart'
import { useGitCount } from '@/chrome/gitCountStore'
import { requestFocus } from '@/chrome/focusRequests'
import {
  branch as branchApi,
  claudeSend,
  diag,
  git as gitApi,
  settings as settingsApi,
  tab as tabApi,
  type Axis,
  type Direction,
  type PaneId,
  type ProjectId,
  type RepoInfo,
  type SplitIntent,
  type TabId,
} from '@/ipc/client'
import { focusedCaret, focusedWord } from '@/editor/caretTrack'
import { goToDefinition } from '@/editor/goToDefinition'
import { findUsages } from '@/editor/codeIntel'
import { navigate } from '@/editor/jump'
import { memberStep } from '@/editor/memberNav'
import { symbolsOf } from '@/editor/outlineStore'
import { requestReveal } from '@/editor/revealRequest'
import { focusedTabOf, registeredBuffers, saveAll, saveTab } from '@/editor/openBuffers'
import { revealPane } from '@/editor/revealPane'
import { startProjectSwitch, startTabSwitch } from './switcherStore'
import {
  activeProjectOf,
  claudeTargetOf,
  consolePaneOf,
  focusTarget,
  focusedFilePath,
  focusedTabPath,
  isClosableTab,
  windowProjectsOf,
} from './target'

export interface DispatchDeps {
  /**
   * Commands nothing in the registry declares.
   *
   * Once this handled all 42 there is nothing left for a host to own, so this is reached
   * only by an id that is not a command: a typo in a user's `keymap.json`, or a stale
   * binding naming something that has been deleted. `reportOnly` is a fine value for it.
   */
  fallback: (command: string, args: unknown) => void
  /**
   * Switch the activity rail's sidebar view, for the `sidebar.*` commands and for anything
   * that has to reveal a panel before acting on it (`file.reveal`, `git.commit`).
   *
   * Optional because a detached-pane window has no rail and no sidebar to switch. Every arm
   * that uses it checks for `undefined` and reports rather than assuming.
   */
  showSidebar?: ((view: 'files' | 'git' | 'search' | 'problems') => void) | undefined
  /**
   * Hide the left panel, or bring back the last one that was open. F4, and the palette row.
   *
   * Separate from [`showSidebar`] rather than an extra value it accepts, because the two are
   * different questions: `showSidebar` names a panel and always reveals it, and this one names
   * none and depends on what was showing. Which panel comes back is `chrome/sidebarView.ts`'s
   * arithmetic, not this module's — that is a rule with cases in it, and a rule that lives in a
   * React state updater is a rule no check script can compile.
   *
   * Optional for the same reason as `showSidebar`: a detached-pane window has no rail and no
   * sidebar to toggle.
   */
  toggleSidebar?: (() => void) | undefined
  /**
   * Override for which tab `file.save` writes.
   *
   * Nothing needs to supply it: the default reads the same workspace mirror the app renders
   * from, so `file.save` works in a window that passes only a `fallback`. It stays on the
   * interface so a host that *does* pass it keeps typechecking, and passing
   * `() => focused?.tab.id ?? null` is the same answer.
   *
   * It is not the place to add "save the pane the caret is in". A File tab has exactly one
   * editor by construction — `PaneBody` dispatches on the pane kind for that reason — so the
   * tab is the whole address of a buffer.
   */
  focusedTab?: (() => string | null) | undefined
}

/** The live mirror. One call per command, so every handler sees one consistent snapshot. */
function boot() {
  return useWorkspace.getState().boot
}

/**
 * The focused tab of the live workspace mirror.
 *
 * The arithmetic is in [`focusedTabOf`], in `editor/openBuffers.ts`, because that module can
 * be compiled and driven from fixtures and this one cannot. Passing `boot` — a `Bootstrap` —
 * to a parameter typed structurally is what keeps that mirror honest: a renamed field or a
 * new `WindowRole` variant fails to typecheck here.
 */
function focusedTabOfWorkspace(): string | null {
  return focusedTabOf(boot())
}

/**
 * Report a command that ran with its precondition unmet.
 *
 * Not silence. Most of these have a `when` clause that should have kept the palette from
 * offering the row and the gate from firing the binding, so reaching one means the clause and
 * this file disagree about what the command needs — and returning quietly makes that
 * indistinguishable from the unwired command it used to be. The rest (`claude.mirror` with no
 * session yet) are conditions no flag in the vocabulary expresses, and they are worth a line
 * for exactly the same reason.
 */
function unmet(command: string, precondition: string): void {
  void diag.log(`[cide] ${command} did nothing: ${precondition}`)
}

/** The registry entry's `unavailable` reason, or `null`. */
function unavailableReason(command: string): string | null {
  return boot()?.commands.find((entry) => entry.id === command)?.unavailable ?? null
}

/**
 * Run something over every repository in the active project, asking Rust which those are.
 *
 * The question is answered by `git_repos` — real discovery, on the disk — and not by the
 * workspace mirror, because the mirror cannot answer it. `keys/target.ts::reposOf` used to,
 * from `ProjectRoot.repo`, a field Rust set to `None` in its one constructor and never filled
 * anywhere; it therefore returned `[]` for every project ever opened, and the three handlers
 * that called it bailed at their first line every single time. That is the *second* half of
 * the bug — the palette hid these rows, and a user who reached them another way (a
 * hand-written `keymap.json`) got a diagnostic log line and nothing else.
 *
 * An empty answer **throws** rather than calling `unmet`. `unmet` writes to the diag log, which
 * is the right surface for "this command's clause and this handler disagree" and the wrong one
 * for "your project has no git repository": that is a fact about the user's disk, they asked a
 * question, and they are owed a sentence. The throw is inside a promise chain nothing catches,
 * so it lands on `chrome/Failures.tsx` through `unhandledrejection` like every other reported
 * failure. Silence here is precisely the defect this file keeps being rewritten for.
 */
function withRepos(command: string, run: (project: ProjectId, repos: RepoInfo[]) => void): void {
  const project = activeProjectOf(boot())
  if (project === null) return unmet(command, 'no open project')
  void gitApi.repos(project.id).then((repos) => {
    if (repos.length === 0) {
      throw new Error('There is no git repository in this project.')
    }
    // The whole `RepoInfo`, not just the id it used to hand over. A command that acts on
    // every repository has to be able to *name* them when it reports back — "already up to
    // date" four times over says nothing about which four.
    run(project.id, repos)
  })
}

/** `args.path` when the caller supplied one — `args` is `unknown` on the wire. */
function pathArg(args: unknown): string | null {
  if (typeof args !== 'object' || args === null) return null
  const path = (args as { path?: unknown }).path
  return typeof path === 'string' ? path : null
}

/**
 * Split the focused pane, or add a row below it.
 *
 * `axis: 'row'` adds a tile beside the focused pane; `addRow` appends a full-width row. The
 * two are different gestures in the rows model and the titles in `cide_core::commands` name
 * which is which — "Split pane right (adds a tile)" against "Split pane down (adds a row)".
 * `App.tsx`'s old fallback ran both through `pane_split`, so *Split pane down* subdivided the
 * current row instead; the title is the specification here, and `chrome/RowControls.tsx`'s
 * `+` button already makes the same call this does.
 */
function split(target: { project: ProjectId; tab: TabId; pane: PaneId }, axis: Axis, intent: SplitIntent | null): void {
  void useWorkspace.getState().splitPane(target.project, target.tab, target.pane, axis, 'after', intent)
}

/**
 * Build the dispatcher.
 *
 * Returns a plain function rather than a hook so the key gate — which runs outside React —
 * and the palette can share one instance. The stores it writes to are read with
 * `getState()` for the same reason.
 */
export function createDispatcher(deps: DispatchDeps): (command: string, args: unknown) => void {
  return (command, args) => {
    /*
     * The `unavailable` gate, before the switch and before anything else.
     *
     * These ids have no case below — that is the point of the field — and without this they
     * would fall through to `fallback` and be reported as unknown commands, which is a
     * different and wrong story. The reason comes from the registry rather than a copy here
     * so the palette's greyed row and this log line can never disagree.
     */
    const blocked = unavailableReason(command)
    if (blocked !== null) {
      void diag.log(`[cide] ${command} is not available in this build: ${blocked}`)
      return
    }

    const ws = useWorkspace.getState()
    const target = focusTarget(boot())
    /** Every `pane_*` command's arguments, or `null` when nothing is focused. */
    const on = target === null ? null : { project: target.project, tab: target.tab.id, pane: target.pane.id }

    switch (command) {
      /* ------------------------------------------------------------------ Window: panes */

      case 'pane.split.right':
        if (on === null) return unmet(command, 'no focused pane')
        return split(on, 'row', null)

      case 'pane.split.down':
        if (on === null) return unmet(command, 'no focused pane')
        // A row, not a column split — see `split` above for why the title decides this.
        return void ws.addRow(on.project, on.tab, on.pane, 'after', null)

      case 'pane.detachToWindow':
        if (on === null) return unmet(command, 'no focused pane')
        return void ws.detachPane(on.project, on.tab, on.pane)

      case 'pane.close':
        // Refusals — the console's primary pane, a tab's last pane, unsaved edits — come
        // back from Rust and the store turns the unsaved one into the close confirmation.
        if (on === null) return unmet(command, 'no focused pane')
        return void ws.closePane(on.project, on.tab, on.pane)

      case 'pane.maximize': {
        // A toggle, because the command is reached by one key and one palette row: a
        // maximize with no un-maximize leaves the user with a full-screen pane and no
        // gesture to undo it except finding the pane title bar's button.
        if (target === null || on === null) return unmet(command, 'no focused pane')
        const already = target.tab.tree.maximized === on.pane
        return void ws.maximizePane(on.project, on.tab, already ? null : on.pane)
      }

      case 'pane.navigate.left':
      case 'pane.navigate.right':
      case 'pane.navigate.up':
      case 'pane.navigate.down': {
        if (on === null) return unmet(command, 'no focused pane')
        // The suffix *is* the direction, and `Direction`'s variants are spelled to match.
        // A four-arm lookup table would be the same four strings written twice.
        const direction = command.slice('pane.navigate.'.length) as Direction
        return void ws.navigatePane(on.project, on.tab, on.pane, direction)
      }

      /* ----------------------------------------------------------------- Window: modes */

      case 'window.mode.stacked':
        return void ws.setWindowMode('stacked')

      case 'window.mode.perProject':
        return void ws.setWindowMode('perProject')

      /* ------------------------------------------------------------------ Window: tabs */

      case 'tab.close': {
        // Ctrl+W. The complaint that started this round: the binding resolved, the gate
        // swallowed the keystroke, and no arm existed. `closeTab` confirms first when the
        // tab holds unsaved edits or a live session.
        if (on === null) return unmet(command, 'no focused tab')
        /*
         * `closableTab` again, because the clause alone does not reach the keyboard.
         *
         * `tabs[0]` is the pinned console and `close_tab` answers `TabPinned`. The console is
         * also the tab a project *opens* on and it always has a Claude session bound, so
         * without this the most ordinary Ctrl+W in the app took the session branch of
         * `closeTab`, asked "close anyway?" about work it could never close, and then failed
         * on the confirmation. Hiding the palette row while the key ran into that was the
         * worst of the three states.
         */
        if (!isClosableTab(boot())) {
          return unmet(command, 'the project console is pinned and cannot be closed')
        }
        return void ws.closeTab(on.project, on.tab)
      }

      /*
       * Ctrl+Shift+T — put back the last tab this project closed, and the one before that, and
       * the one before that. It took the chord from `theme.toggle`, which is palette-only now.
       *
       * The stack is Rust's (`cide_app::closed_tabs`), so this arm is three decisions and no
       * policy: which project to pop for, what to do with nothing, and where the caret ends up.
       *
       * **Which project.** `activeProjectOf`, this window's — never a global pop. Ctrl+Shift+T
       * in a window showing project A must not resurrect a file from B, and in `perProject`
       * window mode every shell window is showing a different one.
       *
       * **Nothing to reopen** answers `null` rather than throwing, and `unmet` writes it to the
       * diagnostic log. `notifyFailure` would be wrong: an empty undo stack is a precondition,
       * the same shape as "no focused pane" three arms up, and a toast for it would fire on the
       * very first Ctrl+Shift+T of every session. The command deliberately carries no `when`
       * clause either — `cide_core::commands` argues that at length; the short version is that
       * a flag for it would need a supplier the snapshot has no business carrying.
       *
       * **Where the caret ends up.** `revealPane`, the way `tab.console` below does. Rust has
       * already made the tab active, but "active" and "typing in it" are two different things —
       * `TabContent` keeps every tab mounted and hides the inactive ones, so the keyboard is
       * still wherever it was. `hydrate` first, because the pane to reveal is read out of the
       * snapshot: the `cide://workspace-changed` broadcast would deliver the same tree a moment
       * later, and racing it would reveal a pane the mirror has not heard of yet.
       */
      case 'tab.reopenClosed': {
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        return void tabApi.reopenClosed(project.id).then(async (tab) => {
          if (tab === null) return unmet(command, 'nothing to reopen')
          await ws.hydrate()
          const reopened = boot()?.workspace.projects[project.id]?.tabs.find((t) => t.id === tab)
          if (reopened === undefined) return
          const refusal = await revealPane(project.id, reopened.tree.focused)
          if (refusal !== null) void diag.log(`[cide] ${command}: ${refusal}`)
        })
      }

      case 'tab.next':
      case 'tab.prev': {
        const project = activeProjectOf(boot())
        if (project === null || target === null) return unmet(command, 'no open project')
        const next = neighbour(
          project.tabs.map((tab) => tab.id),
          target.tab.id,
          command === 'tab.next' ? 1 : -1,
        )
        if (next === null) return unmet(command, 'only one tab is open')
        return void ws.activateTab(project.id, next)
      }

      /*
       * Ctrl+Tab / Ctrl+Shift+Tab — the held-modifier switcher over this project's **tabs**, in
       * most-recently-used order. The same gesture the project switcher below makes, one ring
       * in, and the same implementation: `keys/switcher.ts` is the arithmetic and
       * `keys/switcherStore.ts` holds the one open walk and the one release latch, whichever of
       * the two opened it.
       *
       * `tab.next` / `tab.prev` directly above are the *positional* walk and are deliberately
       * kept: this one matches the order the user works in, that one matches the strip on
       * screen. Neither is bound to the other's chord.
       *
       * Running it from the palette holds no modifier, so it switches immediately to the most —
       * or least — recently used tab, which is a perfectly good terminating thing for a row to
       * do. See `begin` in `keys/switcher.ts`.
       */
      case 'tab.switcher.next':
      case 'tab.switcher.prev':
        return startTabSwitch(command === 'tab.switcher.next' ? 1 : -1)

      /*
       * Ctrl+1 — the pinned Claude console, by identity rather than by position.
       *
       * `revealPane` and not `ws.activateTab`, which is the whole difference between showing the
       * console and *arriving in the prompt*: it activates the project, activates the tab, drops
       * a maximize that would hide the pane, moves the domain's focus and then puts the caret in
       * the terminal, in the one order that works. `claude.mention.file` already ends the same
       * way, for the same reason.
       *
       * `consolePaneOf` and not `claudeTargetOf`: this command names `tabs[0]`'s Claude pane
       * unconditionally, where a mention prefers the focused conversation. Both preconditions
       * are re-checked here — a `Command::when` gates the palette and never the keyboard.
       */
      case 'tab.console': {
        if (boot()?.role.kind !== 'shell') return unmet(command, 'this window has no tab strip')
        const console = consolePaneOf(boot())
        if (console === null) return unmet(command, 'no open project')
        return void revealPane(console.project, console.pane).then((refusal) => {
          if (refusal !== null) void diag.log(`[cide] ${command}: ${refusal}`)
        })
      }

      /* ---------------------------------------------------------------------- Projects */

      /*
       * Ctrl+` — the held-modifier switcher over **projects**, in most-recently-used order.
       *
       * It was Ctrl+Tab until the tab switcher above took that chord; `cide_core::keymap` holds
       * the argument for the move and for why `backquote` and never the literal tilde. The
       * whole gesture is in `keys/switcher.ts` (pure) and `keys/switcherStore.ts` (the one open
       * walk and its release watcher); this arm only names the direction.
       *
       * `project.switcher.prev` has no default chord — `ctrl+shift+backquote` is
       * `terminal.splitBelow` — so this row and holding Shift during a walk are its two routes,
       * which is why it is still registered. Running either from the palette holds no modifier,
       * so it switches immediately to the most — or least — recently used project, because there
       * is no release for the popup to wait on. See `begin` in `keys/switcher.ts`.
       */
      case 'project.switcher.next':
      case 'project.switcher.prev':
        return startProjectSwitch(command === 'project.switcher.next' ? 1 : -1)

      case 'project.next':
      case 'project.prev': {
        // The *immediate* walk, in **header order** — `role.projects` is this window's strip
        // in the order it is drawn. Unbound by default and kept for the palette and for
        // anyone who wants a plain next/previous on a key of their own: it is the one that
        // matches the strip on screen, where the switcher above matches how the user works.
        // `windowProjectsOf` argues down the workspace-wide list, which is a keystroke that
        // does nothing in `perProject` window mode.
        const current = boot()
        const ids = windowProjectsOf(current)
        const active = current?.role.kind === 'shell' ? current.role.active : null
        if (active === null) return unmet(command, 'this window shows no project strip')
        const next = neighbour(ids, active, command === 'project.next' ? 1 : -1)
        if (next === null) return unmet(command, 'this window holds only one project')
        return void ws.activateProject(next as ProjectId)
      }

      /* ------------------------------------------------------------------------ Claude */

      case 'claude.split.newSession':
        if (on === null) return unmet(command, 'no focused pane')
        return split(on, 'col', { kind: 'newClaude' })

      case 'claude.fork':
        // Branches the focused conversation: shared history to this point, then divergent.
        if (on === null) return unmet(command, 'no focused pane')
        return split(on, 'col', { kind: 'forkPrimary' })

      case 'claude.mirror': {
        // No new process — a second sink on the session already running. Meaningless
        // without one, so it reports rather than splitting into an empty pane.
        if (target === null || on === null) return unmet(command, 'no focused pane')
        const session = target.pane.session
        if (session === null) return unmet(command, 'the focused pane has no session yet')
        return split(on, 'row', { kind: 'mirror', session })
      }

      /*
       * Restart, and resume — the two ways back into a pane whose `claude` has gone.
       *
       * `claude.restart` was registered from the first draft of the command table and carried
       * `.unavailable("needs a respawn path in the pane host; kill alone is not a restart")`,
       * which was an accurate description of the gap: only `TerminalPane` knows a pane's
       * geometry and holds the terminal a new child must attach to, so a command layer that
       * could only kill would have been a *stop* command wearing the word restart. The pane
       * publishes the respawn now (`panes/paneRestart.ts`) and these two run it.
       *
       * Two ids rather than one with an argument, because ids are API and are never renamed:
       * splitting them later would leave a `claude.restart` in somebody's `keymap.json` meaning
       * whichever of the two we happened to pick today. Neither is bound by default — a restart
       * is not a per-minute gesture — and both are reachable from the pane's own bar and its
       * context menu, which is where a user looking at a dead terminal actually is.
       */
      case 'claude.restart':
      case 'claude.resume': {
        if (target === null || on === null) return unmet(command, 'no focused pane')
        // The clause is `claudePaneFocused` and a clause does not gate the keyboard, so the
        // same fact is checked here — the rule this file's header states.
        if (target.pane.kind !== 'claude') return unmet(command, 'the focused pane is not Claude')
        const restarter = paneRestarter(on.pane)
        if (restarter === null) {
          // A pane this window is not showing — the detached-pane case, where `focusTarget`
          // answers `null` anyway — or one whose mount has been torn down. Reported rather
          // than silently ignored: the palette offered the row.
          return unmet(command, 'this window is not showing that pane')
        }
        if (command === 'claude.restart') {
          void restarter.restart('fresh').catch(notifyFailure)
          return
        }
        void restarter
          .canResume()
          .then((yes) => {
            if (!yes) {
              // A sentence, not a diag line. Whether Claude Code still holds the transcript is
              // a fact about the user's disk that they just asked a question about, and the
              // alternative to saying so is a menu row that appears to do nothing.
              notify('There is no saved conversation for this pane to resume.', { kind: 'info' })
              return
            }
            return restarter.restart('resume')
          })
          .catch(notifyFailure)
        return
      }

      case 'claude.mention.file': {
        // Deliberately uncaught: `claude_send_lines` rejects with `noServer` /
        // `notConnected` when nothing is listening, and `chrome/Failures.tsx` turns that
        // rejection into something on screen. A `.catch(() => {})` here would make a
        // missing IDE server look exactly like a control wired to nothing.
        const to = claudeTargetOf(boot())
        const path = pathArg(args) ?? focusedFilePath(boot())
        if (to === null) return unmet(command, 'no Claude pane to mention into')
        if (path === null) return unmet(command, 'no file tab focused and no path argument')
        // `sent.pane`, not `to.pane`: Rust reroutes when the pane named here has no `claude`
        // on the IDE server — which is the ordinary state of a Claude pane sitting at a
        // resume splash — and revealing the pane we asked for would show the user an empty
        // prompt while their mention sat in a different conversation. The reveal is what
        // makes the destination visible at all from a palette row, which has no editor to
        // report back into.
        return void claudeSend
          .lines(to.project, to.pane, path, '')
          .then((sent) => revealPane(to.project, sent.pane))
      }

      /* ---------------------------------------------------------------------- Terminal */

      case 'terminal.splitBelow':
        if (on === null) return unmet(command, 'no focused pane')
        return split(on, 'col', { kind: 'shell' })

      case 'terminal.clear': {
        // xterm's own scrollback, not the child's and not the Rust screen mirror. Clearing
        // the mirror would blank every other pane showing the same session, including a
        // detached window — see the same argument on the pane menu's Clear item.
        if (on === null) return unmet(command, 'no focused pane')
        peekHost(on.pane)?.terminal?.term.clear()
        return
      }

      case 'terminal.paste': {
        // Through the terminal, not the pty. `term.paste` is the one path that wraps the text
        // in `ESC[200~ … ESC[201~` when — and only when — the child has asked for bracketed
        // paste, which is what stops a multi-line paste executing line by line in `bash` and
        // submitting at the first newline in `claude`. It leaves through the same `onData` the
        // pane already writes to the pty, so this is not a second way of reaching the child.
        // One implementation, shared with the pane menu and with Ctrl+V: `terminal/clipboard.ts`.
        if (on === null) return unmet(command, 'no focused pane')
        const term = peekHost(on.pane)?.terminal?.term
        if (term === undefined) return unmet(command, 'the focused pane has no terminal')
        if (paneSessionId(on.pane) === undefined) {
          return unmet(command, 'the focused pane has no session yet')
        }
        void pasteIntoTerminal(term).catch(
          (error: unknown) => void diag.log(`terminal.paste failed: ${String(error)}`),
        )
        return
      }

      /* -------------------------------------------------------------------------- File */

      /*
       * Ctrl+S, and the palette's *Save file*.
       *
       * This has to be here rather than left to CodeMirror. `ctrl+s` is in
       * `cide_core::keymap::defaults()`, so the gate resolves it, swallows the keystroke and
       * calls this dispatcher — CodeMirror's own `Mod-s` binding in `EditorSurface` never
       * sees the event. A key bound to a command nothing dispatches is worse than an unbound
       * one: the editor's own handler would at least have worked.
       *
       * Saving nothing is still an ordinary outcome and still says nothing: Ctrl+S with a
       * Claude tab focused finds no editor registered for that tab, and a diagnostic line
       * per keystroke for a keystroke that did the right thing is noise. The stroke stays
       * swallowed either way — letting it through to a focused terminal would send `^S`,
       * which is flow control, and freeze the pane with no visible cause.
       */
      case 'file.save': {
        const saving = saveTab((deps.focusedTab ?? focusedTabOfWorkspace)())
        // Failures are reported by the pane that owns the file, and the tab stays dirty —
        // which is what puts the close confirmation in front of the user. Swallowed here so
        // an unhandled rejection does not reach the window.
        if (saving !== null) void saving.catch(() => {})
        return
      }

      /*
       * Every live editor, not every dirty one — and that is a compromise, not a design.
       *
       * `openBuffers` holds savers, not dirty flags: the flag lives in `EditorSurface`'s
       * `baseline`, travels *outward* through `tab_set_dirty`, and never comes back — Rust
       * has no command that answers "which tabs are unsaved". So the only list this layer can
       * produce is "every editor that is mounted".
       *
       * For a buffer that matches its file the extra write costs an mtime and nothing else.
       * It is **not** free for a buffer that is clean and *stale*: there is no fs watcher yet
       * — `EditorPane` follows `cide://session-tool` only — so a file rewritten by `sed -i`
       * in a shell pane is still showing its old contents, and *Save all files* writes those
       * old contents back over it. That is a narrow window and it is a real one, and the way
       * to close it is a `dirty` predicate on the registry entry, which needs `EditorPane` to
       * pass one.
       */
      case 'file.saveAll': {
        const tabs = registeredBuffers()
        if (tabs.length === 0) return unmet(command, 'no editor is open')
        void saveAll(tabs).then(({ failed }) => {
          if (failed.length > 0) void diag.log(`file.saveAll: ${failed.length} file(s) not saved`)
        })
        return
      }

      /*
       * *Select opened file* — ⌃⇧E, the ⌖ button in the Explorer header, and the palette row.
       *
       * All three arrive here and none of them has its own copy of the rules, which is the
       * point: `Explorer`'s button calls `runCommand('file.reveal', null)` rather than
       * `treeStore.reveal` directly, because a second call site with its own preconditions is
       * how three gestures come to behave in three ways.
       */
      case 'file.reveal': {
        /*
         * Every refusal below is a *sentence*, and that is the whole of what this arm learned.
         *
         * It used to answer `unmet`, which writes a line to the diagnostic log and nothing to
         * the screen. With no binding on the command that was survivable — the palette hid the
         * row. With a hotkey on it, it is a key that does nothing, in an application that has
         * now found that defect fifteen times.
         */
        if (deps.showSidebar === undefined || boot()?.role.kind !== 'shell') {
          notify('This window has no file tree, so there is nothing to select a file in.', {
            kind: 'info',
            hint: 'Detached panes are their own window and show no sidebar.',
          })
          return
        }
        // The path comes from the focused tab when the caller named none, so the palette row
        // and the button work. `args.path` still wins: a `keymap.json` entry may carry one.
        //
        // `focusedTabPath`, not `focusedFilePath`: a diff tab names the file it is diffing, and
        // revealing it is what IDEA does. See that function for the git-diff case it excludes.
        const path = pathArg(args) ?? focusedTabPath(boot())
        if (path === null) {
          notify('No file tab is active, so there is nothing to select.', {
            kind: 'info',
            hint: 'Open a file — the Claude console and the settings tab are not files.',
          })
          return
        }
        // Shown before revealed. Revealing into a sidebar that is on Git — or closed —
        // scrolls a tree nobody can see, which is a command that "did nothing" again.
        deps.showSidebar('files')
        /*
         * And the answer is read, which it was not.
         *
         * `fs_reveal` is index-only, so it says "no row" for a gitignored file, for one deleted
         * between the pick and the reveal, and — since M13, ordinarily — for a tab holding a
         * file that is not in the project at all. The store swallowed all three, so this arm was
         * a listed, enabled, bindable command that opened the Files panel and then did nothing
         * whatsoever: the exact defect this project has now found more than a dozen times, in
         * the exact place the out-of-project open makes common.
         *
         * A notice rather than `unmet`: the user made a gesture that could not be honoured, and
         * `unmet` is a diagnostic line nobody sees. It says *why* rather than "failed", because
         * "not in this project's file tree" is the whole answer for the case that now dominates.
         */
        void useFileTree
          .getState()
          .reveal(path)
          .then((shown) => {
            if (!shown) {
              notify(`${path} is not in this project's file tree, so there is no row to show.`, {
                kind: 'info',
                /*
                 * The third clause is M15's. A standard-library file used to land here always —
                 * `~/.rustup/toolchains/…/library/core/src/option.rs` was in no root, no
                 * dependency cache and no External Libraries row, so this sentence fired about a
                 * file the user was looking at. That is fixed at the source (`cide_deps::sdk`
                 * lists the toolchain, `dependency_roots` contains it), and one case survives
                 * legitimately: with `rust-src` uninstalled the SDK row is a note with no path,
                 * so there is genuinely nothing to select. A hint that did not mention it would
                 * send that user looking for a bug in the wrong half.
                 */
                hint: 'Files git ignores have no row in the tree, neither does a file that has been deleted since it was opened, and a standard-library file has none until `rustup component add rust-src` has been run.',
              })
              return
            }
            /*
             * …and give the tree the keyboard, which is the half that made this feel unfinished.
             *
             * The row lights up and the arrows still go to whatever had focus — a terminal,
             * after ⌃⇧E from a pane. The tree's single tab stop is its scroller (`role="tree"`,
             * `tabIndex={0}`), and the request is *parked* rather than focused here for the two
             * reasons `chrome/focusRequests.ts` gives: the panel is usually not mounted yet when
             * this runs, and when it is, nothing mounts, so an `autoFocus` would do nothing on
             * the second press.
             *
             * After the reveal resolves, deliberately: focusing a scroller that is about to
             * scroll to a different index is a focus ring in the wrong place for a frame.
             */
            requestFocus('fileTree')
          })
        return
      }

      /*
       * The External Libraries group, without a mouse and without hunting for it.
       *
       * The group is the last row of a virtualized tree, so in any repository with depth to it
       * the header sits thousands of rows below the viewport — drawn, and in practice
       * unreachable. This is the route that does not depend on scrolling to find it.
       *
       * Shown before revealed, the same order as `file.reveal` above: revealing into a sidebar
       * that is on Git — or shut — scrolls a tree nobody can see.
       *
       * And the answer is read. `revealGroup` is `false` for a project with no `Cargo.toml` and
       * no `go.mod` under any root, which is the ordinary case for a repository in another
       * language: there is genuinely no group row, and saying so is the difference between this
       * command and the fourteen dead controls this project has already found.
       */
      case 'view.externalLibraries': {
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('files')
        void useFileTree
          .getState()
          .revealGroup(EXTERNAL_LIBRARIES)
          .then((shown) => {
            if (shown) return
            notify('This project has no External Libraries group.', {
              kind: 'info',
              hint: 'The group is shown for a project with a Cargo.toml or a go.mod under one of its roots.',
            })
          })
        return
      }

      /*
       * The Scratches group, by the same route and for the same reason as the one above.
       *
       * It is the *last* row of the tree — the second group, under External Libraries — so on
       * any repository with depth to it the header is thousands of rows past the viewport.
       * `scratch.new` reveals it as a side effect of creating something; this is the route for
       * somebody who wants to find the scratches they already have.
       *
       * `revealGroup` answers `false` only for a project with no roots at all, which
       * `cide_core::workspace` refuses to create — so the notice below is the defensive arm
       * rather than the ordinary one, and it still exists because a palette row that scrolls
       * nowhere and says nothing is this codebase's signature defect.
       */
      case 'view.scratches': {
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('files')
        void useFileTree
          .getState()
          .revealGroup(SCRATCHES)
          .then((shown) => {
            if (shown) return
            notify('This project has no Scratches group.', {
              kind: 'info',
              hint: 'The group is keyed by the project\u2019s first root directory.',
            })
          })
        return
      }

      /* --------------------------------------------------------------------------- Git */

      case 'git.push': {
        // Every repository in the project, in root order. A monorepo with submodules has
        // several and the command names none of them, so pushing "the" repo would have to
        // pick one; pushing each is what *Push to remote* says.
        //
        // Uncaught on purpose, like `claudeSend.lines`: a rejected push — no upstream, no
        // credential helper — is exactly what `chrome/Failures.tsx` exists to put on screen.
        withRepos(command, (project, repos) => {
          void Promise.all(repos.map((repo) => gitApi.push(project, repo.id, null, null)))
        })
        return
      }

      /*
       * The branch half. `switch` and `new` open the popup rather than acting, because both
       * need something only the user can supply — which branch, or what to call it — and a
       * palette row that guessed would be worse than no row.
       *
       * The popup is an overlay (`overlays/store.ts`), not a child of the status bar widget,
       * so these work in a window whose status bar never mounted the widget. That is the
       * whole reason it is built that way.
       *
       * No repository check here, deliberately, and unlike the three commands around it. The
       * popup asks `git_branch_list` itself and draws its own "no repository" state
       * (`chrome/BranchSelector.tsx`), so a guard here would be a second opinion about the
       * disk formed one round trip earlier — and the guard it replaces was `reposOf`, which
       * answered "no repository" for every project in existence and made both of these
       * commands do nothing at all.
       */
      case 'git.branch.switch':
      case 'git.branch.new': {
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        openBranchPopup(command === 'git.branch.new' ? 'new' : 'list')
        return
      }

      case 'git.fetch':
      case 'git.pull': {
        /*
         * Every repository in the project, like `git.push` directly above — the command names
         * none of them, and in a superproject picking one would be a guess.
         *
         * # The failures
         *
         * The rejection still reaches `chrome/Failures.tsx` through `unhandledrejection`, but
         * it may not reach it as a raw `GitError`: that is `{kind, detail}` with no `message`
         * field at all, and `notices.describe` falls through to `kind` — so a divergent pull
         * used to toast the bare word `notFastForward`, with the two counts that are the whole
         * point of refusing sitting unread in `detail`. `explain` is the sentence. Rethrown
         * rather than reported here, so the one window listener stays the only surface.
         *
         * One chain per repository rather than `Promise.all`: with four repositories and two
         * failures, `Promise.all` reports the first and marks the rest handled.
         *
         * # The successes, which is the half that was missing
         *
         * `void run(project, repo).catch(…)` reported every way this can fail and **discarded
         * the value when it worked**. That is the recurring defect of this project with the
         * sign flipped: not "implemented and nothing calls it" but "called, and the answer
         * dropped on the floor". A user who pressed the key and got silence has been given
         * the same evidence a dead key would give them.
         *
         * `allSettled` over the *same* promises collects the answers without disturbing the
         * failure path: attaching a handler marks the original promise handled, but the
         * derived promise from each `.catch(… throw …)` above is still unhandled and is what
         * fires `unhandledrejection`. So both halves report, independently, and a project
         * where two of four repositories fail shows two failures and one report of the other
         * two — rather than `Promise.all`'s single failure and nothing else.
         *
         * Aggregated into one notice by `pullReport` rather than one toast per repository:
         * one gesture, one answer, and — see `notices.admit` — five identical "already up to
         * date" texts would collapse into one toast that silently spoke for five.
         */
        const pulling = command === 'git.pull'
        const run = pulling ? branchApi.pull : branchApi.fetch
        withRepos(command, (project, repos) => {
          const attempts = repos.map((repo) => run(project, repo.id))
          for (const attempt of attempts) {
            void attempt.catch((error: unknown) => {
              // The operation only changes one variant's wording, and only on the pull side:
              // a fetch moves no working tree, so it cannot raise `checkoutWouldOverwrite` at
              // all and the argument is unreachable for it.
              throw new Error(explain(error, pulling ? 'pull' : 'checkout'))
            })
          }
          void Promise.allSettled(attempts).then((settled) => {
            const done: RepoFetch[] = []
            settled.forEach((result, i) => {
              const repo = repos[i]
              if (result.status === 'fulfilled' && repo !== undefined) {
                done.push({ name: repo.name, outcome: result.value })
              }
            })
            // Every repository failed. They each have a toast of their own already, and a
            // report of nothing on top of them would be a second surface saying less.
            if (done.length === 0) return
            const report = pullReport(done)
            notify(report.text, { kind: 'info', detail: report.detail })
          })
        })
        return
      }

      case 'git.commit': {
        /*
         * Reveals the commit box; does not commit.
         *
         * The message and the ticked paths live in `useGitPanel`'s React state, which does not
         * exist while the sidebar is on Files or shut — so there is nothing here to commit
         * *with*, and lifting a half-typed message into a store to make one would be storing a
         * gesture rather than mirroring anything Rust owns. `commands.rs` carries the full
         * argument. What is left is the useful half: put the user in front of the control.
         *
         * Shown before focused, the same order and for the same reason as `file.reveal` above
         * — asking the box for focus while the sidebar is still on Files focuses nothing, and a
         * command that did nothing is the whole complaint this file exists to answer. The
         * request is parked in a module-level store because `setView('git')` only mounts the
         * panel on the *next* React render, and it survives to be picked up by whichever render
         * sees it first. `unmet` rather than a thrown error for the detached-pane case: that
         * window has no sidebar and never will, which is a fact about the window and not a
         * failure the user can act on.
         */
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('git')
        requestFocus('commitMessage')
        return
      }

      case 'git.refresh': {
        // The per-path status behind the file tree's tags, and the activity rail's count.
        // The Git *panel* keeps its own tree and refreshes off `cide://git-status` and
        // `cide://session-tool`, neither of which a read-only `git_status` broadcasts — so
        // this cannot reach it from here, and pretending otherwise by calling `git_status` and
        // dropping the answer would be a command that looks like it worked.
        //
        // The rail's count is a second store with the same problem and the same answer: it is
        // event-driven and correct on its own, but *Refresh git status* has to move every
        // number the phrase covers. A refresh that visibly updated the file tree's tags and
        // left the badge on its old figure would look like the badge was the stale one.
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        const status = useGitStatus.getState()
        void (status.project === project.id ? status.refresh() : status.attach(project.id))
        const count = useGitCount.getState()
        void (count.project === project.id ? count.refresh() : count.attach(project.id))
        return
      }

      /* ---------------------------------------------------------------------- Navigate */

      case 'structure.file': {
        // Needs a caret, not merely an open editor: the popup preselects the member the caret is
        // in, and with no caret there is nothing to preselect and nothing to jump *from*.
        if (focusedCaret() === null) return unmet(command, 'no editor focused')
        useOverlays.getState().toggle('structure')
        return
      }

      /*
       * A new scratch file: ⇧⌥S, and the palette row.
       *
       * The chord is unconditional in `keymap.rs` — the gate never reads a `Command::when` —
       * so this arm is what stops it opening a picker in a window that cannot honour it. Both
       * refusals are `unmet` rather than a notice, and that is the deliberate difference from
       * `file.reveal` above: those two are *facts about the user's situation that they just
       * asked a question about*, while these are "this window is not the one that has a project
       * and a file tree", which is a precondition the palette already hides the row for and
       * which a detached-pane window is in permanently. A notice there would fire on every
       * stray ⇧⌥S in a torn-out terminal.
       */
      case 'scratch.new': {
        if (boot()?.role.kind !== 'shell') return unmet(command, 'this window has no file tree')
        if (activeProjectOf(boot()) === null) return unmet(command, 'no open project')
        // `toggle`, not `show` — the same argument `navigate.line` makes below: the gate
        // swallows the chord either way, so `show` would leave ⇧⌥S unable to dismiss what ⇧⌥S
        // opened.
        useOverlays.getState().toggle('scratch')
        return
      }

      case 'navigate.line': {
        /*
         * Needs a caret, not merely an open editor: the popup opens on the current line, reports
         * how many lines the file has, and jumps within that one file. A `when` gates the palette
         * and never the keyboard, so Ctrl+G in a terminal pane arrives here and is refused with a
         * sentence rather than opening a box with nothing to jump in.
         */
        if (focusedCaret() === null) return unmet(command, 'no editor focused')
        // `toggle`, not `show` — the same argument as `picker.files` below. The gate swallows the
        // chord either way, so `show` would leave Ctrl+G unable to dismiss what Ctrl+G opened.
        // `editorFocused` is derived from the focused *pane*, which the overlay does not change,
        // so the second press still resolves and still lands here.
        useOverlays.getState().toggle('goto')
        return
      }

      case 'navigate.definition': {
        /*
         * Unlike its neighbours below this one *does* go to Rust, because resolution is the
         * language server's and nothing cached can answer it. That is also why the `when` clause
         * is not enough on its own: a `when` gates the palette, never the keyboard, so Ctrl+B in
         * a terminal pane arrives here and has to be refused with a sentence rather than run
         * against a caret that does not exist.
         */
        const caret = focusedCaret()
        if (caret === null) return unmet(command, 'no editor focused')
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        // Fire-and-forget: every outcome — found, not found, still indexing — reports itself
        // through `Failures`. See `editor/goToDefinition.ts`.
        goToDefinition(project.id, caret.path, caret.line, caret.column)
        return
      }

      case 'navigate.usages': {
        /*
         * ⌥F7, and the escape hatch for the day Ctrl+click's discriminator guesses wrong.
         *
         * Same shape as `navigate.definition` above and for the same reason: a `when` gates the
         * palette and never the keyboard, so this has to refuse with a sentence rather than run
         * against a caret that does not exist.
         *
         * This one **skips the discriminator entirely** — it searches for references whatever the
         * caret is on. That is deliberate and is most of the point: `fn fmt` inside an
         * `impl Display` resolves to the *trait's* declaration, so Ctrl+click on it jumps rather
         * than listing, and without an unconditional command there would be no way to ask the
         * question the user actually meant. It also works from a plain reference — LSP answers
         * references from any occurrence — so ⌥F7 on a call site lists that call's siblings.
         *
         * `focusedWord()` may answer `null` (a caret on punctuation, a window with no editor); the
         * popup and the notice both degrade to "the symbol" rather than refusing.
         */
        const caret = focusedCaret()
        if (caret === null) return unmet(command, 'no editor focused')
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        findUsages(project.id, caret.path, caret.line, caret.column, focusedWord())
        return
      }

      case 'navigate.back':
      case 'navigate.forward': {
        /*
         * The mouse's thumb buttons, and anything a user binds to these ids.
         *
         * Every decision is `editor/navHistory.ts`'s and every refusal is `editor/jump.ts`'s.
         *
         * The refusal goes to `notify`, not to `unmet`. That is a deliberate exception to this
         * file's own rule and it is the second half of M15's mouse-button report: `unmet` writes
         * one line through `diag.log`, into a file the user never opens, so a Back that refuses
         * is on screen indistinguishable from a Back that is wired to nothing — which is
         * *precisely* the state the thumb buttons were actually in, and precisely why nobody
         * could tell the two apart while diagnosing it. `jump.ts`'s own header says the refusal
         * is a value rather than a silence "because a mouse button that does nothing is
         * indistinguishable from a mouse button wired to nothing"; routing it to a log defeats
         * the sentence it went to the trouble of writing.
         *
         * Every other `unmet` in this file reports a *precondition a `when` clause should
         * already have caught* — a programming error, whose audience is a developer reading the
         * log. This one is an ordinary, expected state ("you have not jumped anywhere yet") whose
         * audience is the user, and the empty history is the state Back spends most of its life
         * in: nothing records an entry for Ctrl+Tab, a tab-strip click, caret motion or
         * scrolling, so a first press very often lands here.
         *
         * The `when` clause is `projectOpen`, so this is reachable with focus anywhere,
         * including a terminal. That is the point: the thumb button is pressed wherever the
         * pointer is.
         */
        const refusal = navigate(command === 'navigate.back' ? 'back' : 'forward')
        if (refusal !== null) {
          notify(refusal, { kind: 'info' })
          return
        }
        return
      }

      case 'navigate.nextMember':
      case 'navigate.prevMember': {
        /*
         * Answered entirely from the cached outline — no IPC. That is the reason the cache
         * exists: this fires on a held key, and a round trip per press would be the freeze
         * `cmd/picker.rs` was written to avoid.
         */
        const caret = focusedCaret()
        if (caret === null) return unmet(command, 'no editor focused')
        const symbols = symbolsOf(caret.path)
        if (symbols.length === 0) return unmet(command, 'this file has no outline yet')

        const direction = command === 'navigate.nextMember' ? 'next' : 'prev'
        const to = memberStep(symbols, caret.line, caret.column, direction)
        // Clamped, never wrapped — see `memberNav.ts`. Reported rather than silently ignored,
        // per `unmet`'s doctrine: a chord that does nothing with no trace is the complaint this
        // whole file exists to answer.
        if (to === null) {
          return unmet(
            command,
            direction === 'next'
              ? 'the caret is already at the last member'
              : 'the caret is already at the first member',
          )
        }
        /*
         * `requestReveal` delivers to *every* editor on this path, so in a split showing one
         * file twice both carets move. Harmless — they show the same file — and the alternative
         * is a second, pane-targeted reveal path, which would be two answers to one question.
         *
         * **`requestReveal` and not `jumpTo`, deliberately: the member walk is not recorded in
         * the navigation history.** This is bound to `alt+up`/`alt+down`, which is a held key,
         * so ten presses would be ten entries — and `navHistory`'s merge rule cannot collapse
         * them, because members are far apart by construction. The origin of the walk is still
         * reachable through Back, because whatever brought the caret into this file recorded an
         * entry. `editor/navHistory.ts` carries the full list of what is and is not recorded.
         */
        requestReveal(caret.path, {
          line: to.startLine,
          column: to.startColumn,
          endColumn: to.endColumn,
        })
        return
      }

      /* -------------------------------------------------------------------------- View */

      case 'picker.symbols':
        // Toggling, like `picker.files`: Escape is the only other way out of an overlay.
        //
        // The index is built lazily and idempotently, so asking here — rather than at project
        // open — means a user who never presses this never pays for a symbol walk.
        if (activeProjectOf(boot()) === null) return unmet(command, 'no open project')
        useOverlays.getState().toggle('symbols')
        return

      case 'picker.files':
        // `toggle`, not `show`: Ctrl+P with the picker already up closes it. The gate
        // swallows the chord either way, so a `show` here would leave the user with no
        // keystroke that dismisses what their keystroke opened except Escape.
        useOverlays.getState().toggle('files')
        return

      case 'palette.commands':
        useOverlays.getState().toggle('commands')
        return

      case 'theme.toggle':
        // The persisting one from `settings/useSettings`, not `useWorkspace.toggleTheme` —
        // that only sets the local flag, so the choice was lost on the next snapshot and
        // gone entirely on the next launch.
        toggleTheme()
        return

      case 'settings.open':
      case 'settings.keymap': {
        // Settings is a tab inside a project, so it needs one to open into.
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        const section = command === 'settings.keymap' ? 'keymap' : null
        void settingsApi.openTab(project.id, section).then(() => ws.hydrate())
        return
      }

      case 'sidebar.files':
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('files')
        return

      /*
       * F4 — hide the panel, or bring back the last one that was open.
       *
       * The *hidden* state is not new: clicking the lit rail button has set the view to `null`
       * since M3, `ActivityRail::active` has always been nullable, and the four panel branches
       * and the splitter are all guarded on it. What was missing was any way to reach it without
       * a mouse, and any memory of which panel to restore. Both live in
       * `chrome/sidebarView.ts`; this arm only routes.
       *
       * `unmet` rather than a notice, unlike `file.reveal` two screens up. That one refuses over
       * *facts about the user's situation that they just asked a question about*; this one
       * refuses only in a window that has no sidebar and never will, which the binding's
       * `shellWindow` clause and the command's already keep the gesture out of. Reaching this
       * line means the gate and the palette disagree, which is exactly what the diagnostic log
       * is for.
       */
      case 'sidebar.toggle':
        if (deps.toggleSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.toggleSidebar()
        return

      case 'sidebar.git':
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('git')
        return

      case 'sidebar.problems':
        // Reveal, never toggle — the same call `sidebar.files` and `sidebar.git` make, and for
        // the reason spelled out under `sidebar.search`: a panel is not an overlay, the ⚑ button
        // is right there, and a second press that closed it would take away the thing the
        // binding is for.
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('problems')
        return

      case 'sidebar.search':
        /*
         * Reveal *and* focus, and never toggle shut.
         *
         * `picker.files` two screens up toggles because Escape is the only other way out of an
         * overlay. A sidebar panel is not an overlay: the ⌕ button in the activity rail is
         * right there, and a second Ctrl+Shift+F that closed the panel would take away the one
         * thing this binding is for, which is landing in the search box with a chord. VS Code
         * behaves the same way and for the same reason.
         *
         * Focusing is not a flourish. The panel had no keyboard path of any kind — no command,
         * no binding, and no `ref`, `autoFocus` or focus effect on its input, so even the
         * existing mouse path revealed a panel and left the caret in the terminal. Revealing
         * without focusing would have been a new command with the old defect.
         */
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('search')
        requestFocus('search')
        return

      default:
        // Not a registered command at all: see `DispatchDeps.fallback`.
        deps.fallback(command, args)
    }
  }
}

/**
 * The entry `step` places along from `current`, wrapping, or `null` when there is nowhere
 * else to go.
 *
 * Wrapping because both callers cycle a strip the user can see: stopping at the end would
 * make Ctrl+Tab on the last project do nothing, which reads as the key being broken. `null`
 * for a list of one keeps that case reportable rather than silently re-activating what is
 * already active.
 */
function neighbour(ids: readonly string[], current: string, step: 1 | -1): string | null {
  if (ids.length < 2) return null
  const at = ids.indexOf(current)
  if (at < 0) return null
  return ids[(at + step + ids.length) % ids.length] ?? null
}

/** A `fallback` that only reports. Useful until the workspace dispatcher exists. */
export function reportOnly(command: string, args: unknown): void {
  void diag.log(`[cide] command not handled: ${command} ${args === undefined ? '' : JSON.stringify(args)}`)
}
