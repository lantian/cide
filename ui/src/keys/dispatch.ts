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
import { treeFocused } from '@/sidebar/treeFocus'
import { applyScope, scopeFromTree } from '@/sidebar/searchScope'
import { EXTERNAL_LIBRARIES, PROJECT_NOTES, SCRATCHES, groupPath } from '@/sidebar/groupRows'
import { useGitStatus } from '@/sidebar/gitStatusStore'
import { useWorkspace } from '@/store/workspace'
import { toggleTheme } from '@/settings/useSettings'
import { paneSessionId, peekHost } from '@/layout/paneHosts'
import { openBranchPopup } from '@/chrome/BranchSelector'
// M22. The one seam into the extension host from the key layer: `invoke` posts to a worker and
// answers whether one was there to hear it, which is what makes a contributed command reportable
// as unavailable rather than silently inert.
import { invoke as invokeExtension } from '@/ext/host'
import { explain, pullReport, pushReport, type RepoFetch, type RepoPush } from '@/chrome/branchModel'
import {
  divergenceOf,
  strategyAsk,
  type PullStrategy,
  type RepoDivergence,
} from '@/chrome/pullStrategyModel'
import { requestPullStrategy } from '@/chrome/pullStrategyStore'
import { showConflicts } from '@/chrome/conflictsStore'
import { notify, notifyFailure } from '@/chrome/notices'
import { pasteIntoTerminal } from '@/terminal/clipboard'
import { openTerminalFind } from '@/terminal/findStore'
import { paneRestarter } from '@/panes/paneRestart'
import { useGitCount } from '@/chrome/gitCountStore'
import { toggleBlame } from '@/editor/blameStore'
import { requestFocus } from '@/chrome/focusRequests'
import { panelHostPresent, requestPanel } from '@/chrome/panelRequests'
import {
  agentRuns as agentRunsApi,
  agents as agentsApi,
  branch as branchApi,
  claudeSend,
  diag,
  diagnostics,
  file as fileApi,
  fs as fsApi,
  git as gitApi,
  settings as settingsApi,
  toolWindow as toolWindowApi,
  history as historyApi,
  tab as tabApi,
  type Axis,
  type Direction,
  type PaneId,
  type ProjectId,
  type RepoInfo,
  type SplitIntent,
  type TabId,
} from '@/ipc/client'
import { focusedCaret, focusedFolds, focusedFormat, focusedWord } from '@/editor/caretTrack'
import { formatDocument } from '@/editor/formatDocument'
import { goToDefinition } from '@/editor/goToDefinition'
import { findUsages, goToImplementation } from '@/editor/codeIntel'
import { refreshDiagnostics } from '@/sidebar/ProblemsPanel/actions'
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
import { clusterPlan, detachedTabs } from '@/windows/windowTabs'

