/**
 * Which tabs the git tool window has, which one is in front, and what each of them is called.
 *
 * Import-free on purpose, exactly as `chrome/sidebarView.ts` and `chrome/sidebarWidth.ts` are and
 * for the same reason: `ui/scripts/check-toolwindow.mjs` compiles this file standalone with the
 * TypeScript already in `node_modules` and *executes* it. A rule that lives in a React state
 * updater is a rule no check script can compile, and this project has paid for that five times.
 *
 * [`HistoryTab`] is therefore a structural restatement of the generated DTO rather than an import
 * of it; `check-toolwindow.mjs` pins the field names against `ui/src/ipc/generated.ts`, which is
 * the join that catches a Rust rename before it shows up as `undefined` in a tab row.
 *
 * # The Log tab is a position, not an entity
 *
 * It is always there, always first, and can never be closed, so it has no id and does not live in
 * [`ToolWindowTabs::history`]. `active: null` *is* "the Log tab". The alternative — giving it an
 * id and putting it at `history[0]` — makes every close path carry a "unless it is the first one"
 * clause, and the first time somebody writes `history.filter(...)` without that clause the panel
 * loses its only permanent tab.
 */

/**
 * The Docker tab's id. (M46)
 *
 * A sentinel string, the way `ext:` ids are, and for the same reason: `ToolWindowTabs::active` is
 * a `HistoryTabId` — a uuid — everywhere it reaches Rust, so anything that is not one must never
 * be sent. [`isDockerTab`] is the guard, and `ToolWindowHost` holds this tab's selection in the
 * webview exactly as it holds a contributed tab's.
 *
 * # Why the Docker tab is not persisted, when it easily could be
 *
 * A contributed tab is not persisted because there may be no such extension next launch. That
 * argument does not apply here — Docker cannot be uninstalled — so the real reason is narrower
 * and worth stating: `ToolWindowState::active` is typed as an optional uuid in `workspace.json`,
 * and widening it to carry a builtin sentinel is a wire change, a migration, and a new way for a
 * stored value to be invalid. The Log tab is the right thing to come back to, and Docker is one
 * click from it.
 */
export const DOCKER_TAB = 'docker'

/** Whether an id addresses the Docker tab rather than one of Rust's. */
export function isDockerTab(id: string | null): boolean {
  return id === DOCKER_TAB
}

/** One open per-file History tab. A structural restatement of `HistoryTab` in `generated.ts`. */
export interface HistoryTab {
  readonly id: string
  readonly repo: string
  /** Repo-relative and slash-separated, the way `cide_ipc::git` spells every path. */
  readonly path: string
  /** The basename, so the row can be drawn before the query answers. */
  readonly title: string
}

/**
 * One tab an extension contributes. (M22)
 *
 * Not a [`HistoryTab`] and not stored anywhere: `ToolWindowState` lives on `Project::tool_window`
 * in `workspace.json`, and a contributed tab has nothing durable in it. Its content is a view
 * model its worker rebuilds from scratch on every launch, so persisting *which* one was open would
 * mean restoring a tab whose extension may since have been disabled or uninstalled — a tab that
 * draws a sentence about not being available, restored on every launch until somebody closes it.
 *
 * That is `chrome/sidebarView.ts`'s decision, arrived at the same way and for a sharper reason:
 * there the sidebar's hidden state is not persisted because it would cross windows wrongly, here
 * a contributed tab is not persisted because there is nothing to persist.
 */
export interface ExtTab {
  /** `ext:<marketplace>.<extension>.<panel>` — the same id the rail addresses a sidebar panel by. */
  readonly view: string
  readonly label: string
}

/** The tab row's whole state. */
export interface ToolWindowTabs {
  readonly open: boolean
  readonly history: readonly HistoryTab[]
  /**
   * Tabs contributed by enabled extensions, in registry order.
   *
   * Not part of the stored state — see [`ExtTab`] — so this is filled in by the host from the
   * extension store rather than from the workspace snapshot. Defaulted, so every existing caller
   * that builds a `ToolWindowTabs` from a `ToolWindowState` alone keeps working.
   */
  readonly ext?: readonly ExtTab[]
  /** `null` is the Log tab. A `ext:…` string is a contributed tab. */
  readonly active: string | null
}

