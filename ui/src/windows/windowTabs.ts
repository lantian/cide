/**
 * Which projects and tabs a window draws, and what a tab's pane cluster acts on.
 *
 * Three rules, one module, because they are three views of one invariant: **a thing is drawn
 * by exactly one window**. [`shownProjects`] is the outer ring of it and the newest — the
 * header used to render every project in the *process*, so *One window per project* opened
 * three windows that each drew the same three-tab strip. The rest of this note is the tab
 * ring, which came first.
 *
 * **A tab is drawn by exactly one window.** A `WindowRole::DetachedTab` names a tab that stays
 * in `project.tabs` — `unsaved_tabs`, `plan_restore` and the awaiting arithmetic all walk
 * that list and must keep seeing it — so the *only* thing keeping the shell from rendering
 * the same tab a second time is the filter below. Two windows rendering one file tab is two live
 * CodeMirror buffers over one file, and whichever saves second silently discards the
 * other's edits; that is the exact failure `editor/openBuffers.ts` registers buffers per
 * tab to prevent, and it is why `windows/detachedPane.ts` refuses to render an editor at
 * all.
 *
 * The second rule is what the pane cluster's buttons mean for a tab's **only** pane:
 *
 * * *maximize* is withheld — one pane already fills its tab, so the button was a toggle
 *   that could never change a pixel;
 * * *close* and *detach* act on the **tab**. `layout::close` and `layout::take_pane` both
 *   refuse a tab's last pane (`CoreError::LastPane` — an empty tree is unrepresentable),
 *   and the buttons used to run straight into that refusal and print it. Closing the last
 *   pane *is* closing the tab, which is the sentence `cmd::file::open_file_tab` has carried
 *   since M9; detaching it is detaching the tab, which is the only road an editor can take
 *   out of the shell (see above — the buffer moves with the `TabId`).
 *
 * Import-free on purpose: `ui/scripts/check-detached.mjs` compiles this file on its own and
 * runs the tables below. The types are structural subsets of the generated DTOs, declared
 * rather than imported; the real ones are checked against them at the call sites in
 * `App.tsx` and `keys/dispatch.ts`.
 */

/**
 * The part of `WindowRole` these rules read.
 *
 * `projects` on the shell arm and `project` on the two detached ones are what
 * [`shownProjects`] answers from; the tab rules below read neither. Declared required rather
 * than optional on purpose: an absent field would have to mean *"unknown, so show
 * everything"*, and "show everything" is precisely the bug — a window drawing a project it
 * does not render. The generated `WindowRole` carries all three, so nothing in the app has
 * to invent one.
 */
export type WindowRoleLike =
  | { readonly kind: 'shell'; readonly projects: readonly string[] }
  | { readonly kind: 'detachedPane'; readonly project: string; readonly tab: string }
  | { readonly kind: 'detachedTab'; readonly project: string; readonly tab: string }

/**
 * The tabs of one project that have windows of their own, from the workspace's window map.
 *
 * A `Set` because both consumers ask membership in a loop, and derived per call rather than
 * cached: the map rides every `cide://workspace-changed` snapshot, and a cache over it would
 * be the invalidation-free mirror `keys/target.ts` records going wrong.
 */
export function detachedTabs(
  windows: Readonly<Record<string, WindowRoleLike>>,
): ReadonlySet<string> {
  const out = new Set<string>()
  for (const role of Object.values(windows)) {
    if (role.kind === 'detachedTab') out.add(role.tab)
  }
  return out
}

/**
 * The tabs this window actually draws, in strip order.
 *
 * * A **shell** draws its project's tabs minus the torn-out ones. The domain keeps
 *   `active_tab` off a detached tab (`detach_tab` moves it, `activate_tab` refuses to move
 *   it back), so filtering the list never hides the active tab.
 * * A **detached tab** window draws exactly its one tab — the same list shape, so
 *   `TabContent` needs no second rendering path.
 * * A **detached pane** window draws no tab at all; its pane lives outside every tree.
 */
