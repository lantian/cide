/**
 * Every decision the header's and the tab strip's menus make, with none of the React they
 * make them in.
 *
 * Same argument, and the same shape, as `src/menus/model.ts` and `src/chrome/sidebarWidth.ts`:
 * there is no browser and no jsdom in this harness, so anything that can only be observed by
 * rendering is effectively unverified. What a reviewer actually wants proved about these menus
 * is *which lines a user can click and what a greyed one tells them* — the pinned console
 * refusing to close, a recent project whose folder has gone naming the reason rather than
 * silently failing, Split declining on a tab that is not active. All of that is a pure function
 * of records, so it lives here and `ui/scripts/check-menu-model.mjs` compiles this file on its
 * own and exercises it.
 *
 * **Import only types, and only from import-free modules.** `check-menu-model.mjs` runs a bare
 * `tsc` over this file; a value import — React, a CSS module, `@/ipc/client` — is how that
 * stops working. The `@/` alias is deliberately not used for the same reason.
 *
 * Every handler arrives as a plain callback rather than being called for. `navigator.clipboard`
 * and `projectMenu.reveal` are the component's business; whether the *item* is offered at all
 * is this file's, and separating the two is what makes the enablement rules checkable.
 */
import type { MenuEntry } from '../menus/model'
import type { Bootstrap, PaneId, ProjectId, Tab, TabId } from '../ipc/generated'

// --- the sentences a disabled line shows ----------------------------------------------------
//
// Named constants rather than literals at each site: they are user-visible prose, they repeat
// across two surfaces, and a check script asserting on a string it also defines proves nothing.

/** A recent project whose directory is gone. Shown, never hidden — see [`recentEntries`]. */
export const MISSING_REASON = 'This folder is no longer there — moved, deleted or unmounted'
/** Nothing has been opened yet. */
export const NO_RECENTS_REASON = 'Projects you open are listed here'
/** The host passed no handler. Worded to match the header's disabled ⊞/⧉ tooltips. */
export const NO_HOST = 'Not available in this window'
/** A per-tab action whose handler only knows about the active tab. */
export const NOT_ACTIVE = 'Activate this tab first'
export const NO_CLIPBOARD = 'This webview exposes no clipboard'
export const PINNED_REASON = 'The project console is pinned'
export const ONLY_PROJECT = 'This is the only open project'
export const NO_PROJECT_REASON = 'Open a project first'
export const MAXIMIZED_REASON = 'A pane is maximized — restore it to add a row'

// --- recent projects ------------------------------------------------------------------------

/** The half of `RecentEntry` this file needs. Structurally satisfied by the generated type. */
export interface RecentLike {
  readonly project: { readonly path: string; readonly displayPath: string }
  readonly exists: boolean
}

export interface RecentActions {
  browse: () => void
  reopen: (path: string) => void
  forgetMissing: () => void
  clear: () => void
}

/**
 * The `▾` menu: open a folder, then the projects opened before, newest first.
 *
 * Ordering is the backend's — `project_recent` sorts and caps — so nothing here re-sorts. Two
 * readers of one list agreeing by accident is a bug waiting to happen; one authority is not.
 *
 * A missing folder is **listed and disabled**, never dropped. Dropping it is the failure this
 * is written against: a user who worked in `~/tmp/spike` last week and cannot find it in the
 * list learns nothing, because "the app forgot" and "the folder is gone" look identical from
 * the outside. A greyed line saying which it is answers the question, and the two removal
 * items below give them somewhere to go.
 */
