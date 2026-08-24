/**
 * The frontend's mirror of the Rust-owned workspace.
 *
 * A *mirror*, not a copy with opinions: nothing here mutates the tree. Every change goes
 * to Rust as a command and comes back as a new snapshot, because the domain has to survive
 * a window closing and a second window has to see the same state. Local writes would give
 * two windows two answers.
 *
 * The only genuinely local state is transient chrome: which overlay is open, whether a
 * splitter is mid-drag. Those never outlive the window and never need to agree with anyone.
 */
import { registerLanguages } from '@/editor/languages'
import { rememberSpawnPlan } from '@/layout/spawnPlans'
import { reconcile, restack } from '@/keys/switcher'
import { windowProjectsOf } from '@/keys/target'
import { useMemo } from 'react'
import { create } from 'zustand'
import { destroyHost, peekHost, releaseHost } from '@/layout/paneHosts'
import { requestCloseConfirm } from '@/chrome/closeConfirmStore'
import type { CloseScope } from '@/chrome/closeConfirmModel'
import { planFileIndex, type IndexTarget } from './fileIndex'
import {
  app as appApi,
  events,
  fs as fsApi,
  pane as paneApi,
  pendingCommand,
  session as sessionApi,
  unsavedChanges,
  windows as windowApi,
  project as projectApi,
  tab as tabApi,
  type Axis,
  type Bootstrap,
  type Direction,
  type PaneId,
  type Project,
  type ProjectId,
  type SessionId,
  type SessionSummary,
  type Side,
  type SplitId,
  type SplitIntent,
  type SplitOutcome,
  type Tab,
  type TabId,
  type UnsavedTab,
  type WindowMode,
  type Workspace,
} from '@/ipc/client'

export type Theme = 'dark' | 'light'