export interface DispatchDeps {
  /**
   * Commands nothing in the registry declares.
   *
   * Once this handled all 42 there is nothing left for a host to own, so this is reached
   * only by an id that is not a command: a typo in a user's `keymap.json`, or a stale
   * binding naming something that has been deleted. `reportOnly` is a fine value for it.
   */
  fallback: (command: string, args: unknown) => void
  /*
   * There is no `showSidebar` here any more, and its absence is the point.
   *
   * It used to be an optional closure over `App.tsx`'s `setSidebar`, and it was the *only* way
   * to reveal a panel — which meant that anything outside this dispatcher could not reveal one
   * at all. The git log's *Amend…* shipped listed and **disabled** for exactly that reason: the
   * item's whole job is to put the user in front of the commit box, and it had no way to open
   * the panel the box lives in. `README.md` recorded the missing seam as the reason.
   *
   * `chrome/panelRequests.ts` is that seam, and every arm below now goes through it. One
   * mechanism rather than two: a second way to reveal a panel is a second set of preconditions
   * to keep in step, and this file's header is a list of what happens when two routes to one
   * gesture drift. The refusal survives intact — `requestPanel` answers `false` in a window
   * where nothing registered a sidebar, which is precisely what `showSidebar === undefined`
   * used to stand for, asked of the module instead of read off a prop.
   */
  /**
   * Hide the left panel, or bring back the last one that was open. F4, and the palette row.
   *
   * Separate from the reveal seam rather than an extra value it accepts, because the two are
   * different questions: a reveal names a panel and always shows it, and this one names none
   * and depends on what was showing. Which panel comes back is `chrome/sidebarView.ts`'s
   * arithmetic, not this module's — that is a rule with cases in it, and a rule that lives in a
   * React state updater is a rule no check script can compile.
   *
   * Still a dep rather than a second module-level opener, and deliberately: `App.tsx` hands
   * `setSidebar` the `toggleSidebar` *updater itself*, which is what keeps the decision out of
   * the component. There is nothing outside React that needs to press F4.
   *
   * Optional because a detached-pane window has no rail and no sidebar to toggle.
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

/**
 * One pass of `git.fetch` / `git.pull` over every repository, and the retry after a dialog.
 *
 * Factored out of the `case` so the answer to *merge or rebase?* re-enters exactly the same
 * code path the first attempt took — one place that knows how to fan out, report and fail, and
 * therefore one place that can be wrong.
 *
 * `strategy === null` means *first pass*: only then is a divergence a question rather than a
 * failure, and only then is the dialog built.
 */
function pullPass(
  project: ProjectId,
  repos: readonly RepoInfo[],
  pulling: boolean,
  strategy: PullStrategy | null,
  remember: boolean,
): void {
  const attempts = repos.map((repo) =>
    pulling
      ? branchApi.pull(project, repo.id, {
          // Spread rather than `strategy: strategy ?? undefined`. `strategy` is `#[ts(optional)]`
          // on the wire and `exactOptionalPropertyTypes` is on, so *absent* and *present and
          // undefined* are different types here — and absent is the one that means "decide from
          // configuration".
          ...(strategy !== null ? { strategy } : {}),
          // See the `case` above: the refusal was raised *after* the fetch, so the numbers the
          // user just answered about are already on disk.
          skipFetch: strategy !== null,
          remember,
        })
      : branchApi.fetch(project, repo.id),
  )

  for (const attempt of attempts) {
    void attempt.catch((error: unknown) => {
      // A divergence is a question, asked below. Swallowed rather than rethrown so this derived
      // promise *resolves* and no `unhandledrejection` fires for it; every other rejection keeps
      // the property the `case`'s comment depends on.
      if (strategy === null && divergenceOf(error) !== null) return
      // The operation only changes one variant's wording, and only on the pull side: a fetch
      // moves no working tree, so it cannot raise `checkoutWouldOverwrite` at all and the
      // argument is unreachable for it.
      throw new Error(explain(error, pulling ? 'pull' : 'checkout'))
    })
  }

  void Promise.allSettled(attempts).then((settled) => {
    const done: RepoFetch[] = []
    const asked: RepoDivergence[] = []
    // The repositories a merge or rebase stopped in, carried with their **ids** rather than
    // looked up by name afterwards: two roots in a monorepo can share a basename, and a lookup
    // that matched the wrong one would open the conflict list over somebody else's merge.
    const stopped: { id: string; name: string }[] = []
    settled.forEach((result, i) => {
      const repo = repos[i]
      if (repo === undefined) return
      if (result.status === 'fulfilled') {
        done.push({ name: repo.name, outcome: result.value })
        if (result.value.conflicts.length > 0) stopped.push({ id: repo.id, name: repo.name })
        return
      }
      const diverged = strategy === null ? divergenceOf(result.reason) : null
      if (diverged !== null) asked.push({ name: repo.name, repo: repo.id, diverged })
    })

    // Every repository failed. They each have a toast of their own already, and a report of
    // nothing on top of them would be a second surface saying less.
    if (done.length > 0) {
      const report = pullReport(done)
      notify(report.text, { kind: 'ok', detail: report.detail })
    }

    /*
     * A conflict opens its own list. (M20)
     *
     * The toast says *"2 files to resolve"* and offers nothing that resolves them; the Git
     * panel's `MergeBar` does, but only if the sidebar happens to be open — and Ctrl+T is
     * reachable from a terminal pane with it shut. So the one surface that a stopped merge
     * cannot leave to chance opens itself, which is what IDEA does and why.
     *
     * The **first** conflicted repository only. `showConflicts` drops a second while one is up,
     * and a monorepo where three roots all conflict would otherwise stack three modal lists over
     * each other; the bar is what carries the rest.
     */
    const first = stopped[0]
    if (first !== undefined) {
      void branchApi.conflicts(project, first.id).then((state) => {
        if (state === null) return
        showConflicts({
          project,
          repo: first.id,
          repoName: repos.length > 1 ? first.name : '',
          state,
        })
      })
    }

    const ask = strategyAsk(asked)
    if (ask === null) return
    requestPullStrategy({
      ask,
      repos: asked,
      proceed: (picked, keep) => {
        // Only the repositories that actually asked. Re-issuing for the ones that already
        // fast-forwarded would pull them a second time and report them twice.
        const again = repos.filter((r) => asked.some((a) => a.repo === r.id))
        pullPass(project, again, pulling, picked, keep)
      },
    })
  })
}

/** `args.path` when the caller supplied one — `args` is `unknown` on the wire. */
function pathArg(args: unknown): string | null {
  if (typeof args !== 'object' || args === null) return null
  const path = (args as { path?: unknown }).path
  return typeof path === 'string' ? path : null
}

/** `args.rev` — a full oid from a log row, for the commands a context menu opens on one. */
function revArg(args: unknown): string | null {
  if (typeof args !== 'object' || args === null) return null
  const rev = (args as { rev?: unknown }).rev
  return typeof rev === 'string' && rev !== '' ? rev : null
}

/**
 * The tag dialog's opener, and the seam `git.tag.new` goes through. (M18)
 *
 * `target` is the commit to tag: a full oid from a log row, or **`null` meaning HEAD** — which
 * is the palette's case and the reason `git.tag.new` earns a row at all when `git.reset`,
 * `git.revert` and `git.cherryPick` do not. `cide_git::tag::create` takes a revspec, so "the
 * commit I am standing on" needs nothing to have been clicked.
 */
export type TagDialogOpener = (target: string | null) => void

/**
 * Registered by whoever mounts the dialog, in the shape `openBranchPopup` has — a module-level
 * opener the dispatcher calls, so the control works in a window whose status bar, tool window
 * or context menu never mounted anything.
 *
 * **Inverted relative to `openBranchPopup`, and only because of who owns which file.**
 * `chrome/BranchSelector.tsx` exports its opener and this module imports it; the tag dialog is
 * being written in `chrome/logActions.ts` in parallel with this, so the registration goes the
 * other way and the import arrow with it. Either direction gives the same guarantee — exactly
 * one opener, reachable from outside React — and this one has the property that the dispatcher
 * compiles and reports sensibly before the dialog exists, rather than failing to resolve an
 * import. Collapse it into a plain import once both halves are in the tree if that reads
 * better; nothing here depends on the indirection.
 *
 * A single slot rather than a listener list. Two dialogs answering one command is two dialogs
 * on screen, and the second registration winning silently is easier to notice than a stack.
 */
let tagDialog: TagDialogOpener | null = null

/**
 * A request that arrived before anything could draw it, held until something can.
 *
 * **This is the whole fix for a real defect, not a convenience.** The only thing that mounts the
 * tag dialog is `gitlog/LogTab.tsx`, and that mounts only while the git tool window is *open* —
 * which it is not on any fresh launch, because the panel's open flag deliberately starts false.
 * So `git.tag.new`, a command the palette lists whenever a project is open, reported "nothing in
 * this window can ask what to call the tag" in the default state, every time. A listed command
 * that does nothing is the exact defect this file's header is written about.
 *
 * The prompt itself cannot simply move somewhere always-mounted: it resolves the commit's short
 * oid and its repository from the *loaded page*, and only the log has one. So the request waits
 * instead, and [`registerTagDialog`] drains it the moment a dialog appears. One slot, not a
 * queue — a second request while one is parked replaces it, because they are the same gesture
 * repeated and the newer target is the one the user just asked for.
 */
let parkedTag: { target: string | null } | null = null

/**
 * Install the opener, and hand it anything that was waiting.
 *
 * Pass `null` on unmount, or the next window's registration leaks this one. Unregistering does
 * **not** re-park: a dialog that was open and went away took the user's answer with it, and
 * re-raising it in the next window that happens to mount one would be a question arriving with
 * no gesture behind it.
 */
export function registerTagDialog(open: TagDialogOpener | null): void {
  tagDialog = open
  if (open === null) return
  const waiting = parkedTag
  parkedTag = null
  if (waiting !== null) open(waiting.target)
}

/**
 * Ask for the tag dialog.
 *
 * `true` when something drew it, `false` when the request was **parked** for whatever mounts one
 * next. The boolean is not decoration and it does not mean "failed": the caller uses it to decide
 * whether it also has to *reveal* the surface that owns the dialog, which is what turns a parked
 * request into a visible one. A caller that ignores it leaves a question nobody ever sees.
 */
export function openTagDialog(target: string | null): boolean {
  if (tagDialog === null) {
    parkedTag = { target }
    return false
  }
  tagDialog(target)
  return true
}

/** Test seam: forget anything parked. Never called by the app. */
export function __clearParkedTag(): void {
  parkedTag = null
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

      case 'pane.detachToWindow': {
        if (target === null || on === null) return unmet(command, 'no focused pane')
        /*
         * A tab's only pane detaches as the *tab* — `layout::take_pane` refuses a last pane
         * (an empty tree is unrepresentable), and this is the same rule that words the
         * corner cluster's button, from the same function, so the chord and the button
         * cannot diverge. See `windows/windowTabs.ts` for why an editor can only leave the
         * shell this way.
         */
        const plan = clusterPlan(Object.keys(target.tab.tree.panes).length, !isClosableTab(boot()))
        if (plan.detach !== 'tab') return void ws.detachPane(on.project, on.tab, on.pane)
        if (boot()?.role.kind === 'detachedTab') {
          return unmet(command, 'this tab is already in a window of its own')
        }
        return void ws.detachTab(on.project, on.tab)
      }

      case 'pane.close': {
        // Refusals — the console's primary pane, unsaved edits — come back from Rust and
        // the store turns the unsaved one into the close confirmation.
        if (target === null || on === null) return unmet(command, 'no focused pane')
        // A tab's only pane closes as the tab, per `clusterPlan` — the rule the cluster's
        // button follows. `layout::close` would refuse it as the last pane; closing the
        // last pane IS closing the tab, and `closeTab` still asks about a live session or
        // an unsaved buffer first.
        const plan = clusterPlan(Object.keys(target.tab.tree.panes).length, !isClosableTab(boot()))
        if (plan.close === 'tab') return void ws.closeTab(on.project, on.tab)
        return void ws.closePane(on.project, on.tab, on.pane)
      }

      case 'pane.evenRow':
        // 'row' rather than the tab's rows: the palette row and the pane menu's item are the
        // same gesture, and the menu's says "row" on it.
        if (on === null) return unmet(command, 'no focused pane')
        return void ws.distributePanes(on.project, on.tab, on.pane, 'row')

      case 'pane.maximize': {
        // A toggle, because the command is reached by one key and one palette row: a
        // maximize with no un-maximize leaves the user with a full-screen pane and no
        // gesture to undo it except finding the pane title bar's button.
        if (target === null || on === null) return unmet(command, 'no focused pane')
        // A tab's only pane already fills it. The cluster withholds the button for the same
        // fact (`clusterPlan`); the chord answering `unmet` is what keeps the two honest.
        if (Object.keys(target.tab.tree.panes).length <= 1) {
          return unmet(command, 'this pane already fills its tab')
        }
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
        if (boot()?.role.kind === 'detachedTab') {
          return unmet(command, 'this window shows one tab')
        }
        // The strip's walk, over the strip's list: a tab torn out into a window of its own
        // keeps its slot in `project.tabs` and has to be stepped over here, or the
        // keystroke lands on a tab this window refuses to draw — `activate_tab` answers
        // success without moving, and the press visibly does nothing.
        const torn = detachedTabs(boot()?.workspace.windows ?? {})
        const next = neighbour(
          project.tabs.filter((tab) => !torn.has(tab.id)).map((tab) => tab.id),
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
              notify('There is no saved conversation for this pane to resume.', { kind: 'warn' })
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
        // The pane's kind, because it decides the no-text branch: a Claude pane hands `^V` to
        // the CLI so its own image paste runs, a shell pane must not (readline's
        // `quoted-insert` would eat the next keystroke). Read off `target`, which carries the
        // whole `Pane`, rather than off `on`, which is the three ids a `pane_*` command takes.
        // Same mapping `TerminalPane` makes — anything that is not the Claude CLI is a shell as
        // far as this chord is concerned.
        void pasteIntoTerminal(term, target?.pane.kind === 'claude' ? 'claude' : 'shell').catch(
          (error: unknown) => void diag.log(`terminal.paste failed: ${String(error)}`),
        )
        return
      }

      case 'terminal.find': {
        /*
         * The find bar over the focused terminal pane. Ctrl+F reaches this the short way — the
         * terminal's own key handler calls `openTerminalFind` directly, because the chord is
         * focus-scoped and never enters the keymap (see `cide_core::commands`' entry for the
         * argument) — so this arm is the palette's route, and a user's own `keymap.json` chord's.
         *
         * Both routes end in the same store write, which is what stops the row in the palette
         * and the keystroke being two different features.
         *
         * A session is deliberately *not* required, unlike `terminal.paste`: a pane whose child
         * has exited still holds its whole transcript, and searching a dead pane's output is one
         * of the times somebody most wants this. What is required is a terminal to search, which
         * a diff or editor pane has none of — the clause says so and this re-checks it, because
         * the gate never reads a `Command::when`.
         */
        if (on === null) return unmet(command, 'no focused pane')
        if (peekHost(on.pane)?.terminal === undefined) {
          return unmet(command, 'the focused pane has no terminal')
        }
        openTerminalFind(on.pane)
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

      case 'editor.format': {
        /*
         * Ctrl+Alt+F. (M26)
         *
         * **The precondition is re-checked here even though the command carries
         * `.when("editorFocused")`.** That clause filters the palette and never gates the
         * keyboard — the key gate resolves through `Binding::when`, a different field — so the
         * chord arriving from a terminal pane reaches this line and must be answered. It is the
         * rule the fold family states below and the one written out at the head of
         * `cide_core::commands`.
         *
         * `focusedFormat()` and not `focusedCaret()`: a split showing one file twice has two
         * selections, and the actions come from the half the user is in. It is also null for a
         * claim made without them — a fixture, a check script — which this refusal covers too.
         *
         * Fire-and-forget: every outcome, including "no formatter for this language" and a
         * formatter's own error text, reports itself through `Failures`. See
         * `editor/formatDocument.ts`.
         */
        const actions = focusedFormat()
        if (actions === null) return unmet(command, 'no editor focused')
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        void formatDocument(project.id, actions)
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
        if (!panelHostPresent() || boot()?.role.kind !== 'shell') {
          notify('This window has no file tree, so there is nothing to select a file in.', {
            kind: 'warn',
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
            kind: 'warn',
            hint: 'Open a file — the Claude console and the settings tab are not files.',
          })
          return
        }
        // Shown before revealed. Revealing into a sidebar that is on Git — or closed —
        // scrolls a tree nobody can see, which is a command that "did nothing" again.
        // The answer is not read here, unlike everywhere else: the refusal above already
        // returned for every window where this could be `false`.
        requestPanel('files')
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
                kind: 'warn',
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
        if (!requestPanel('files')) return unmet(command, 'this window has no sidebar')
        void useFileTree
          .getState()
          .revealGroup(EXTERNAL_LIBRARIES)
          .then((shown) => {
            if (shown) return
            notify('This project has no External Libraries group.', {
              kind: 'warn',
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
        if (!requestPanel('files')) return unmet(command, 'this window has no sidebar')
        void useFileTree
          .getState()
          .revealGroup(SCRATCHES)
          .then((shown) => {
            if (shown) return
            notify('This project has no Scratches group.', {
              kind: 'warn',
              hint: 'The group is keyed by the project\u2019s first root directory.',
            })
          })
        return
      }

      /* --------------------------------------------------------------------------- Git */

      case 'git.push': {
        /*
         * Every repository in the project, in root order. A monorepo with submodules has
         * several and the command names none of them, so pushing "the" repo would have to pick
         * one; pushing each is what *Push to remote* says.
         *
         * # Two things this used to get wrong, both fixed in M20
         *
         * It was `Promise.all` with the value discarded. That is the recurring defect of this
         * project with the sign flipped — not "implemented and nothing calls it" but "called,
         * and the answer dropped on the floor". A user who pressed the key and got silence had
         * the same evidence a dead key would give them. `allSettled` over the *same* promises
         * collects the answers without disturbing the failure path, exactly as `git.pull` does
         * one case below, and for the same two reasons: with four repositories and two
         * failures `Promise.all` reports the first and marks the rest handled.
         *
         * And it had **no `.catch` at all**, so a rejection reached `Failures.tsx` as a raw
         * `GitError` — `{kind: 'push', detail: {output: …}}`, which has no `message` field and
         * whose `detail` is an object, so `notices.describe` fell through to `kind` and the
         * toast read the single word `push`. `explain` is the sentence.
         */
        withRepos(command, (project, repos) => {
          const attempts = repos.map((repo) => gitApi.push(project, repo.id, null, null))
          for (const attempt of attempts) {
            void attempt.catch((error: unknown) => {
              throw new Error(explain(error))
            })
          }
          void Promise.allSettled(attempts).then((settled) => {
            const done: RepoPush[] = []
            settled.forEach((result, i) => {
              const repo = repos[i]
              if (result.status === 'fulfilled' && repo !== undefined) {
                done.push({ name: repo.name, outcome: result.value })
              }
            })
            if (done.length === 0) return
            // Aggregated into one notice, never one per repository: `notices.admit` collapses
            // toasts by identical text, so five submodules all saying "already up to date"
            // would show one toast that silently spoke for five.
            const report = pushReport(done)
            notify(report.text, { kind: 'ok', detail: report.detail })
          })
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
        /*
         * # The divergence, which is a question and not a failure (M20)
         *
         * A `pullNeedsStrategy` rejection is **swallowed** in the loop below rather than
         * rethrown, so its derived promise resolves and no `unhandledrejection` fires for it.
         * Every other rejection keeps the property the paragraphs above depend on. The
         * `allSettled` pass then collects those questions the same way it collects successes,
         * and asks **once** for all of them — see `chrome/pullStrategyStore.ts` for why one
         * question rather than a queue.
         *
         * The retry passes `skipFetch`, because the counts and commits the user has just read
         * describe refs that are already on disk: re-fetching could only make them answer a
         * question about one divergence and get another.
         *
         * And the ask is built **only on the first pass** (`strategy === null`). A
         * `pullNeedsStrategy` arriving on the retry — which Rust must not produce, but might —
         * falls through to `explain` and toasts once, rather than putting the dialog back on
         * screen for ever. Same reasoning as `logMenu.ts`'s mainline guard.
         */
        const pulling = command === 'git.pull'
        withRepos(command, (project, repos) => {
          pullPass(project, repos, pulling, null, false)
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
         *
         * The reveal goes through `chrome/panelRequests.ts` rather than a `showSidebar` dep, so
         * that this arm and the git log's *Amend…* — which cannot reach a dep at all — reveal
         * the panel by the same call. `requestAmend` in that module is the log's entry point and
         * it ends in the same `showPanel`; two routes to one panel is how the reveal here and
         * the reveal there come to disagree about, say, which tab of the panel is showing.
         */
        if (!requestPanel('git')) return unmet(command, 'this window has no sidebar')
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

      /* ------------------------------------------------ Git: the bottom tool window (M18) */

      case 'git.log': {
        if (boot()?.role.kind !== 'shell') {
          return unmet(command, 'this window has no tool window')
        }
        /*
         * Through `withRepos`, not a bare open, and that is the difference between this and a
         * panel that draws an empty list. `withRepos` asks `git_repos` — real discovery on the
         * disk, the only honest source, because whether a project *holds* a repository is a fact
         * a `git init` in a bash pane changes — and throws a sentence into `chrome/Failures.tsx`
         * when the answer is empty. So a project with no repository is told, rather than shown a
         * blank panel it has to interpret. This is also why no `when` clause claims to know:
         * that was the `repoOpen` mistake.
         */
        withRepos(command, (project) => {
          void toolWindowApi.activate(project, null).catch(() => {})
        })
        return
      }

      case 'git.history.file': {
        if (boot()?.role.kind !== 'shell') {
          return unmet(command, 'this window has no tool window')
        }
        /*
         * `args.path` wins so the four context menus name the file they were opened on;
         * `focusedTabPath` is the fallback for the palette row. Same order and same two
         * functions as `file.reveal`.
         *
         * The path arrives **absolute** — that is what the tab strip, the file tree and the
         * editor all have — and `git_locate` turns it into `(repo, repo-relative)` in Rust. Not
         * a prefix comparison here: a symlinked root or a nested submodule makes that silently
         * wrong, which is the rule `TreeStatusMap` already states.
         */
        const path = pathArg(args) ?? focusedTabPath(boot())
        if (path === null) {
          return unmet(command, 'no file tab is active')
        }
        withRepos(command, (project) => {
          void historyApi
            .locate(project, path)
            .then((found) => {
              if (found === null) {
                notify('That file is not inside any repository in this project.', {
                  kind: 'warn',
                })
                return
              }
              return toolWindowApi.openHistory(project, found.repo, found.path).then(() => {})
            })
            .catch(() => {})
        })
        return
      }

      case 'git.blame': {
        /*
         * No `shellWindow` guard and no `withRepos`: the gutter is per *buffer*, a detached-pane
         * window can hold one, and the blame fetch reports "not in a repository" with the file's
         * own name in it — a better sentence than the generic one this command could form before
         * knowing which file it is about.
         *
         * `focusedFilePath`, not `focusedTabPath`: annotating a *diff* would have to answer
         * "which side?", and nothing has asked.
         */
        const path = pathArg(args) ?? focusedFilePath(boot())
        if (path === null) return unmet(command, 'no file editor is focused')
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        toggleBlame(project.id, path)
        return
      }

      case 'git.tag.new': {
        /*
         * `git.branch.new`'s shape exactly: open the control that asks, do not guess.
         *
         * A tag needs a name, and a name is not something a palette row can supply — the same
         * argument `git.commit` and both branch commands make. What makes this one *listable*
         * where `git.reset`, `git.revert` and `git.cherryPick` are not is the other half: those
         * three need a **commit** with no honest default, and this one has one. `null` is HEAD,
         * `TagRequest.target` takes a revspec, and *tag where I am standing* is the common case.
         * `args.rev` is what the log's context menu passes when the gesture started on a row.
         *
         * No repository check and no `withRepos`, deliberately and for `git.branch.new`'s stated
         * reason: the dialog asks `git_repos` itself and draws its own "no repository" state, so
         * a guard here would be a second opinion about the disk formed one round trip earlier.
         *
         * No `shellWindow` guard either, for `git.blame`'s reason: the dialog is an overlay and
         * any window can raise one.
         *
         * # Why this reveals the tool window, and why that is not a side effect
         *
         * The only thing that mounts the dialog is `gitlog/LogTab.tsx`, and that mounts only
         * while the tool window is **open** — which it is not on a fresh launch, because the
         * panel's open flag starts false on purpose. So this command used to report "nothing in
         * this window can ask what to call the tag" in the *default* state, every time: a listed
         * palette row that did nothing, which is the exact defect this file's header is about.
         *
         * `openTagDialog` now **parks** the request when nothing is mounted, and
         * `registerTagDialog` drains it the moment something is. So the `false` arm is not a
         * failure — it means "held" — and all this has to do is make something mount. Revealing
         * the log is the honest way to do that: it is the surface the dialog belongs to, it is
         * where the tag will show up as a ref chip the moment it exists, and the alternative is
         * a question that never appears.
         *
         * A detached-pane window has no tool window to reveal, so there the request stays parked
         * and the notice below is the truthful answer rather than a fallback.
         */
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        if (openTagDialog(revArg(args))) return
        if (boot()?.role.kind !== 'shell') {
          notify('Tagging needs the Git log, which this window does not have.', {
            kind: 'warn',
            hint: 'Tag from a commit row in the main window’s Git tool window.',
          })
          return
        }
        // Parked. Open the panel on its Log tab; `LogTab` registers on mount and drains it.
        void toolWindowApi
          .setLayout(project.id, { open: true })
          .then(() => toolWindowApi.activate(project.id, null))
          .catch(() => {
            notify('Could not open the Git log to ask what to call the tag.', { kind: 'error' })
          })
        return
      }

      case 'view.toolWindow.toggle': {
        if (boot()?.role.kind !== 'shell') {
          return unmet(command, 'this window has no tool window')
        }
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        const open = project.toolWindow.open
        void toolWindowApi.setLayout(project.id, { open: !open }).catch(() => {})
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
      /*
       * The project's notes: the pinned row's double-click, its context menu, and the palette
       * row, all arriving here.
       *
       * Three calls in a fixed order, and each one is load-bearing:
       *
       *   1. `notesEnsure` — creates the file and its directory on the **first** call and
       *      answers the path. It never truncates, so the second and hundredth clicks are safe;
       *      the guarantee lives in `cide_core::notes::ensure` and is the one line in this
       *      feature that could lose data.
       *   2. `file.open` — an **ordinary** file tab. That is the whole reason the feature is one
       *      command rather than a new tab kind: save, the dirty marker, undo, find-in-file and
       *      the markdown grammar all work because nothing about this tab is special.
       *   3. `reveal` of the pin's sentinel — **not** `revealGroup`, which issues an
       *      `fs_expand` a pin has nothing to answer. `reveal` scrolls to the row and selects
       *      it, which is what tells the user where the thing they just opened lives.
       *
       * The refusals are `unmet` rather than notices, the same deliberate difference
       * `scratch.new` below draws: these are "this window is not the one with a project and a
       * file tree", which the palette already hides the row for and which a detached-pane window
       * is in permanently — a notice there would fire on every stray keystroke in a torn-out
       * terminal.
       *
       * Deliberately uncaught, like every other command call in this file: `chrome/Failures.tsx`
       * turns a rejection into the sentence that says why. And deliberately no `jumpTo` — the
       * same reasoning `createScratch` states in `App.tsx`: a notes file has no place in it to
       * navigate to, and recording a Back stop on it would put a history entry on nothing.
       */
      case 'file.projectNotes': {
        if (boot()?.role.kind !== 'shell') return unmet(command, 'this window has no file tree')
        const notesProject = activeProjectOf(boot())
        if (notesProject === null) return unmet(command, 'no open project')
        void fsApi.notesEnsure(notesProject.id).then(async (path) => {
          await fileApi.open(notesProject.id, path)
          await useWorkspace.getState().hydrate()
          // Unread, as the `?.` before it was: the role check at the top of this arm has
          // already refused every window where nothing can reveal a panel.
          requestPanel('files')
          await useFileTree.getState().reveal(groupPath(PROJECT_NOTES))
        })
        return
      }

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
        //
        // The redirect is passed in rather than living inside `goToDefinition`, and it is spent
        // only when the answer landed on a method inside an interface — the Go case where the
        // declaration is not what anyone meant. `goToImplementation` falls back to the plain
        // declaration when nothing implements it, which is both the right answer and what stops
        // the two functions calling each other for ever.
        goToDefinition(project.id, caret.path, caret.line, caret.column, () =>
          goToImplementation(project.id, caret.path, caret.line, caret.column, focusedWord()),
        )
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

      case 'navigate.implementation': {
        /*
         * Ctrl+Alt+B — the concrete thing, as opposed to the declaration. (M18)
         *
         * Same shape as `navigate.definition` and `navigate.usages` above and for the same
         * reason: a `when` gates the palette and never the keyboard, so the chord arriving from a
         * terminal pane has to be refused with a sentence rather than run against a caret that
         * does not exist.
         *
         * Ctrl+B still means "definition" everywhere except one position: a definition that
         * landed on a method inside an interface now redirects here, because that is the case
         * the report was about and the declaration is not what anyone meant by it. The test is
         * the *target*, parsed — never the language — so `implementation` is not folded into
         * `definition` for a struct or a trait or an interface type name, which is where doing
         * so would regress Rust. `cide_core::commands` has the whole argument.
         *
         * An empty answer falls back to Go to definition inside `goToImplementation`, so this
         * command always does something wherever a caret is.
         */
        const caret = focusedCaret()
        if (caret === null) return unmet(command, 'no editor focused')
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        goToImplementation(project.id, caret.path, caret.line, caret.column, focusedWord())
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
        /*
         * Awaited now, because one refusal is only known after a round trip: a Back into a file
         * that has been deleted since answers `gone`, and the sentence naming it is worth as
         * much as the empty-history one — more, since a tab reading "This file could not be
         * opened" with no path in it was what the user used to get instead.
         *
         * `void` with a `.then` rather than making this arm `async`: the dispatcher is
         * synchronous by design — it decides whether a key was consumed, and a promise cannot
         * answer that in time for `preventDefault`.
         */
        void navigate(command === 'navigate.back' ? 'back' : 'forward').then((refusal) => {
          if (refusal !== null) {
            notify(refusal, { kind: 'warn' })
          }
        })
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

      /* ----------------------------------------------------------------------- Folding */

      case 'editor.fold':
      case 'editor.unfold':
      case 'editor.toggleFold':
      case 'editor.foldAll':
      case 'editor.unfoldAll':
      case 'editor.foldRecursively':
      case 'editor.unfoldRecursively': {
        /*
         * Folding is entirely client-side — no IPC, no Rust — so the whole of this arm is
         * finding the editor. `focusedFolds()` is `caretTrack.ts`'s claim stack, which is the
         * same slot `navigate.line` and ⌥F7 read; the module note there says why folding rides
         * on it rather than on a registry of its own.
         *
         * **The precondition is re-checked here even though every one of these commands carries
         * `.when("editorFocused")`.** That clause filters the palette and never gates the
         * keyboard — the key gate resolves through `Binding::when`, a different field — so a
         * chord arriving with no editor focused reaches this line and must be answered. Leaving
         * that out is how Ctrl+W on the pinned console came to raise a dialog and then fail; the
         * rule is written out at the top of `cide_core::commands`.
         */
        const folds = focusedFolds()
        if (folds === null) return unmet(command, 'no editor focused')
        const did =
          command === 'editor.fold'
            ? folds.fold()
            : command === 'editor.unfold'
              ? folds.unfold()
              : command === 'editor.toggleFold'
                ? folds.toggle()
                : command === 'editor.foldAll'
                  ? folds.foldAll()
                  : command === 'editor.unfoldAll'
                    ? folds.unfoldAll()
                    : command === 'editor.foldRecursively'
                      ? folds.foldRecursively()
                      : folds.unfoldRecursively()
        /*
         * A refusal is reported rather than swallowed, for the reason `unmet` exists: a caret in
         * a file with nothing foldable under it, or an Expand with nothing collapsed, is a
         * keystroke that did nothing — and a keystroke that does nothing *and says nothing* is
         * indistinguishable from a command that is not wired, which is the complaint this whole
         * dispatcher was rewritten to answer.
         */
        if (!did) {
          return unmet(
            command,
            command.startsWith('editor.unfold') || command === 'editor.toggleFold'
              ? 'nothing is collapsed here'
              : 'nothing to collapse here',
          )
        }
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

      case 'picker.libraries':
        /*
         * Widen — or narrow — the file picker's scope.
         *
         * It flips a flag and nothing else. Starting the walk is `FilePicker`'s, on an effect
         * keyed to the flag, and deliberately: the walk needs a project id and the overlay has
         * one, this does not, and a second place that could start a `cargo metadata` is a second
         * place to get "at most once per project" wrong.
         *
         * Reachable two ways, which is the point of it being a command at all. From the keyboard
         * it is ⌥L while the picker has focus (`filePickerOpen`), and from the palette it sets
         * the scope for the *next* Ctrl+P — the two overlays cannot be open at once, so from
         * there it can only ever be a preparation, and that is a useful thing to be able to do.
         */
        useOverlays.getState().toggleLibraries()
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
        if (!requestPanel('files')) return unmet(command, 'this window has no sidebar')
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
        if (!requestPanel('git')) return unmet(command, 'this window has no sidebar')
        return

      case 'problems.refresh': {
        /*
         * Re-run the analysers. The Problems panel's own button calls the same function. (M18)
         *
         * `projectOpen` is the command's clause and it is re-checked here, per this file's rule:
         * the clause filters the palette, the keyboard is never gated by it, and an id a user has
         * bound in `keymap.json` arrives whatever the workspace looks like.
         *
         * Deliberately **not** gated on the sidebar being open or on the Problems view being
         * selected. The thing being re-run is a language server, not a panel, and a user who
         * pressed a chord they bound for this while looking at a terminal means exactly what they
         * said. `sidebar/ProblemsPanel/actions.ts` puts the outcome on screen either way.
         */
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        refreshDiagnostics(project.id)
        return
      }

      case 'problems.invalidateCaches': {
        /*
         * Stop every analyser, delete every server's on-disk cache, start them again. The
         * heavyweight sibling of `problems.refresh` above, and deliberately fire-and-forget
         * from here: the call blocks server-side for as long as the shutdown ladders take,
         * and the Problems panel's own status rows ("Scanning…") are the progress surface —
         * the same one a plain restart uses. Global on purpose — the caches are per server,
         * not per project — so no `project` argument, but the clause still gates the palette
         * row: with nothing open there is nothing to rebuild into.
         */
        if (activeProjectOf(boot()) === null) return unmet(command, 'no open project')
        void diagnostics.invalidateCaches()
        return
      }

      case 'sidebar.problems':
        // Reveal, never toggle — the same call `sidebar.files` and `sidebar.git` make, and for
        // the reason spelled out under `sidebar.search`: a panel is not an overlay, the ⚑ button
        // is right there, and a second press that closed it would take away the thing the
        // binding is for.
        if (!requestPanel('problems')) return unmet(command, 'this window has no sidebar')
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
        /*
         * Pressed with the file tree focused, the chord also points the panel at the selected
         * folder — a file row meaning its parent. That is the *only* thing focus changes here;
         * from anywhere else this arm does exactly what it always did.
         *
         * The focus test is at runtime rather than in a `when` clause, and `treeFocus.ts`'s
         * header is the argument: a `Command::when` does not gate the keyboard at all, and
         * gating is not wanted anyway — the command must keep working everywhere. The tree's
         * other focus-scoped chords live on its scroller instead, which this one cannot,
         * because the gate is a window *capture* listener and `ctrl+shift+f` is bound, so the
         * scroller's `onKeyDown` never runs for it.
         *
         * An existing scope is deliberately **left alone** when the press comes from anywhere
         * else. A chord that silently widened a search back to the whole project would be
         * worse than one that does not narrow it — the box is on screen with an ✕ beside it,
         * and the panel opens with the boxes shown whenever either holds something.
         */
        if (treeFocused()) {
          const scope = scopeFromTree()
          if (scope !== null) applyScope(scope)
        }
        if (!requestPanel('search')) return unmet(command, 'this window has no sidebar')
        requestFocus('search')
        return

      /*
       * Pause and resume, at the **project scope**: `agents_pause` / `agents_resume` take a
       * nullable run and these two call them with `null`, which shuts the dispatch queue, freezes
       * every live run *and* freezes the project's own console session. `client.ts` and
       * `AgentRegistry::pause` carry the argument for one command with two scopes rather than
       * four; the per-run halves are the ⏸ and ▶ on each Agents-panel row.
       *
       * **`agents.resume` is the way out of the freeze, not a convenience.** Pause-all stops the
       * pane the user types in, so a resume reachable only from that pane is no resume at all.
       * The palette is chrome rather than a pane, so it keeps working while every session in the
       * project is stopped — which is why these two were registered together with the panel's
       * per-run controls rather than after them.
       *
       * # The clause is `projectOpen` alone, and the handler asks the rest
       *
       * Whether subagents are enabled is a fact only `.cide/config.json` knows. A
       * `subagentsEnabled` context flag would have to be supplied by the webview, the workspace
       * mirror cannot derive it, and a flag nobody sets is false for ever — the `repoOpen`
       * failure `cide_core::commands` spends fifty lines on. So the question is asked over IPC,
       * here, at the gesture, and answered with a sentence.
       *
       * The refusal **throws into an uncaught promise chain** rather than calling `unmet`, which
       * is `withRepos`'s rule at the top of this file and the same shape: `unmet` writes to the
       * diag log, which is right for "this command's clause and this handler disagree" and wrong
       * for "subagents are off for this project" — that is a fact about the user's repository,
       * they just asked a question about it, and they are owed a sentence. `chrome/Failures.tsx`
       * turns the rejection into one.
       *
       * # Only *pause* asks, and the asymmetry is deliberate
       *
       * A resume can do nothing but undo a freeze: it thaws what is stopped, costs no quota and
       * starts no child, and with nothing frozen it is a no-op the registry answers `Ok` to. A
       * pause is the one that acts — and in a project with subagents off, the only thing the
       * project scope would find to freeze is *the user's own console*, which is why the switch
       * is checked before it, in the direction where being wrong stops something.
       *
       * Gating the resume on the same switch would be worse than redundant. `.cide/config.json`
       * is a committed file, so a `git checkout` between the freeze and the thaw can flip it, and
       * a resume that refused on the strength of it would leave a stopped console with no control
       * anywhere that could start it again. That is exactly the state this pair exists to keep
       * unreachable.
       */
      case 'agents.pause':
      case 'agents.resume': {
        const project = activeProjectOf(boot())
        if (project === null) return unmet(command, 'no open project')
        if (command === 'agents.resume') {
          // Uncaught on purpose: the registry refuses a resume with a sentence (it has none of
          // its own for an empty thaw, which is a legitimate no-op) and `Failures.tsx` shows it.
          void agentRunsApi.resume(project.id)
          return
        }
        void agentsApi.config(project.id).then((config) => {
          if (config === null) {
            // `agents.config` goes through `pendingCommand`, so `null` is "this build could not
            // answer" — a stale binary against a hot-reloaded frontend, the shape `Failures.tsx`
            // was written for. Its notice carries the stale-binary hint automatically.
            throw new Error('cide could not read this project’s subagent settings.')
          }
          if (!config.enabled) {
            throw new Error(
              'Subagents are off for this project, so there is nothing to pause. Turn them on ' +
                'in the Agents panel — it writes .cide/config.json into your repository.',
            )
          }
          return agentRunsApi.pause(project.id)
        })
        return
      }

      /*
       * M18's two panels, routed exactly like `sidebar.git` above: reveal, never toggle, and
       * `unmet` in a window that has no rail to switch.
       *
       * Neither arm asks whether subagents are enabled for the project, and neither ever will.
       * That is a fact only `.cide/config.json` knows, the command's clause deliberately does
       * not name it (see the `AGENTS` block in `cide-core::commands`), and a *panel* is the one
       * surface that must open regardless: it is where the answer is shown. Refusing to open the
       * panel that would have explained the feature is off is the shape of failure this project
       * keeps finding.
       */
      case 'sidebar.agents':
        if (!requestPanel('agents')) return unmet(command, 'this window has no sidebar')
        return

      /*
       * Two ids, one arm, and the duplication is on purpose.
       *
       * The palette wants a verb — *Go to tasks* is what a user types when they want to be
       * looking at the board — and the sidebar group wants a noun that reads beside *Show git
       * sidebar* and *Show problems sidebar*. **Ids are API**: a user's `keymap.json` names
       * them, so neither can be renamed into the other later, and shipping one id under two
       * titles is not possible (`no_two_commands_share_a_title` forbids it, because two palette
       * rows a user cannot tell apart is the defect it guards). So both exist, they mean the
       * same thing today, and they share a body rather than drifting into two spellings of one
       * gesture. `task.focusBoard` is the one that will grow — focusing a row, or the New task
       * field — at which point it stops falling through and this comment gets shorter.
       */
      case 'task.focusBoard':
      case 'sidebar.tasks':
        if (!requestPanel('tasks')) return unmet(command, 'this window has no sidebar')
        return

      case 'sidebar.extensions':
        if (!requestPanel('extensions')) return unmet(command, 'this window has no sidebar')
        return

      default:
        /*
         * A command an extension contributed. (M22)
         *
         * One arm with a prefix test, and it is what keeps `check:commands`' rule true for a set
         * that does not exist at build time: *every id is either handled here or carries an
         * `unavailable` reason*, and the alternative — listed-and-silently-inert — is the state
         * that once described 24 of 37 commands.
         *
         * `ext.<marketplace>.<extension>.<id>`, built by `cide_ext::manifest::command_id` and
         * refused at install unless every segment is a shape that can be a stable prefix. Split
         * on the first three dots and nothing else: the extension's own id may contain dots,
         * because `cide-core`'s builtin ids do (`sidebar.git`, `file.reveal`) and an author will
         * reach for that on the first try.
         *
         * A worker that is not running answers `false`, and that becomes an `unmet` the palette
         * shows — rather than a command that appears to succeed and does nothing, which is the
         * failure this whole arm exists to avoid.
         */
        if (command.startsWith('ext.')) {
          const parts = command.slice('ext.'.length).split('.')
          const marketplace = parts[0]
          const extension = parts[1]
          const id = parts.slice(2).join('.')
          if (marketplace === undefined || extension === undefined || id === '') {
            return unmet(command, 'that extension command id is malformed')
          }
          if (!invokeExtension({ marketplace, extension }, id)) {
            return unmet(command, 'that extension is not running')
          }
          return
        }
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