export function recentEntries(
  recents: readonly RecentLike[],
  actions: RecentActions,
): MenuEntry[] {
  const items: MenuEntry[] = [
    { id: 'browse', label: 'Open folder…', run: actions.browse },
    { kind: 'separator' },
  ]

  if (recents.length === 0) {
    items.push({ id: 'empty', label: 'No recent projects', disabledReason: NO_RECENTS_REASON })
    return items
  }

  for (const entry of recents) {
    items.push({
      // Keyed and labelled by the path, not the name: two checkouts of one repository share a
      // basename, and a menu with `cide` in it twice cannot be used. The display form is
      // already `~`-abbreviated by Rust, so it fits.
      id: `recent:${entry.project.path}`,
      label: entry.project.displayPath,
      ...(entry.exists
        ? { run: () => actions.reopen(entry.project.path) }
        : { disabledReason: MISSING_REASON }),
    })
  }

  const missing = recents.filter((entry) => !entry.exists).length
  items.push({ kind: 'separator' })
  if (missing > 0) {
    items.push({
      id: 'forget-missing',
      label: missing === 1 ? 'Remove 1 missing project' : `Remove ${missing} missing projects`,
      run: actions.forgetMissing,
    })
  }
  items.push({ id: 'clear', label: 'Clear recent projects', danger: true, run: actions.clear })

  return items
}

// --- project tabs ---------------------------------------------------------------------------

/** The half of a project this menu needs. Satisfied by `AppHeader`'s `ProjectTab`. */
export interface ProjectLike {
  readonly id: string
  readonly displayPath: string
}

export interface ProjectTabActions {
  close?: ((id: ProjectId) => void) | undefined
  reveal?: ((id: ProjectId) => void) | undefined
  /** `undefined` when the webview exposes no clipboard, which disables Copy path. */
  copy?: ((text: string) => void) | undefined
}

export function projectTabEntries(
  projects: readonly ProjectLike[],
  project: ProjectLike,
  actions: ProjectTabActions,
): MenuEntry[] {
  const { close, reveal, copy } = actions
  const others = projects.filter((p) => p.id !== project.id)

  return [
    {
      id: 'close',
      label: 'Close project',
      ...(close
        ? { run: () => close(project.id as ProjectId) }
        : { disabledReason: NO_HOST }),
    },
    {
      id: 'close-others',
      label: others.length === 1 ? 'Close other project' : 'Close other projects',
      ...(close && others.length > 0
        ? { run: () => others.forEach((p) => close(p.id as ProjectId)) }
        : { disabledReason: others.length === 0 ? ONLY_PROJECT : NO_HOST }),
    },
    { kind: 'separator' },
    {
      id: 'reveal',
      label: 'Reveal in file manager',
      ...(reveal ? { run: () => reveal(project.id as ProjectId) } : { disabledReason: NO_HOST }),
    },
    {
      id: 'copy-path',
      label: 'Copy path',
      ...(copy
        ? { run: () => copy(project.displayPath) }
        : { disabledReason: NO_CLIPBOARD }),
    },
  ]
}

// --- workspace tabs -------------------------------------------------------------------------

export interface TabActions {
  close?: ((id: TabId) => void) | undefined
  /** Splits the **active** tab's focused pane. See `TabStripProps.onSplit`. */
  split?: (() => void) | undefined
  /** Detaches the **active** tab's focused pane. See `TabStripProps.onDetach`. */
  detach?: (() => void) | undefined
  copy?: ((text: string) => void) | undefined
}

/**
 * Whether a tab may be closed.
 *
 * Read off `kind`, never off the index, for the same reason `TabStrip` withholds the close
 * button that way: array order is a rendering detail, whereas the pin is a property of the tab.
 * This is still only the courtesy — `cide_core::workspace::close_tab` is the enforcement.
 */
export function closable(tab: Tab): boolean {
  return tab.kind.kind !== 'claudeHome'
}

/** The path a tab is *about*, or `null` when it is not about one. */
export function pathOf(tab: Tab): string | null {
  if (tab.kind.kind === 'file') return tab.kind.path
  if (tab.kind.kind === 'diff') return tab.kind.spec.newPath
  return null
}

/**
 * The workspace tab menu.
 *
 * Split and Detach are offered **only on the active tab**, and that is a deliberate refusal
 * rather than an oversight. Both handlers act on "the focused pane of the active tab" — that is
 * all `App.tsx` can express through a zero-argument callback — so running them from a
 * right-click on some other tab would quietly act somewhere the user was not pointing. An item
 * that says `Activate this tab first` is worse than one that works and better than one that
 * lies.
 */
