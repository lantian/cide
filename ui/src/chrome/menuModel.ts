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
/** A tab that is about no file at all — the console, or Settings. */
export const NOT_A_FILE = 'This tab is not a file'
/**
 * A git diff tab, whose path is repo-relative.
 *
 * Its own sentence rather than [`NOT_A_FILE`], because a git diff tab plainly *is* about a file
 * and telling the user it is not would be false. What it lacks is an **absolute** path — see
 * [`absolutePathOf`].
 */
export const RELATIVE_DIFF_PATH =
  'A diff tab names a path inside a repository, not a file on disk'

// --- recent projects ------------------------------------------------------------------------

/** The half of `RecentEntry` this file needs. Structurally satisfied by the generated type. */
export interface RecentLike {
  readonly project: { readonly path: string; readonly displayPath: string }
  readonly exists: boolean
}

export interface RecentActions {
  browse: () => void
  /** The New project wizard (M97), right under *Open folder…*. */
  create: () => void
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
    { id: 'new', label: 'New project…', run: actions.create },
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
  /**
   * Close several tabs as one gesture — what the three bulk items below run.
   *
   * A **separate action from `close`**, rather than the model calling `close` in a loop, and the
   * separation is the fix for a real defect rather than tidiness. Looping meant one close per
   * tab, one refusal per dirty tab, and one queued dialog per refusal: closing eight tabs with
   * three unsaved files put three modals in front of the user, each headed "Closing *this tab*",
   * for a gesture they made once. The store's `closeTabs` asks once and names everything at
   * stake; keeping the two doors distinct at the type level is what stops the loop coming back.
   *
   * All three bulk items require it. Falling back to `close` when it is absent was the
   * alternative and it loses for the reason this whole indirection exists: it would leave two
   * behaviours in the app for one gesture, and the wrong one would be the one that survives in
   * whichever surface nobody re-tested.
   */
  closeMany?: ((ids: TabId[]) => void) | undefined
  /** Splits the **active** tab's focused pane. See `TabStripProps.onSplit`. */
  split?: (() => void) | undefined
  /** Detaches the **active** tab's focused pane. See `TabStripProps.onDetach`. */
  detach?: (() => void) | undefined
  copy?: ((text: string) => void) | undefined
  /**
   * *Show history for this file* — opens a tab in the git tool window. (M18)
   *
   * Takes an **absolute** path, which is why the item is built from [`absolutePathOf`] and not
   * from [`pathOf`]: the command resolves it to a repository through `git_locate`, and a
   * repo-relative string would resolve to nothing.
   *
   * Absent in a window with no tool window, which is every detached-pane window.
   */
  history?: ((path: string) => void) | undefined
  /**
   * *Properties* — raises the file properties card. (M70)
   *
   * Takes an **absolute** path, for [`history`]'s reason exactly: the card stats it and asks
   * `git_locate` about it, and a repo-relative string is not a path to either.
   *
   * Unlike [`history`] this is present in **every** window, because the card is an overlay and
   * `file.properties` carries no `shellWindow` clause — `git.blame`'s precedent, argued at the
   * command's own definition. The one control on the card that needs a tool window is the one
   * that is conditionally drawn, not the whole gesture.
   */
  properties?: ((path: string) => void) | undefined
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
  if (tab.kind.kind === 'revision') return tab.kind.path
  return null
}

/**
 * The **absolute** path a tab is about, or `null`.
 *
 * [`pathOf`] plus the one test `keys/target.ts::focusedTabPath` documents and this module did
 * not: `DiffSpec.newPath` is absolute for a `ClaudeMcp` diff, which names files on disk, and
 * **repo-relative** for a `Git` one, because that is how git spells a path and how the fetch key
 * spells it. `TabKind::Revision` is repo-relative for the same reason.
 *
 * That distinction is not academic and it is already costing something: *Copy path* on a git diff
 * tab copies `src/main.rs` rather than a path anything can open. Any item that hands a path to a
 * command — *Show history for this file* is the first — must use this one instead, or it names a
 * path the backend cannot resolve.
 *
 * A leading `/` is the test, which is exactly what `focusedTabPath` uses: cide is Linux-first and
 * every absolute path it deals in begins with one, while every repo-relative path it deals in is
 * slash-separated and does *not*.
 */
