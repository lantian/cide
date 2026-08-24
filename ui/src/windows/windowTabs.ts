/**
 * Which tabs a window draws, and what a tab's pane cluster acts on. (Detached tabs)
 *
 * Two rules, one module, because they are two halves of one invariant: **a tab is drawn by
 * exactly one window**. A `WindowRole::DetachedTab` names a tab that stays in `project.tabs`
 * — `unsaved_tabs`, `plan_restore` and the awaiting arithmetic all walk that list and must
 * keep seeing it — so the *only* thing keeping the shell from rendering the same tab a
 * second time is the filter below. Two windows rendering one file tab is two live
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

/** The part of `WindowRole` these rules read. */
export type WindowRoleLike =
  | { readonly kind: 'shell' }
  | { readonly kind: 'detachedPane'; readonly tab: string }
  | { readonly kind: 'detachedTab'; readonly tab: string }

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
  }
}

/*
 * Runtime values only, no imports — see the header. The same note is at the foot of
 * `windows/detachedPane.ts` and `terminal/pathMatch.ts`.
 */
