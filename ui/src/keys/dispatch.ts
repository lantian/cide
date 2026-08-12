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
import { useGitStatus } from '@/sidebar/gitStatusStore'
import { useWorkspace } from '@/store/workspace'
import { toggleTheme } from '@/settings/useSettings'
import { paneSessionId, peekHost } from '@/layout/paneHosts'
import { openBranchPopup } from '@/chrome/BranchSelector'
import { explain } from '@/chrome/branchModel'
import {
  branch as branchApi,
  claudeSend,
  clipboard,
  diag,
  git as gitApi,
  session as sessionApi,
  settings as settingsApi,
  type Axis,
  type Direction,
  type PaneId,
  type ProjectId,
  type SplitIntent,
  type TabId,
} from '@/ipc/client'
import { focusedTabOf, registeredBuffers, saveAll, saveTab } from '@/editor/openBuffers'
import { startProjectSwitch } from './switcherStore'
import {
  activeProjectOf,
  claudeTargetOf,
  focusTarget,
  focusedFilePath,
  isClosableTab,
  reposOf,
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
  /** Switch the activity rail's sidebar view, for `sidebar.files` / `sidebar.git`. */
  showSidebar?: ((view: 'files' | 'git') => void) | undefined
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

      /* ---------------------------------------------------------------------- Projects */

      /*
       * Ctrl+Tab / Ctrl+Shift+Tab — the held-modifier switcher, in most-recently-used order.
       *
       * The whole gesture is in `keys/switcher.ts` (pure) and `keys/switcherStore.ts` (the
       * open walk and its release watcher); this arm only names the direction. Running either
       * from the command palette is a legitimate, terminating thing to do and switches
       * immediately to the most — or least — recently used project, because no modifier is
       * being held for the popup to wait on. See `begin` in `keys/switcher.ts`.
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

      case 'claude.mention.file': {
        // Deliberately uncaught: `claude_send_lines` rejects with `noServer` /
        // `notConnected` when nothing is listening, and `chrome/Failures.tsx` turns that
        // rejection into something on screen. A `.catch(() => {})` here would make a
        // missing IDE server look exactly like a control wired to nothing.
        const to = claudeTargetOf(boot())
        const path = pathArg(args) ?? focusedFilePath(boot())
        if (to === null) return unmet(command, 'no Claude pane to mention into')
        if (path === null) return unmet(command, 'no file tab focused and no path argument')
        return void claudeSend.lines(to.project, to.pane, path, '')
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
        // "Paste" here means *write bytes to a pty*, which is what every keystroke in the
        // pane already does — this is the one surface where WebKit's refusal to run
        // `execCommand('paste')` from page script does not bite.
        if (on === null) return unmet(command, 'no focused pane')
        const session = paneSessionId(on.pane)
        if (session === undefined) return unmet(command, 'the focused pane has no session yet')
        void (async () => {
          const text = await clipboard.readText()
          if (text !== '') await sessionApi.write(session, text)
        })().catch((error: unknown) => void diag.log(`terminal.paste failed: ${String(error)}`))
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

      case 'file.reveal': {
        // The path comes from the focused file tab when the caller named none, so the
        // palette row works. `args.path` still wins: a `keymap.json` entry may carry one.
        const path = pathArg(args) ?? focusedFilePath(boot())
        if (path === null) return unmet(command, 'no file tab focused and no path argument')
        // Shown before revealed. Revealing into a sidebar that is on Git — or closed —
        // scrolls a tree nobody can see, which is a command that "did nothing" again.
        deps.showSidebar?.('files')
        void useFileTree.getState().reveal(path)
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
        const project = activeProjectOf(boot())
        const repos = reposOf(boot())
        if (project === null || repos.length === 0) return unmet(command, 'no repository open')
        void Promise.all(repos.map((repo) => gitApi.push(project.id, repo, null, null)))
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
       */
      case 'git.branch.switch':
      case 'git.branch.new': {
        const project = activeProjectOf(boot())
        const repos = reposOf(boot())
        if (project === null || repos.length === 0) return unmet(command, 'no repository open')
        openBranchPopup(command === 'git.branch.new' ? 'new' : 'list')
        return
      }

      case 'git.fetch':
      case 'git.pull': {
        // Every repository in the project, like `git.push` directly above — the command names
        // none of them, and in a superproject picking one would be a guess.
        //
        // The rejection still reaches `chrome/Failures.tsx` through `unhandledrejection`, but
        // it may not reach it as a raw `GitError`: that is `{kind, detail}` with no `message`
        // field at all, and `Failures.describe` falls through to `kind` — so a divergent pull
        // used to toast the bare word `notFastForward`, with the two counts that are the whole
        // point of refusing sitting unread in `detail`. `explain` is the sentence. Rethrown
        // rather than reported here, so the one window listener stays the only surface.
        //
        // One chain per repository rather than `Promise.all`: with four repositories and two
        // failures, `Promise.all` reports the first and marks the rest handled.
        const project = activeProjectOf(boot())
        const repos = reposOf(boot())
        if (project === null || repos.length === 0) return unmet(command, 'no repository open')
        const run = command === 'git.pull' ? branchApi.pull : branchApi.fetch
        for (const repo of repos) {
          void run(project.id, repo).catch((error: unknown) => {
            throw new Error(explain(error))
          })
        }
        return
      }

      case 'git.refresh': {
        // The per-path status behind the file tree's tags. The Git *panel* keeps its own
        // tree and refreshes off `cide://git-status` and `cide://session-tool`, neither of
        // which a read-only `git_status` broadcasts — so this cannot reach it from here, and
        // pretending otherwise by calling `git_status` and dropping the answer would be a
        // command that looks like it worked.
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        const status = useGitStatus.getState()
        void (status.project === project.id ? status.refresh() : status.attach(project.id))
        return
      }

      /* -------------------------------------------------------------------------- View */

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

      case 'sidebar.git':
        if (deps.showSidebar === undefined) return unmet(command, 'this window has no sidebar')
        deps.showSidebar('git')
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