export function absolutePathOf(tab: Tab): string | null {
  if (tab.kind.kind === 'file') return tab.kind.path
  if (tab.kind.kind === 'diff') {
    const path = tab.kind.spec.newPath
    return path.startsWith('/') ? path : null
  }
  // A revision tab's path is always repo-relative — it names a blob in the object database, not
  // a file on disk, and at an old revision the file may not be on disk at all.
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
  const { close, closeMany, split, detach, copy, history, properties } = actions
  const index = tabs.indexOf(tab)
  const others = tabs.filter((t) => t !== tab && closable(t))
  /*
   * The three bulk sets, and every one of them is filtered by `closable`.
   *
   * That filter is what makes the pinned console disappear from all three without any of them
   * knowing it exists. `toLeft` is the set that would otherwise get it wrong: the console is at
   * index 0, so it is to the left of *everything*, and a `slice(0, index)` alone would offer
   * "Close to the left" on the first file tab and then fail against `CoreError::TabPinned` — a
   * menu entry that errors, which the brief for this change rightly calls worse than one that is
   * not offered. Reusing `closable` rather than testing `index === 0` is the same discipline
   * `TabStrip` applies to the close button: the pin is a property of the tab, not of its
   * position, and reordering is now a gesture that makes the position lie.
   */
  const toLeft = tabs.slice(0, index).filter(closable)
  const toRight = tabs.slice(index + 1).filter(closable)
  const path = pathOf(tab)
  const absolute = absolutePathOf(tab)
  const active = tab.id === activeTab
  /** Run a bulk close, or say why the line is off. Shared by the three items below. */
  const bulk = (set: readonly Tab[], nothing: string) =>
    closeMany && set.length > 0
      ? { run: () => closeMany(set.map((t) => t.id)) }
      : { disabledReason: set.length === 0 ? nothing : NO_HOST }

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
      ...bulk(others, 'Nothing else here can be closed'),
    },
    /*
     * Left before right, because that is the order they are on screen and a menu that lists them
     * the other way makes the reader translate.
     *
     * Offered-and-disabled rather than dropped when the set is empty, the rule `close-right`
     * already follows: an item that comes and goes with the tab order is one the user has to
     * hunt for. That matters more here than for its sibling, because the empty state is the
     * *common* one — every tab at index 1 has only the pinned console to its left — so the
     * sentence has to explain rather than merely refuse, and "Nothing to the left can be closed"
     * is true of a lone console in a way that "there is nothing to the left" would not be.
     */
    {
      id: 'close-left',
      label: 'Close to the left',
      ...bulk(toLeft, 'Nothing to the left can be closed'),
    },
    {
      id: 'close-right',
      label: 'Close to the right',
      ...bulk(toRight, 'Nothing to the right can be closed'),
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
        : { disabledReason: path === null ? NOT_A_FILE : NO_CLIPBOARD }),
    },
    {
      // Carries the command id, so the row draws whatever chord the live keymap has for it —
      // nothing today, which is correct: this is the same *act* as the palette row, unlike
      // `mention` above, whose long comment explains why it deliberately carries no chip.
      id: 'history',
      label: 'Show history for this file',
      command: 'git.history.file',
      ...(absolute !== null && history
        ? { run: () => history(absolute) }
        : {
            disabledReason:
              // Three ways to be unavailable and three different sentences, because "not
              // available" tells the user nothing they can act on. The middle one is the
              // interesting case: a git diff tab *is* about a file, but its path is
              // repo-relative and naming it to a command that wants a file on disk would be
              // naming a path that is not a path. See `absolutePathOf`.
              path === null ? NOT_A_FILE : absolute === null ? RELATIVE_DIFF_PATH : NO_HOST,
          }),
    },
    {
      // Carries the command id for the chord chip, like `history` above — nothing today, and
      // correct: this is the same act as the palette row.
      //
      // Two refusal sentences and not three. `NO_HOST` cannot arise here the way it does for
      // `history`: the card is an overlay, so every window that draws this menu can raise one,
      // and a caller that omitted the dep would be a wiring bug rather than a window without a
      // tool window. It still reads as a sentence if it ever happens.
      id: 'properties',
      label: 'Properties',
      command: 'file.properties',
      ...(absolute !== null && properties
        ? { run: () => properties(absolute) }
        : {
            disabledReason:
              path === null ? NOT_A_FILE : absolute === null ? RELATIVE_DIFF_PATH : NO_HOST,
          }),
    },
  ]
}

// --- tabs the strip is hiding -----------------------------------------------------------------

/**
 * The one thing the overflow list may do.
 *
 * There is deliberately **no `close` and no `closeMany` here**, and the missing fields are the
 * enforcement rather than a comment asking nicely. The requirement is that this control never
 * closes anything: it exists because tabs past the right edge are unreachable, and a close
 * button on a line naming a tab the user cannot see is a destructive action aimed at something
 * invisible. Expressed as an actions bag with one member, a future edit that copies
 * [`TabActions`]'s shape into here has to *add a field* to violate it, which is a change a
 * reviewer sees. Expressed as a comment on a five-field bag, it lasts until the first copy.
 *
 * (The pin is enforced in Rust either way — `cide_core::workspace::close_tab` refuses the
 * console. This is about not offering the gesture at all.)
 */