/** Resolve once React has committed and the browser has laid out against it. */
function nextFrame(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

/**
 * Run a close command, turning the domain's unsaved-work refusal into a confirmation.
 *
 * Returns true when the close was *not* performed and a dialog is now up; false when it
 * went through. Any other failure re-throws — a close that fails for an unrelated reason is
 * not something to swallow behind a dialog about unsaved files.
 *
 * A shared helper rather than a copy in each close path: the two are one sentence apart and
 * the difference between them is the scope word, so a copy would eventually differ in
 * whether it re-throws — and the version that swallows loses errors silently.
 */
async function refused(
  scope: CloseScope,
  run: () => Promise<unknown>,
  proceed: () => Promise<void>,
): Promise<boolean> {
  try {
    await run()
    return false
  } catch (e) {
    const unsaved = unsavedChanges(e)
    if (!unsaved) throw e
    requestCloseConfirm({ scope, unsaved, sessions: [], proceed })
    return true
  }
}

/**
 * What closing one tab would cost, asked before the command rather than after it.
 *
 * Answers `{ unsaved: [], sessions: [] }` without an IPC call when the tab has no session
 * bound at all, which is every file tab that has not been split and every diff tab. In that
 * case the unsaved half is left to Rust's refusal, which already names the file — paying a
 * round trip to be told the same thing on every `×` is the kind of cost that gets a guard
 * deleted later.
 *
 * When there *is* a session bound the round trip happens anyway, and then **both** halves
 * come back from it. That matters: a file tab split to hold a Claude pane can be dirty and
 * mid-turn at the same time, and asking only about the session produced a dialog that said
 * nothing about the buffer and a "Close anyway" that discarded it. The user opted in to
 * losing a turn, not to losing their edits.
 *
 * The session ids come from the local mirror and the *states* from Rust, because only the
 * hook server knows which are mid-turn — and `app.quitRequested` already applies the
 * `confirmCloseWithLiveSession` setting, so the session half inherits it. The unsaved half
 * deliberately does not; see `Settings::confirm_close_with_live_session` in Rust.
 */
async function tabCloseRisk(
  boot: Bootstrap | null,
  project: ProjectId,
  tab: TabId,
): Promise<{ unsaved: UnsavedTab[]; sessions: SessionSummary[] }> {
  const nothing = { unsaved: [], sessions: [] }
  const target = boot?.workspace.projects[project]?.tabs.find((t) => t.id === tab)
  if (!target) return nothing
  const bound = new Set(
    Object.values(target.tree.panes)
      .map((pane) => pane.session)
      .filter((session): session is SessionId => session !== null),
  )
  if (bound.size === 0) return nothing

  const decision = await appApi.quitRequested(project)
  return {
    // Narrowed to this tab: the decision answers for the whole project, and listing a dirty
    // file from a tab that is not closing would be a dialog about work that is not at risk.
    unsaved: decision.unsaved.filter((u) => u.tab === tab),
    sessions: decision.blocking.filter((s) => bound.has(s.session)),
  }
}

/**
 * What closing **several** tabs would cost, asked once for the whole gesture.
 *
 * Not `tabCloseRisk` in a loop, and the difference is the entire point of this function. That
 * one short-circuits to "nothing" when a tab has no session bound, deliberately, so a single `×`
 * does not pay a round trip to be told what Rust's refusal would say anyway. Run over eight
 * tabs, that produces eight independent refusals, which `closeConfirmStore` queues into eight
 * dialogs for a gesture the user made once — the behaviour *Close others* and *Close to the
 * right* shipped with, and which the queue was built to make survivable rather than to endorse.
 *
 * So this asks `quitRequested` **once** and narrows the answer to the tabs actually closing. One
 * round trip for the gesture, where the loop paid one per tab that had a session bound and none
 * for the rest. That is cheaper whenever the answer matters and one trip dearer when nothing is
 * at stake, which is the right way round: the trip nobody needed is a millisecond, and the
 * dialogs it replaces were the user's afternoon.
 *
 * The narrowing is by tab id on the unsaved half and by bound session on the other, exactly as
 * the single-tab version does — a dirty file in a tab that is *not* in this batch is not at risk
 * and must not be listed, or the user is asked to discard work that was never going anywhere.
 */
async function tabsCloseRisk(
  boot: Bootstrap | null,
  project: ProjectId,
  tabs: readonly TabId[],
): Promise<{ unsaved: UnsavedTab[]; sessions: SessionSummary[] }> {
  const nothing = { unsaved: [], sessions: [] }
  const open = boot?.workspace.projects[project]?.tabs
  if (!open || tabs.length === 0) return nothing

  const closing = new Set<TabId>(tabs)
  const bound = new Set<SessionId>()
  for (const tab of open) {
    if (!closing.has(tab.id)) continue
    for (const pane of Object.values(tab.tree.panes)) {
      if (pane.session !== null) bound.add(pane.session)
    }
  }

  const decision = await appApi.quitRequested(project)
  return {
    unsaved: decision.unsaved.filter((u) => closing.has(u.tab)),
    sessions: decision.blocking.filter((s) => bound.has(s.session)),
  }
}

/**
 * Which projects this window has asked Rust to index, and over which roots.
 *
 * Module scope rather than store state, for the reason `treeStore`'s `inFlight` is: nothing
 * renders from it, and putting it in the store would make every `fs.index` a state update
 * and so a re-render of the whole shell.
 */
let indexedProjects = new Map<string, string>()

/**
 * Point the Rust-side file index at whatever projects the snapshot says are open.
 *
 * This is the call nothing was making. `fs.index` is what fills the tree and the picker, and
 * every `fs_*` and `picker_query` handler answers `NoIndex` until it has run — so before
 * this existed, the explorer showed zero rows and Ctrl+P showed an empty list, for ever, in
 * a build where all fourteen handlers were registered and working.
 *
 * Driven from the snapshot rather than from `openProject`, and the difference is not
 * cosmetic. `project.open` is only one of the ways a project comes to be open in this
 * window: a workspace restored from disk at launch has projects nobody opened this session,
 * and a project opened in a *second* window arrives here only as `cide://workspace-changed`.
 * Hanging the call off `openProject` would have left both of those cases exactly as broken
 * as they were.
 *
 * `Explorer.tsx` was the alternative and it loses on the same argument: it is handed one
 * project — this window's active one — so a second project in the same window would go
 * unindexed until the user switched to it, a detached-pane window would index nothing, and
 * the picker would depend on the sidebar being mounted.
 *
 * Every window runs this against the same workspace, so `fs.index` is called once per window
 * per project. That is deliberate and handled on the Rust side rather than here: `fs.index`
 * over roots that have already been walked is a no-op that answers with the existing index's
 * status, whether the first walk is still running or finished long ago (`FsRegistry::claim`),
 * and `fs.close` is idempotent. Suppressing it here instead would mean deciding which window
 * "owns" a project, which nothing else in this app has to decide.
 *
 * The *finished* half of that guarantee is the one this window depends on and the one that
 * was missing: `indexedProjects` is module state in one webview, so a second window — and a
 * detached pane is a window — starts with an empty map and asks for every open project long
 * after the first window's walk is done. A re-walk there is not merely wasted work; the walk
 * begins by clearing the matcher and dropping the watcher, so it would empty the *first*
 * window's Ctrl+P and stop its file events. `cmd::fs::tests` pins both halves.
 */
function syncFileIndex(workspace: Workspace): void {
  const open: IndexTarget[] = Object.values(workspace.projects).map((project) => ({
    project: project.id,
    roots: project.roots.map((root) => root.path),
  }))
  const plan = planFileIndex(indexedProjects, open)
  indexedProjects = plan.known

  // Not awaited: the walk is seconds of work on a large repository and the whole design is
  // that the window keeps painting while it runs. `pendingCommand` is what keeps a build
  // without these handlers — or a project whose roots have gone — from raising an unhandled
  // rejection out of a snapshot handler.
  for (const project of plan.index) {
    void pendingCommand('fs_index', () => fsApi.index(project), null)
  }
  for (const project of plan.close) {
    void pendingCommand('fs_close', () => fsApi.close(project), false)
  }
}

/* ----------------------------------------------------------------------- the MRU stacks */

/**
 * Where the project MRU order lives, and why it is not in Rust.
 *
 * Ctrl+` walks *most-recently-used* order (`keys/switcher.ts`), so something has to
 * remember the order the user visited projects in. There were two places it could go and the
 * choice is not a toss-up:
 *
 * * **The workspace** — persisted in `workspace.json`, shared by every window. Rejected. It
 *   is not domain state: `Workspace` is the tree two windows must *agree* on, and "the order
 *   I visited things in" is a fact about one window's user. Putting it there means every
 *   Ctrl+Tab bumps `rev` and broadcasts `cide://workspace-changed` to every window — a
 *   keystroke that repaints somebody else's screen — and it means a new field on `Project` or
 *   `Workspace`, a schema migration, and a `project_touch` command, for a list of strings.
 * * **The window** — which is what this is. Module-scope state in one webview, mirrored to
 *   `localStorage` so it survives a quit.
 *
 * The `localStorage` half is not optional and is the reason "the window" is not the wrong
 * answer: a stack that resets every launch makes the first Ctrl+Tab of every session land on
 * whatever happens to be second in the header, which is precisely the header-order behaviour
 * this replaced. `chrome/SidebarSplitter.tsx` already keeps per-window durable chrome the same
 * way, and the ids are stable — `ProjectId` is persisted in `workspace.json`, so an id written
 * here is still the same project after a restart.
 *
 * Two honest limits, stated rather than hidden:
 *
 * * `localStorage` is per *origin*, so **every** window of this app shares this one key — the
 *   detached tab and pane windows included, and they are not theoretical. That is what
 *   [`restack`] guards: a window whose strip holds fewer than two projects reads as "everything
 *   closed" and would write that emptiness over the shell window's order. It is therefore not
 *   allowed to write at all. In `PerProject` mode that silences every window, which costs
 *   nothing because each strip holds one project and Ctrl+Tab has nothing to walk there.
 * * A stale id for a project that has since been closed is dropped by `reconcile` on the first
 *   snapshot a window with a walkable strip sees, not persisted forward.
 */
const MRU_CACHE_KEY = 'cide.projectMru'

/** The remembered stack, or `[]` when there is nothing readable there. */
function loadMru(): string[] {
  try {
    const raw = globalThis.localStorage?.getItem(MRU_CACHE_KEY)
    if (raw === null || raw === undefined) return []
    const parsed: unknown = JSON.parse(raw)
    // Validated rather than cast: this is data from disk that a user can edit, and a
    // malformed entry would otherwise reach `project_activate` as a project id.
    if (!Array.isArray(parsed)) return []
    return parsed.filter((id): id is string => typeof id === 'string')
  } catch {
    // A quota error, a private-mode throw, or a half-written line. An empty stack degrades to
    // header order on the first press, which is worse than the feature and better than a
    // window that fails to boot over a cache.
    return []
  }
}

function saveMru(order: readonly string[]): void {
  try {
    globalThis.localStorage?.setItem(MRU_CACHE_KEY, JSON.stringify(order))
  } catch {
    // Same argument as above, and the write is the half that can genuinely fail on quota.
  }
}

/**
 * The stack this snapshot implies: closed projects dropped, new ones added, the active one at
 * the front.
 *
 * Snapshot-driven rather than hung off `activateProject`, and that is the whole reason it is
 * one function. A project becomes the active one in four ways — opened, clicked in the header,
 * committed from the switcher, or activated in *another* window — and only three of them pass
 * through this store's actions. `cide://workspace-changed` is the one path all four share.
 *
 * The rule itself is [`restack`], in the pure module beside the walk, because the interesting
 * half of it is not "what does this window's strip say" but "may this window speak at all" — a
 * detached window's strip is empty and would otherwise reconcile the whole stack away, into a
 * cache every window of this origin shares. Returns the same array identity when nothing moved,
 * so subscribers do not re-render on every snapshot and nothing is written back for a no-op.
 */
function nextMru(previous: readonly string[], boot: Bootstrap | null): readonly string[] {
  const active = boot?.role.kind === 'shell' ? boot.role.active : null
  return restack(previous, windowProjectsOf(boot), active)
}

/** [`nextMru`], writing the cache whenever the order actually moved. */
function mruFor(previous: readonly ProjectId[], boot: Bootstrap | null): readonly ProjectId[] {
  const next = nextMru(previous, boot) as readonly ProjectId[]
  if (next !== previous) saveMru(next)
  return next
}

/* ---------------------------------------------------------------- and the same for tabs */

/**
 * One MRU stack per project, over that project's tabs. What Ctrl+Tab walks.
 *
 * # It moved into Rust, and this is now a *read* rather than a derivation
 *
 * Until M15 there was no per-tab focus history anywhere: `Tab` is `{ id, kind, tree }` with no
 * timestamp and no ordinal, `Project.activeTab` held one value with nothing behind it, and this
 * module rebuilt an order from every snapshot with [`restack`] and cached it under
 * `localStorage`'s `cide.tabMru`. The note that stood here argued the case for keeping it in the
 * webview and then admitted the argument was weak, because `activeTab` is *already* workspace
 * state.
 *
 * What settled it was **close**. "Closing a tab should activate the most recently used remaining
 * tab" makes the order something a mutation *consults*, and the mutation is
 * `cide_core::workspace::close_tab`. Two of its callers have no webview to ask — `cide_app::ide`
 * closes a withdrawn diff from the MCP server's thread, and the quit ladder closes projects
 * wholesale — so a successor computed here and passed down would have had to exist twice, and
 * the copy in Rust would have been the left-neighbour rule the feature replaces. Both objections
 * the old note raised turned out not to apply: the order moves only inside mutations that
 * already bump `rev`, so it costs no extra broadcast, and `#[serde(default)]` plus a repair on
 * load is not a schema break. See `cide_ipc::workspace::Project::tab_mru`.
 *
 * # What is left here, and why it is not just `project.tabMru`
 *
 * Two things Rust's field does not do on its own.
 *
 * [`reconcile`] appends tabs the order has never seen, at the **back**. Rust's order holds only
 * tabs that have actually been activated, and it is legitimately shorter than `tabs` — a
 * workspace restored from a build before the field arrived comes back with a single entry, by
 * `repair_tab_mru`'s deliberate refusal to invent a history out of strip order. The switcher has
 * to be able to walk to those tabs, so the tail is filled in here, where "which tabs exist" is
 * already known and where guessing costs nothing.
 *
 * And [`rememberTabMru`] still overlays an in-flight order, which is what makes a fast Ctrl+Tab
 * double-tap land on the third tab rather than bouncing between two: the walk commits, and the
 * next press must not wait a round trip for Rust's answer to come back. That job is unchanged by
 * the move; the answer it is racing is simply now authoritative when it arrives.
 *
 * # And what it contains
 *
 * Tabs, not files. The pinned Claude console, `ClaudeFull` tabs, file tabs, diff tabs and the
 * settings tab all live in one `Vec` and all appear here. Including the console is the whole
 * value of the gesture: one Ctrl+Tab from a file gets you back to the conversation about it.
 */
type TabStacks = Readonly<Record<string, readonly string[]>>

/** Shared by every project with nothing remembered yet, so "unchanged" compares by identity. */
const NO_STACK: readonly string[] = []

/**
 * The tab stacks this snapshot carries: `project.tabMru`, with unvisited tabs appended.
 *
 * Not `restack`, which is the *project* stack's function and does a different job — it derives
 * an order by touching the active id, because nothing else records one. Here the order arrives
 * already correct; the only thing missing is the tail, and [`reconcile`] is exactly that half of
 * `restack` on its own.
 *
 * `previous` is consulted for identity alone: returning the same array when the contents match
 * keeps subscribers from re-rendering on every snapshot, which arrives for every mutation in
 * every window. The remembered value is never *preferred* to Rust's — an optimistic overlay
 * written by [`rememberTabMru`] is meant to be overwritten the moment the real answer lands, and
 * a stale-wins rule here would make it permanent.
 */
function nextTabMru(previous: TabStacks, boot: Bootstrap | null): TabStacks {
  const projects = boot?.workspace.projects
  if (projects === undefined) return previous

  let moved = false
  const next: Record<string, readonly string[]> = {}
  for (const project of Object.values(projects)) {
    const remembered = previous[project.id] ?? NO_STACK
    const filled = reconcile(
      project.tabMru,
      project.tabs.map((tab) => tab.id),
    )
    const same =
      filled.length === remembered.length && filled.every((id, at) => id === remembered[at])
    const stack = same ? remembered : filled
    next[project.id] = stack
    if (stack !== remembered) moved = true
  }
  // The key count catches the other half: a project that closed, and a project seen for the
  // first time whose order happened to compare equal to the empty one it started from.
  if (!moved && Object.keys(next).length === Object.keys(previous).length) return previous
  return next
}

interface WorkspaceStore {
  /** Null until the first `app.getBootstrap` resolves. */
  boot: Bootstrap | null
  theme: Theme
  /**
   * Projects in most-recently-used order, `mru[0]` being the active one.
   *
   * What Ctrl+Tab walks. Read the note on [`MRU_CACHE_KEY`] for why it lives here and not in
   * the Rust workspace, and `keys/switcher.ts` for the walk itself. Never written by a
   * component: it is derived from every snapshot by [`nextMru`], so it cannot drift from the
   * set of projects that actually exist.
   */
  mru: readonly ProjectId[]
  /**
   * Tabs in most-recently-used order, per project — what Ctrl+Tab walks.
   *
   * Read the note on [`nextTabMru`] for what it holds and why the order itself lives in Rust
   * now. Never written by a component: it is read off every snapshot, so it cannot drift from
   * the set of tabs that actually exist.
   */
  tabMru: TabStacks
  /**
   * Record the order the switcher committed to, before the activation round trip lands.
   *
   * The snapshot recomputes exactly this a moment later — [`nextMru`] puts the active project
   * at the front over the same reconciled list — so the two agree by construction and this is
   * an early copy of an answer, not a second opinion. It exists because the round trip is not
   * instant and the user can press Ctrl+Tab again inside it: reading a stack whose front is
   * still the *previous* project makes the second press pick the project that was just
   * activated, which is the double-tap doing nothing.
   */
  rememberMru: (order: readonly ProjectId[]) => void
  /** [`rememberMru`] for one project's tab stack, and for exactly the same reason. */
  rememberTabMru: (project: ProjectId, order: readonly TabId[]) => void

  hydrate: () => Promise<void>
  /** Start following `cide://workspace-changed`. Returns an unlisten function. */
  subscribe: () => Promise<() => void>
  /** Open a project and re-read the tree. */
  openProject: (paths: string[]) => Promise<void>
  /**
   * Make a project the one this window shows.
   *
   * `project_activate` has existed since projects went multi-window, and every caller — the
   * header tab's click, the Ctrl+Tab switcher's commit, `project.next` / `project.prev` — had to spell
   * `projectApi.activate(id).then(hydrate)` for itself. It belongs here with the other
   * mutations for the same reason they do: the re-read is not optional, and a call site that
   * forgets it leaves the window painting the old project until something else hydrates.
   */
  activateProject: (id: ProjectId) => Promise<void>
  /**
   * Close a project. Confirms first when it holds unsaved edits or a session mid-turn.
   *
   * `force` is the answer coming back from that confirmation and is never passed by a call
   * site that has not shown it — see `closeTab`.
   */
  closeProject: (id: ProjectId, force?: boolean) => Promise<void>
  newClaudeTab: (project: ProjectId) => Promise<TabId>
  activateTab: (project: ProjectId, tab: TabId) => Promise<void>
  /**
   * Close a tab, confirming first if that would lose something.
   *
   * Call sites pass two arguments and get the guard for free: with unsaved edits in the tab
   * this parks a `PendingClose` and resolves without closing, and the dialog's answer calls
   * back in with `force: true`. The refusal itself comes from Rust, so a call site that
   * bypassed this store still could not discard a buffer.
   */
  closeTab: (project: ProjectId, tab: TabId, force?: boolean) => Promise<void>
  /**
   * Move a tab along the strip. What a tab drag commits on `pointerup`.
   *
   * `before` is the tab it lands in front of, `null` for the end. Ids rather than indices for
   * the reason `tab.reorder` documents at length: the boundary is computed in the webview at
   * pointer-move time and the strip can change before the release.
   */
  reorderTab: (project: ProjectId, tab: TabId, before: TabId | null) => Promise<void>
  /**
   * Close several tabs as **one** gesture — *Close others*, *Close to the left*, *Close to the
   * right*.
   *
   * Not a loop over `closeTab`, which is what those three used to be. That asked once per tab at
   * risk and queued the dialogs, so closing eight tabs with three dirty files meant three
   * modals, each saying "Closing *this tab*", for a gesture made once. This asks once in total,
   * names every file in the one list, and `force` comes back from that single answer.
   */
  closeTabs: (project: ProjectId, tabs: readonly TabId[], force?: boolean) => Promise<void>

  splitPane: (
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    axis: Axis,
    side: Side,
    intent?: SplitIntent | null,
  ) => Promise<SplitOutcome>
  /**
   * Add a full-width row holding one pane.
   *
   * `after: null` appends at the bottom; otherwise the row lands on `side` of the row that
   * holds `after`. Independent of how many tiles any other row has — that is the whole point
   * of a row being a row.
   */
  addRow: (
    project: ProjectId,
    tab: TabId,
    after?: PaneId | null,
    side?: Side,
    intent?: SplitIntent | null,
  ) => Promise<SplitOutcome>
  closePane: (project: ProjectId, tab: TabId, pane: PaneId, force?: boolean) => Promise<void>
  focusPane: (project: ProjectId, tab: TabId, pane: PaneId) => Promise<void>
  maximizePane: (project: ProjectId, tab: TabId, pane: PaneId | null) => Promise<void>
  /** Committed on pointerup only — the drag itself writes to the DOM. */
  setRatio: (project: ProjectId, tab: TabId, split: SplitId, ratio: number) => Promise<void>
  /** Every member of the pane's chain to an equal share of it. `'row'` is its tiles. */
  distributePanes: (project: ProjectId, tab: TabId, pane: PaneId, axis: Axis) => Promise<void>
  navigatePane: (
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    direction: Direction,
  ) => Promise<void>
  bindSession: (
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    session: SessionId,
  ) => Promise<void>

  /**
   * Tear a pane out into its own window.
   *
   * The host is *released*, not destroyed: the session stays alive in the Rust registry and
   * the new window attaches to it. Destroying would take the child with it, which is the
   * one thing detaching must never do.
   */
  detachPane: (project: ProjectId, tab: TabId, pane: PaneId) => Promise<void>
  redockPane: (label: string) => Promise<void>
  /**
   * Tear a whole tab out into its own window — the road a file takes, since a lone editor
   * pane cannot leave the tab its buffer is registered under. Every host in the tab is
   * released, for `detachPane`'s reason, one pane at a time.
   */
  detachTab: (project: ProjectId, tab: TabId) => Promise<void>
  redockTab: (label: string) => Promise<void>
  setWindowMode: (mode: WindowMode) => Promise<void>
  /** Replace the mirror. Older revisions are ignored — snapshots can race. */
  applySnapshot: (workspace: Workspace) => void
  setTheme: (theme: Theme) => void
  toggleTheme: () => void
}

export const useWorkspace = create<WorkspaceStore>((set, get) => ({
  boot: null,
  // Read back from the attribute `public/theme-boot.js` already wrote in <head>, rather
  // than named literally here. This value is live for the whole window-open — `hydrate` is
  // an IPC round trip — and `App.tsx` writes it straight to `documentElement.dataset.theme`
  // on mount. A literal here therefore *overwrites* what theme-boot resolved from `?theme=`
  // and repaints the window in the wrong palette until the bootstrap lands, which is the
  // exact flash theme-boot exists to remove. Hardcoding `'light'` instead would fix the
  // default case and leave dark-theme users flashing; only the attribute knows.
  theme: globalThis.document?.documentElement.dataset.theme === 'dark' ? 'dark' : 'light',

  // Read from the cache at module load, before any snapshot has arrived, so the very first
  // Ctrl+` of a session has a real order to walk. It is reconciled against the live project
  // list on the first snapshot, so a stale id never survives to reach `project_activate`.
  mru: loadMru() as readonly ProjectId[],
  // Empty, one level down, and deliberately not read from anywhere: the tab order arrives with
  // the first snapshot, out of `workspace.json`, so there is nothing for a cache to be earlier
  // than. That is the whole practical dividend of the move — `cide.tabMru` in `localStorage` had
  // to exist precisely because the order it held was not in the file the tabs came from.
  tabMru: {},

  hydrate: async () => {
    const boot = await appApi.getBootstrap()
    // Before `set`, and before anything can render. The language table decides how a buffer folds,
    // and `foldSpecFor` is called inside the editor's mount dispatch where it cannot await — a
    // table that arrived one tick after the first editor would restore the remembered scroll
    // position against unfolded heights and land the reader somewhere they did not leave. This is
    // the whole reason the resolved set rides `Bootstrap` rather than having a command of its own.
    for (const problem of registerLanguages(boot.extensions.languages.map((row) => row.def))) {
      console.warn(`[cide] ${problem}`)
    }
    set({
      boot,
      theme: boot.workspace.settings.theme,
      mru: mruFor(get().mru, boot),
      tabMru: nextTabMru(get().tabMru, boot),
    })
    // After the state is set, not before: the explorer and the picker read the project from
    // the store, and an index that started against a project the window has not adopted yet
    // would race the components that are about to ask it questions.
    syncFileIndex(boot.workspace)
  },

  /**
   * Follow every mutation, including the ones this window did not make.
   *
   * Before M5 there was one window and a local re-read after each command was enough. A
   * detached pane makes that false: the pane has to vanish from the shell window and appear
   * in the new one, and neither window can learn that by asking about its own last command.
   */
  subscribe: async () => {
    const unlisten = await events.onWorkspaceChanged((workspace) => {
      get().applySnapshot(workspace)
    })
    /*
     * The keymap is a second subscription because it is not in the workspace.
     *
     * `applySnapshot` builds `{ ...current, workspace }` and keeps everything else, `keymap`
     * included — which is correct for a tree mutation and is exactly why a rebind cannot ride
     * along on one. Without this line, Settings → Keymap in one window changes that window's
     * bindings (it re-reads them itself) and no other window's until something there happens
     * to call `hydrate`, which is the "my keybinding does nothing" failure with a window's
     * worth of distance between cause and symptom.
     *
     * A **new array** is what lands in `boot.keymap`, and that matters as much as its
     * contents: `keys/gate.ts` caches its indexed keymap against the identity of what
     * `bindings()` returns, so an in-place mutation would leave the gate resolving against the
     * table it built on the first keystroke.
     */
    const unlistenKeymap = await events.onKeymapChanged((keymap) => {
      const boot = get().boot
      if (boot === null) return
      set({ boot: { ...boot, keymap } })
    })
    /*
     * The imported colour schemes are a third subscription, for the keymap's reason. (M24)
     *
     * They ride `Bootstrap` and are not part of the workspace, so `applySnapshot` keeps the
     * old array. Without this line, importing a theme in one window changes the *setting*
     * everywhere — that does ride the workspace — while the palette it names exists in one
     * window only, and every other window falls back to the builtin. The user sees the import
     * work in the window they did it in and silently not work in the one beside it.
     */
    const unlistenSchemes = await events.onSchemesChanged((schemes) => {
      const boot = get().boot
      if (boot === null) return
      set({ boot: { ...boot, schemes } })
    })
    return () => {
      unlisten()
      unlistenKeymap()
      unlistenSchemes()
    }
  },

  // Every mutation re-reads the whole tree rather than patching the mirror locally. The
  // snapshot is small, the round trip is already paid for, and a local patch that drifts
  // from Rust's answer is a class of bug worth not having. `cide://workspace-changed`
  // replaces the re-read in a later milestone.
  openProject: async (paths) => {
    await projectApi.open(paths)
    await get().hydrate()
  },
  activateProject: async (id) => {
    await projectApi.activate(id)
    await get().hydrate()
  },
  closeProject: async (id, force = false) => {
    // Asked *before* the command, unlike `closeTab`, because a project close can be blocked
    // by something Rust does not refuse: a Claude session mid-turn. Rust has no business
    // refusing that — an interrupted turn is recoverable and the user may have turned the
    // warning off — so the only way to warn about it is to ask.
    if (!force) {
      const decision = await appApi.quitRequested(id)
      if (decision.unsaved.length > 0 || decision.blocking.length > 0) {
        requestCloseConfirm({
          scope: 'project',
          unsaved: decision.unsaved,
          sessions: decision.blocking,
          proceed: () => get().closeProject(id, true),
        })
        return
      }
    }
    // Still guarded on the way out: a buffer can go dirty between the question and the
    // answer, and the refusal is the thing that makes that race harmless.
    const parked = await refused(
      'project',
      () => projectApi.close(id, force),
      () => get().closeProject(id, true),
    )
    if (parked) return
    await get().hydrate()
  },
  newClaudeTab: async (project) => {
    const created = await tabApi.newClaude(project)
    await get().hydrate()
    return created
  },
  activateTab: async (project, tab) => {
    await tabApi.activate(project, tab)
    await get().hydrate()
  },
  reorderTab: async (project, tab, before) => {
    await tabApi.reorder(project, tab, before)
    await get().hydrate()
  },
  closeTabs: async (project, tabs, force = false) => {
    if (tabs.length === 0) return
    if (!force) {
      // One question for the whole gesture. See `tabsCloseRisk` for why this is not
      // `tabCloseRisk` in a loop, and `CloseScope`'s `tabs` variant for why the wording differs.
      const risk = await tabsCloseRisk(get().boot, project, tabs)
      if (risk.sessions.length > 0 || risk.unsaved.length > 0) {
        requestCloseConfirm({
          scope: 'tabs',
          unsaved: risk.unsaved,
          sessions: risk.sessions,
          proceed: () => get().closeTabs(project, tabs, true),
        })
        return
      }
    }

    /*
     * Sequential, not `Promise.all`, and that is not caution about load.
     *
     * Every close is a workspace mutation that bumps `rev`, and `tab_close` also reconciles
     * windows and can resolve an agent's pending `openDiff`. Firing eight at once interleaves
     * eight read-modify-writes against one lock and eight `workspace-changed` broadcasts, and
     * the strip visibly flickers through seven intermediate layouts. In order, the user sees the
     * tabs go.
     *
     * Each one still goes to Rust **unforced** unless the dialog was answered, so the domain
     * stays the authority: `tabsCloseRisk` read a snapshot, and a file that turned dirty between
     * that read and this call must still be refused. That refusal parks its own dialog through
     * `refused`, which is the pre-existing per-tab path doing what it is for — catching a race,
     * rather than being the ordinary way this gesture asks.
     */
    for (const tab of tabs) {
      const parked = await refused(
        'tab',
        () => tabApi.close(project, tab, force),
        () => get().closeTab(project, tab, true),
      )
      // A refusal we did not anticipate stops the batch rather than closing the tabs after it
      // behind the user's back: the dialog on screen is about *this* tab, and marching on would
      // mean answering it decides the fate of files it never named.
      if (parked) break
    }
    await get().hydrate()
  },
  closeTab: async (project, tab, force = false) => {
    // Mostly no pre-flight question here, unlike `closeProject`: the unsaved case comes back
    // from Rust as a refusal that already names the file, so asking first would be a second
    // round trip for an answer the failure path hands over anyway — and it would be the
    // answer to a slightly older workspace.
    //
    // Sessions are the exception, because Rust has no business refusing an interrupted turn,
    // and they are asked about only when this tab actually has one bound. `tabCloseRisk`
    // reports the unsaved files in the same breath: once that round trip is being made, a
    // dialog that mentions only the session would let "Close anyway" discard a buffer the
    // user was never shown.
    if (!force) {
      const risk = await tabCloseRisk(get().boot, project, tab)
      if (risk.sessions.length > 0 || risk.unsaved.length > 0) {
        requestCloseConfirm({
          scope: 'tab',
          unsaved: risk.unsaved,
          sessions: risk.sessions,
          proceed: () => get().closeTab(project, tab, true),
        })
        return
      }
    }
    const parked = await refused(
      'tab',
      () => tabApi.close(project, tab, force),
      () => get().closeTab(project, tab, true),
    )
    if (parked) return
    await get().hydrate()
  },

  splitPane: async (project, tab, pane, axis, side, intent = null) => {
    const created = await paneApi.split(project, tab, pane, axis, side, intent)
    // Recorded before the hydrate that makes the pane renderable, so the spawn plan is
    // already in place by the time `TerminalPane` mounts and asks for one. The other order
    // races: the pane appears, spawns a fresh session, and the fork is lost.
    if (created.intent.kind === 'forkPrimary' || created.intent.kind === 'mirror') {
      rememberSpawnPlan(created.pane, created.intent)
    }
    await get().hydrate()
    return created
  },
  addRow: async (project, tab, after = null, side = 'after', intent = null) => {
    const created = await paneApi.addRow(project, tab, after, side, intent)
    // Same ordering as `splitPane`, and for the same reason: the plan has to be in place
    // before the hydrate that makes the pane renderable, or `TerminalPane` mounts, finds no
    // plan and spawns a fresh session where the user asked to branch one.
    if (created.intent.kind === 'forkPrimary' || created.intent.kind === 'mirror') {
      rememberSpawnPlan(created.pane, created.intent)
    }
    await get().hydrate()
    return created
  },
  closePane: async (project, tab, pane, force = false) => {
    // Three steps, and the order is the whole of it.
    //
    // 1. The domain call, which may be refused — the project console's primary pane cannot
    //    go, nor a tab's last, nor an editor pane holding unsaved edits — so nothing may be
    //    disposed until it has succeeded. The unsaved refusal parks the same dialog a tab
    //    close does; discarding re-issues with `force: true`.
    const parked = await refused(
      'pane',
      () => paneApi.close(project, tab, pane, force),
      () => get().closePane(project, tab, pane, true),
    )
    if (parked) return

    // 2. Re-read and let React commit. The pane's `TerminalPane` unmounts here: its effect
    //    cleanup detaches the sink, clears the exit poll and parks the host. Disposing
    //    before this point destroys a host that is still mounted, and any async
    //    continuation still in flight would call `getHost` and resurrect it — leaving a
    //    freshly created host carrying a session nobody is watching.
    await get().hydrate()
    await nextFrame()

    // 3. Only now is the pane genuinely finished, so the child and the host go with it.
    //    Leaving the host registered leaks an xterm instance and possibly a WebGL context
    //    per closed pane, and WebKitGTK caps concurrent contexts at roughly 8-16 — a
    //    session spent splitting and closing would stop painting. Detaching a pane into its
    //    own window is the opposite case and uses `releaseHost`, which keeps both.
    //
    //    The child is a separate question from the host, and the two answers differ for a
    //    pane that adopted a session it did not spawn — a `claude.mirror`, and every
    //    subagent run opened into a pane. That pane closes its *view*; the child belongs to
    //    the pane being mirrored and is not ours to kill. `destroyHost` still runs
    //    unconditionally, because this pane genuinely is finished, and clearing the ledger's
    //    session id is what stops a late async continuation resurrecting it.
    const host = peekHost(pane)
    const session = host?.mirrored === true ? undefined : host?.sessionId
    destroyHost(pane)
    if (session) await sessionApi.kill(session)
  },
  focusPane: async (project, tab, pane) => {
    await paneApi.focus(project, tab, pane)
    await get().hydrate()
  },
  maximizePane: async (project, tab, pane) => {
    await paneApi.maximize(project, tab, pane)
    await get().hydrate()
  },
  /**
   * The one mutator here that does **not** re-hydrate afterwards, and the reason is the drag.
   *
   * Every command that reaches `WorkspaceState::update` already broadcasts
   * `cide://workspace-changed` to every window *including this one* (`crates/cide-app/src/emit.rs`),
   * and `applySnapshot` below is what receives it. The `hydrate()` its neighbours add on top is a
   * second round trip and a second `set({ boot })` — and with no `React.memo` anywhere in this
   * frontend, each of those is a re-render of the entire tree under `App`.
   *
   * Harmless at the end of a click. Not harmless here: this is what `pointerup` calls at the end
   * of a divider drag, so it was landing two whole-app re-renders — each of which resizes every
   * pane in every tab — in the frame the user let go of the mouse. That is the visible hitch at
   * the end of a resize.
   *
   * Left in place everywhere else deliberately: the pattern is load-bearing for commands whose
   * answer is not only the workspace, and rewriting eighteen call sites is not this change's to
   * make.
   */
  setRatio: async (project, tab, split, ratio) => {
    await paneApi.setRatio(project, tab, split, ratio)
  },
  /*
   * Hydrates, unlike `setRatio` above, and the difference is that this one is a menu click
   * rather than the last frame of a drag: nothing has written the new tracks to the DOM
   * ahead of the round trip, so the snapshot *is* the only thing that moves the dividers.
   * One re-render at the end of a click is what every other mutator here costs.
   */
  distributePanes: async (project, tab, pane, axis) => {
    await paneApi.distribute(project, tab, pane, axis)
    await get().hydrate()
  },
  navigatePane: async (project, tab, pane, direction) => {
    // Two calls because the domain separates "where would focus go" from "move it": the
    // first is a pure query on the tree, and at the edge it answers null rather than
    // wrapping around, which is what stops Alt+Left cycling forever in a two-pane tab.
    const target = await paneApi.navigate(project, tab, pane, direction)
    if (target === null) return
    await paneApi.focus(project, tab, target)
    await get().hydrate()
  },
  bindSession: async (project, tab, pane, session) => {
    await paneApi.bindSession(project, tab, pane, session)
    await get().hydrate()
  },

  detachPane: async (project, tab, pane) => {
    // Measured before the pane leaves the tree, because a moment later its host is parked
    // and reports nothing. The new window then opens at the size the pane already was.
    const el = peekHost(pane)?.el
    const rect =
      el && el.clientWidth > 0 && el.clientHeight > 0
        ? { width: el.clientWidth, height: el.clientHeight }
        : undefined
    await windowApi.detachPane(project, tab, pane, rect)
    await get().hydrate()
    await nextFrame()
    // Released rather than destroyed. The pane has left this window's tree but its session
    // is still running and the new window is attaching to it; destroying the host here
    // would dispose a terminal whose child nobody has told to stop.
    releaseHost(pane)
  },
  redockPane: async (label) => {
    await windowApi.redockPane(label)
    await get().hydrate()
  },
  detachTab: async (project, tab) => {
    // Read before the call: the ids do not change — the tab stays in `project.tabs` — but
    // reading them now means the release below cannot miss a pane over a mirror that
    // happens to be mid-refresh.
    const tree = get().boot?.workspace.projects[project]?.tabs.find((t) => t.id === tab)?.tree
    const panes = tree ? Object.keys(tree.panes) : []
    // The size the new window should open at is the size the tab's content already has.
    // `TabContent` stamps each panel with its tab id, so the panel *is* the measurement —
    // unlike a pane there is no host to ask, and an editor pane has no host at all.
    const el = document.querySelector(`[data-tab-id="${tab}"]`)
    const rect =
      el instanceof HTMLElement && el.clientWidth > 0 && el.clientHeight > 0
        ? { width: el.clientWidth, height: el.clientHeight }
        : undefined
    await windowApi.detachTab(project, tab, rect)
    await get().hydrate()
    await nextFrame()
    // Released rather than destroyed, per pane, exactly as `detachPane` releases its one:
    // any terminal split into this tab keeps its child running and the new window attaches
    // to the same session. Editor panes have no host and `releaseHost` ignores them.
    for (const pane of panes) releaseHost(pane)
  },
  redockTab: async (label) => {
    await windowApi.redockTab(label)
    await get().hydrate()
  },
  setWindowMode: async (mode) => {
    await windowApi.setMode(mode)
    await get().hydrate()
  },

  applySnapshot: (workspace) => {
    const current = get().boot
    if (!current) return
    // Events carry the revision precisely so a snapshot that arrives out of order can be
    // dropped rather than winding the UI backwards.
    if (workspace.rev < current.workspace.rev) return
    const boot = { ...current, workspace }
    // Both MRU stacks follow the snapshot, not the action: a project or a tab activated, opened
    // or closed in another window reaches this one only here. See `nextMru`, `nextTabMru`.
    set({ boot, mru: mruFor(get().mru, boot), tabMru: nextTabMru(get().tabMru, boot) })
    // A project opened or closed in *another* window reaches this one only here. Without
    // this line the second window's tree and picker stay empty until something in it happens
    // to call `hydrate`.
    syncFileIndex(workspace)
  },

  rememberMru: (order) => {
    saveMru(order)
    set({ mru: order })
  },

  // Store-only: there is no cache behind this any more, and there must not be. The order it
  // writes is an optimistic copy of what `tab_activate` is about to make true, and the next
  // snapshot overwrites it with Rust's answer — persisting it would outlive that correction.
  rememberTabMru: (project, order) => {
    set({ tabMru: { ...get().tabMru, [project]: order } })
  },

  setTheme: (theme) => set({ theme }),
  toggleTheme: () => set((s) => ({ theme: s.theme === 'dark' ? 'light' : 'dark' })),
}))

/** A stable empty list, so a bootless window does not hand React a new array every read. */
const NO_PROJECTS: Project[] = []

/**
 * Projects in header-tab order.
 *
 * The `useMemo` is load-bearing and this function is the reason the rule is written down here.
 * A zustand selector runs on every store read and `useSyncExternalStore` compares the result
 * with `Object.is`, so `useWorkspace((s) => Object.values(...))` returns a fresh array every
 * time, React concludes the snapshot changed, re-renders, reads again — and stops only at
 * *Maximum update depth exceeded*, which unmounts the whole root.
 *
 * This one never fired because nothing calls it. `ToolWindowSplitter` wrote the same line and
 * did fire, and because it renders exactly when the tool window is open — a flag persisted on
 * `Project` — it emptied the window on every launch. `check:selectors` now refuses the shape in
 * either place, so neither the live copy nor the dormant one can come back.
 */
export function useProjects(): Project[] {
  const projects = useWorkspace((s) => s.boot?.workspace.projects)
  return useMemo(() => (projects ? Object.values(projects) : NO_PROJECTS), [projects])
}

/** The project this window is currently showing, if any. */
export function useActiveProject(): Project | null {
  return useWorkspace((s) => {
    const boot = s.boot
    if (!boot || boot.role.kind !== 'shell' || !boot.role.active) return null
    return boot.workspace.projects[boot.role.active] ?? null
  })
}

/** The active project's tabs, `tabs[0]` being the pinned console. */
export function useTabs(): Tab[] {
  const project = useActiveProject()
  return project?.tabs ?? []
}