export function shownTabs<T extends { readonly id: string }>(
  role: WindowRoleLike,
  tabs: readonly T[],
  windows: Readonly<Record<string, WindowRoleLike>>,
): readonly T[] {
  if (role.kind === 'detachedPane') return []
  if (role.kind === 'detachedTab') {
    const own = role.tab
    return tabs.filter((tab) => tab.id === own)
  }
  const torn = detachedTabs(windows)
  return torn.size === 0 ? tabs : tabs.filter((tab) => !torn.has(tab.id))
}

/**
 * The projects this window's header draws, in workspace order.
 *
 * `Stacked` and `PerProject` are nothing but two mappings from `ProjectId` to shell window
 * (`WindowRole`'s own doc says so), and the role already carries this window's half of the
 * mapping: `projects` is every project it is responsible for — all of them stacked, exactly
 * one per-project. Reading it is the whole rule.
 *
 * **What it replaces.** `App.tsx` passed `Object.values(workspace.projects)` — every project
 * in the *process* — so choosing *One window per project* opened three windows that each drew
 * the same three-tab strip, one tab of which was the window's own. Nothing about that was
 * cosmetic: `activate_project` sets `active` on every shell whose `projects` contains the id,
 * so clicking another window's tab moved *that* window's active project and left this one
 * showing a header tab highlighted over content it does not render. The strip was also the
 * only place offering to close a project this window has no view of.
 *
 * **Empty for a detached window.** A `pane:` or `tab:` window has no project strip — `App.tsx`
 * gives each its own header — and the switcher must not offer one either. That second half is
 * `keys/target.ts`'s `windowProjectsOf`, which reads the same `role.projects` and answers the
 * same shape in ids; it is a separate function only because `check-commands` compiles that
 * module *alone* and a value import would emit a `require` nothing can resolve. Both read the
 * role; neither may derive a strip from `workspace.projects`, which is the whole bug.
 *
 * The order is the caller's, which is the workspace's `projects` insertion order and therefore
 * the header order `reorder_project` writes. Filtering preserves it; mapping `role.projects`
 * instead would only agree by luck — it agrees today because `reorder_project` ends in
 * `rebuild_windows`, and that is a fact about another crate rather than a property of this one.
 */
export function shownProjects<T extends { readonly id: string }>(
  role: WindowRoleLike,
  projects: readonly T[],
): readonly T[] {
  if (role.kind !== 'shell') return []
  const own = new Set(role.projects)
  return projects.filter((project) => own.has(project.id))
}

/** What one button in the pane cluster should do. `hidden` draws nothing at all. */
export type ClusterAction = 'pane' | 'tab' | 'hidden'

/** What the cluster offers for one pane, given its situation. */
export interface ClusterPlan {
  /** Whether the maximize toggle is drawn. One pane already fills its tab. */
  readonly maximize: boolean
  /** What close acts on. Never `hidden`: a refused close is *drawn* refused, with a reason. */
  readonly close: 'pane' | 'tab'
  /** What detach acts on. */
  readonly detach: 'pane' | 'tab'
  /**
   * Whether the grab handle is drawn — a pane that is its tab's only one has nowhere to go.
   *
   * The *kind* gate is not here: it belongs to the frame, which knows what it is drawing.
   * This answers the question the whole cluster asks, "is there more than one pane", so that
   * the handle and the maximize toggle cannot disagree about it.
   */
  readonly move: boolean
}

/**
 * The cluster's plan for one pane.
 *
 * `paneCount` is the tab's whole tree, not the pane's row — a 2×1 column is two panes and
 * its members can maximize, close and detach individually. The `pinned` flag (the project
 * console, `tabs[0]`) keeps the tab-scoped arms out of reach there: the console cannot
 * close or detach as a tab any more than its primary pane can as a pane, and the buttons
 * for its sole pane stay pane-scoped so the existing role-based refusals keep wording them.
 */
export function clusterPlan(paneCount: number, pinned: boolean): ClusterPlan {
  const sole = paneCount <= 1
  return {
    maximize: !sole,
    close: sole && !pinned ? 'tab' : 'pane',
    detach: sole && !pinned ? 'tab' : 'pane',
    move: !sole,
  }
}

/*
 * Runtime values only, no imports — see the header. The same note is at the foot of
 * `windows/detachedPane.ts` and `terminal/pathMatch.ts`.
 */