export interface OverflowActions {
  activate?: ((id: TabId) => void) | undefined
}

/**
 * What one hidden tab is called in the list.
 *
 * The vocabulary is `TabStrip.viewFor`'s and `Switcher.tabRow`'s — "Claude" for the console,
 * the basename for a file — because the same tab appearing under two different names in two
 * surfaces of one app is a small lie the user has to reconcile. Restated rather than imported
 * for the reason `Switcher` gives about the same duplication: `viewFor` returns JSX bound to the
 * strip's own CSS modules, and importing it here would drag a stylesheet into a file that a bare
 * `tsc` has to compile alone.
 */
function overflowLabel(tab: Tab): string {
  switch (tab.kind.kind) {
    case 'claudeHome':
      // `chrome/consoleName.ts`'s rule, restated for this file's reason above: a check compiles
      // it alone and runs it under Node, where a value import does not resolve. (M93)
      // Read through `?.`: the check's fixtures build tabs without a tree.
      return Object.values(tab.tree?.panes ?? {}).some(
        (pane) => pane?.role === 'primary' && pane.harness === 'codex',
      )
        ? 'Codex'
        : 'Claude'
    case 'claudeFull':
      return tab.kind.title
    case 'file':
      return basename(tab.kind.path)
    case 'diff':
      return tab.kind.spec.title
    case 'settings':
      return 'Settings'
    case 'docs':
      return tab.kind.title
    default:
      // A `TabKind` variant added later. A line that says *something* beats a hole in the list;
      // `TabStrip.viewFor` is where the compiler is made to care about the new variant.
      return 'Tab'
  }
}

function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1)
}

/**
 * The `▾` menu at the right end of the tab strip: the tabs that are currently out of view.
 *
 * `hidden` is measured off the DOM by `tabOverflow.clippedTabs`; this function turns it into
 * prose and enablement. The split is the same one `tabDrag.ts`/`useTabDrag.ts` make and for the
 * same reason — geometry in the import-free module its own check compiles, wording and
 * enablement in the menu model whose check already exists.
 *
 * Three rules that are not obvious:
 *
 * * **An id naming no tab is skipped.** The list is measured at layout time and the menu is
 *   built at open time, and `cide://workspace-changed` can close a tab in between — a background
 *   window, a hook, another window's ⌘W. The same staleness `tabDrag.dropOutcome` guards against
 *   by looking the id up rather than trusting it.
 * * **Order is the strip's**, not the measurement's, so the list reads the way the strip does.
 *   `hidden` already arrives in strip order; iterating `tabs` and filtering makes that true by
 *   construction rather than by the caller remembering.
 * * **Duplicate labels fall back to the whole path.** Four clipped tabs reading
 *   `mod.rs / mod.rs / mod.rs / mod.rs` is a menu that answers nothing, and the case is not
 *   exotic — it is what a Rust or a Go tree looks like. Only the colliding entries expand, so
 *   one `main.rs` beside a `Cargo.toml` stays short.
 */
export function overflowEntries(
  tabs: readonly Tab[],
  hidden: readonly string[],
  actions: OverflowActions,
): MenuEntry[] {
  const { activate } = actions
  const wanted = new Set(hidden)
  const listed = tabs.filter((tab) => wanted.has(tab.id))

  // How many listed tabs would wear each label. Counted over the *listed* set only: a collision
  // with a tab that is comfortably on screen is not a collision the reader of this menu can see.
  const seen = new Map<string, number>()
  for (const tab of listed) {
    const label = overflowLabel(tab)
    seen.set(label, (seen.get(label) ?? 0) + 1)
  }

  return listed.map((tab) => {
    const label = overflowLabel(tab)
    const collides = (seen.get(label) ?? 0) > 1
    const path = pathOf(tab)
    return {
      // Prefixed, because a menu's ids only have to be unique within the menu but they are also
      // what a check script asserts on, and a bare tab id would read as though the entry *were*
      // the tab rather than a way to reach it.
      id: `overflow:${tab.id}`,
      label: collides && path !== null ? path : label,
      ...(activate
        ? { run: () => activate(tab.id) }
        : // The chrome audit renders a handler-free `<TabStrip>`; a live-looking line that does
          // nothing on click is the failure this whole model exists to make unrepresentable.
          { disabledReason: NO_HOST }),
    }
  })
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