/** Closed, with only the Log tab, which is what a project with no saved state gets. */
export const TABS_INITIAL: ToolWindowTabs = { open: false, history: [], active: null }

/**
 * The id the Log tab walks under, since it has none of its own.
 *
 * `LogRegistry` in `cide-app` is keyed `(ProjectId, ToolTabId)` — per tab and not per project, so
 * that opening a History tab does not cancel the Log's walk — which means the Log tab needs *an*
 * id even though the section above argues at length that it must not *have* one. This is the seam
 * between those two facts: the Log tab stays a position everywhere it is stored, and becomes a
 * uuid only at the moment it addresses the registry.
 *
 * It must parse as a uuid, because `ToolTabId` is `serde(transparent)` over one and a value that
 * does not parse is rejected by the command rather than by anything visible. The nil uuid is the
 * one such value that can never collide with a `HistoryTabId`, which is always minted from v4.
 *
 * Constant, and that is the load-bearing part: an id that changed between renders would make two
 * remounts look like two tabs, and neither would cancel the other's walk.
 */
export const LOG_TAB_ID = '00000000-0000-0000-0000-000000000000'

/** Which tab id a walk runs under: a History tab's own, or [`LOG_TAB_ID`] for the Log tab. */
export function walkTab(active: string | null): string {
  return active ?? LOG_TAB_ID
}

/** A row the tab strip draws. */
export interface TabRow {
  /** `null` for the Log tab, matching [`ToolWindowTabs::active`]. */
  readonly id: string | null
  readonly label: string
  /** The tooltip: the full path for a history tab, so a truncated basename is still legible. */
  readonly title: string
  readonly active: boolean
  readonly closable: boolean
}

/** The basename, which is what a tab is called. */
export function historyTitle(path: string): string {
  const cut = path.lastIndexOf('/')
  const name = cut === -1 ? path : path.slice(cut + 1)
  return name.length === 0 ? path : name
}

/** The rows to draw, in order: the Log tab, then history tabs in the order they were opened. */
export function tabRow(tabs: ToolWindowTabs): TabRow[] {
  const rows: TabRow[] = [
    {
      id: null,
      label: 'Log',
      title: 'The commit log',
      active: tabs.active === null,
      closable: false,
    },
  ]
  // Docker, immediately after Log and before anything the user opened. (M46)
  //
  // Second rather than last, which is the opposite of where contributed tabs go — and the
  // difference is who put the tab there. A contributed tab appears because an extension was
  // enabled, so it must not push the user's own history tabs along; Docker is a fixture of the
  // panel like Log, and a fixture that moved as history tabs came and went would be a target that
  // is never in the same place twice. Not closable, for the same reason Log is not.
  rows.push({
    id: DOCKER_TAB,
    label: 'Docker',
    title: "This machine's containers and images",
    active: isDockerTab(tabs.active ?? null),
    closable: false,
  })
  for (const tab of tabs.history) {
    rows.push({
      id: tab.id,
      label: tab.title,
      title: tab.path,
      active: tabs.active === tab.id,
      closable: true,
    })
  }
  // Contributed tabs last, after every history tab. (M22)
  //
  // Last rather than immediately after Log, because the history tabs are the ones the *user*
  // opened and a tab appearing between two of them when an extension is enabled would move
  // something they put there. Not closable: a contributed tab is a property of which extensions
  // are enabled, and a close that only lasted until the next launch is a control that does not
  // do what it says.
  for (const tab of tabs.ext ?? []) {
    rows.push({
      id: tab.view,
      label: tab.label,
      title: tab.label,
      active: tabs.active === tab.view,
      closable: false,
    })
  }
  return rows
}