export function tabMenuEntries(
  tabs: readonly Tab[],
  tab: Tab,
  activeTab: TabId,
  actions: TabActions,
): MenuEntry[] {
  const { close, split, detach, copy } = actions
  const index = tabs.indexOf(tab)
  const others = tabs.filter((t) => t !== tab && closable(t))
  const toRight = tabs.slice(index + 1).filter(closable)
  const path = pathOf(tab)
  const active = tab.id === activeTab

  return [
    {
      id: 'close',
      label: 'Close',
      command: 'tab.close',
      ...(close && closable(tab)
        ? { run: () => close(tab.id) }
        : { disabledReason: closable(tab) ? NO_HOST : PINNED_REASON }),
    },
    {
      id: 'close-others',
      label: 'Close others',
      ...(close && others.length > 0
        ? { run: () => others.forEach((t) => close(t.id)) }
        : { disabledReason: 'Nothing else here can be closed' }),
    },
    {
      id: 'close-right',
      label: 'Close to the right',
      ...(close && toRight.length > 0
        ? { run: () => toRight.forEach((t) => close(t.id)) }
        : { disabledReason: 'Nothing to the right can be closed' }),
    },
    { kind: 'separator' },
    {
      id: 'split',
      label: 'Split pane right',
      command: 'pane.split.right',
      ...(split && active ? { run: split } : { disabledReason: active ? NO_HOST : NOT_ACTIVE }),
    },
    {
      id: 'detach',
      label: 'Detach pane to its own window',
      ...(detach && active ? { run: detach } : { disabledReason: active ? NO_HOST : NOT_ACTIVE }),
    },
    { kind: 'separator' },
    {
      id: 'copy-path',
      label: 'Copy path',
      ...(path !== null && copy
        ? { run: () => copy(path) }
        : { disabledReason: path === null ? 'This tab is not a file' : NO_CLIPBOARD }),
    },
  ]
}

// --- the header's row controls --------------------------------------------------------------

export interface RowTarget {
  readonly project: ProjectId
  readonly tab: TabId
  /** The focused pane, which the new row is anchored under. */
  readonly after: PaneId
  readonly maximized: PaneId | null
}

/**
 * The project, tab and pane a row would be added to — or `null` when there is no such thing.
 *
 * Resolves the active project the way `App.tsx` does, **including the detached-window case**, so
 * a header in a window showing one project still names it instead of reporting none.
 */
export function rowTarget(boot: Bootstrap | null): RowTarget | null {
  if (boot === null) return null
  const projectId = boot.role.kind === 'shell' ? boot.role.active : boot.role.project
  if (projectId === null) return null
  const project = boot.workspace.projects[projectId]
  if (!project) return null
  const tab = project.tabs.find((t) => t.id === project.activeTab)
  if (!tab) return null
  return {
    project: project.id,
    tab: tab.id,
    after: tab.tree.focused,
    maximized: tab.tree.maximized,
  }
}

/**
 * Why `⊞ bash row` / `⊞ claude row` cannot be pressed, or `null` when they can.
 *
 * A **string or null** rather than a boolean, for two reasons. It is the sentence the tooltip
 * shows, which is what stops a greyed control being indistinguishable from a broken one — the
 * complaint that produced `disabledReason` in the menu model. And it is a primitive, so reading
 * it through a zustand selector does not re-render the header on every unrelated snapshot.
 *
 * Maximized is a refusal, not an un-maximize. `SplitTree`'s strip hid itself in that state
 * because adding a row the user cannot see is worse than not adding one; that argument survives
 * the move to the header, and saying it out loud is the improvement.
 */
export function rowGate(boot: Bootstrap | null): string | null {
  const target = rowTarget(boot)
  if (target === null) return NO_PROJECT_REASON
  return target.maximized === null ? null : MAXIMIZED_REASON
}