/** Whether a tab id names a contributed tab rather than a history one. */
export function isExtTab(id: string | null): boolean {
  return id !== null && id.startsWith('ext:')
}

/** The open History tab for this file, or `null`. Keyed on the pair, because a path alone is
 * ambiguous in a multi-root project where two repositories both contain `src/main.rs`. */
export function findHistory(
  tabs: ToolWindowTabs,
  repo: string,
  path: string,
): HistoryTab | null {
  return tabs.history.find((t) => t.repo === repo && t.path === path) ?? null
}

/**
 * Show a file's history: **open-or-activate, never append a duplicate.**
 *
 * Four separate menus reach this — the file tab strip, the file tree, the git changes tree and
 * the editor's own context menu — and they reach it for the same file often. Without the lookup,
 * right-clicking one file in three places gives three identical tabs, each re-running the same
 * walk. `id` is only consumed when the tab is genuinely new, so the caller may mint one
 * unconditionally.
 *
 * Always reveals. Every caller got here by asking to *see* something, so a version of this that
 * left a closed panel closed would be a command that sometimes did nothing visible — the argument
 * `sidebarView.ts::showPanel` makes for the sidebar's commands.
 */
export function openHistory(
  tabs: ToolWindowTabs,
  id: string,
  repo: string,
  path: string,
): ToolWindowTabs {
  const existing = findHistory(tabs, repo, path)
  if (existing !== null) return { ...tabs, open: true, active: existing.id }
  const tab: HistoryTab = { id, repo, path, title: historyTitle(path) }
  return { ...tabs, open: true, history: [...tabs.history, tab], active: id }
}

/** Bring a tab to the front, revealing the panel. `null` is the Log tab. */
export function activate(tabs: ToolWindowTabs, id: string | null): ToolWindowTabs {
  if (id !== null && !tabs.history.some((t) => t.id === id)) return tabs
  return { ...tabs, open: true, active: id }
}

/**
 * Close one History tab.
 *
 * **The successor is the left neighbour, then the right, then the Log tab — and never "nothing".**
 * A panel that vanished because you closed one of its tabs reads as a crash rather than as a
 * close, and the Log tab is always there to fall back to, so the closed state is unreachable from
 * this gesture. `close_tab`'s left-neighbour rule in `cide-core::workspace` is the same choice for
 * the same reason: the tab you were reading before is a better guess than the one after it.
 *
 * Closing a tab that is not in front leaves `active` alone.
 */
export function closeHistory(tabs: ToolWindowTabs, id: string): ToolWindowTabs {
  const index = tabs.history.findIndex((t) => t.id === id)
  if (index === -1) return tabs
  const history = tabs.history.filter((t) => t.id !== id)
  if (tabs.active !== id) return { ...tabs, history }
  const left = index > 0 ? history[index - 1] : undefined
  const right = history[index]
  return { ...tabs, history, active: left?.id ?? right?.id ?? null }
}

/** Show the Log tab, revealing the panel. What the `git.log` command does. */
export function showLog(tabs: ToolWindowTabs): ToolWindowTabs {
  return { ...tabs, open: true, active: null }
}

/** The rail button and `view.toolWindow.toggle`. Keeps the tab set either way. */
export function toggle(tabs: ToolWindowTabs): ToolWindowTabs {
  return { ...tabs, open: !tabs.open }
}

/**
 * A persisted state made safe to render.
 *
 * Two repairs, both of states `cide_core::workspace::validate` refuses to write but a hand-edited
 * or older `workspace.json` can still hold. An `active` naming no tab would draw an empty body
 * with nothing lit and no way back except closing the panel; a duplicate id would give two rows
 * one React key and make the second unclickable.
 */
export function restoreTabs(tabs: ToolWindowTabs): ToolWindowTabs {
  const seen = new Set<string>()
  const history = tabs.history.filter((t) => {
    if (seen.has(t.id)) return false
    seen.add(t.id)
    return true
  })
  const active = tabs.active !== null && seen.has(tabs.active) ? tabs.active : null
  return { open: tabs.open === true, history, active }
}
