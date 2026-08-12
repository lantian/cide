/**
 * What "focused" means, once, for both the `when` clause and the handler that runs.
 *
 * A command that acts on a pane is offered by the palette when its clause says a pane is
 * focused, and then acts on the pane its handler finds. If those two derive "the focused
 * pane" separately they will one day disagree, and the visible symptom is the one this whole
 * round is about: the row was enabled, the key was swallowed, and nothing happened. So the
 * clause in `keys/context.ts` and the handler in `keys/dispatch.ts` both call these.
 *
 * Pure functions over a `Bootstrap` — no store, no hooks — so `App.tsx` is not in the path
 * and a fixture can drive them. Reading the mirror is what makes the dispatcher reachable
 * from the key gate, which runs outside React and cannot be handed props.
 *
 * # Window roles
 *
 * All three are handled and they are not the same:
 *
 * * `shell` — the project in `role.active`, its `activeTab`, that tab's focused pane.
 * * `detachedTab` — one tab in its own window; the project and tab come from the role.
 * * `detachedPane` — one pane in its own window, and its pane lives in `project.detached`
 *   rather than in any tab's tree. [`focusTarget`] answers `null` there on purpose: a pane
 *   that is not in a tree cannot be split, closed or navigated away from, and every
 *   `pane_*` command takes the tab that holds it.
 */
import type { Bootstrap, Pane, PaneId, Project, ProjectId, Tab } from '@/ipc/client'

/** A pane that is in a tab that is in a project — everything a `pane_*` command needs. */
export interface FocusTarget {
  project: ProjectId
  tab: Tab
  pane: Pane
}

/** The project this window is showing, or `null`. */
export function activeProjectOf(boot: Bootstrap | null): Project | null {
  const id = activeProjectIdOf(boot)
  if (boot === null || id === null) return null
  return boot.workspace.projects[id] ?? null
}

/** The id of the project this window is showing, or `null`. */
export function activeProjectIdOf(boot: Bootstrap | null): ProjectId | null {
  if (boot === null) return null
  const role = boot.role
  return role.kind === 'shell' ? role.active : role.project
}

/** The tab this window is showing, or `null`. */
export function activeTabOf(boot: Bootstrap | null): Tab | null {
  const project = activeProjectOf(boot)
  if (boot === null || project === null) return null
  const role = boot.role
  const wanted = role.kind === 'shell' ? project.activeTab : role.tab
  return project.tabs.find((tab) => tab.id === wanted) ?? null
}

/** The focused pane, with the tab and project that own it, or `null`. */
export function focusTarget(boot: Bootstrap | null): FocusTarget | null {
  const project = activeProjectOf(boot)
  const tab = activeTabOf(boot)
  if (project === null || tab === null) return null
  const pane = tab.tree.panes[tab.tree.focused]
  return pane === undefined ? null : { project: project.id, tab, pane }
}

/**
 * The Claude pane an `@`-mention should land in, or `null`.
 *
 * The focused pane when it is a Claude one, otherwise the project's console — `tabs[0]`'s
 * Claude pane, which is the conversation the project is *about*. The fallback is the whole
 * point: the gesture is made from an editor, where by definition no Claude pane is focused,
 * so without it the command would be inapplicable exactly when it is wanted. Same rule as
 * `editor/mentionTarget.ts`, which states it for the editor's own context menu.
 */
export function claudeTargetOf(boot: Bootstrap | null): { project: ProjectId; pane: PaneId } | null {
  const project = activeProjectOf(boot)
  if (project === null) return null

  const tab = activeTabOf(boot)
  const focused = tab === null ? undefined : tab.tree.panes[tab.tree.focused]
  if (focused?.kind === 'claude') return { project: project.id, pane: focused.id }

  const console = project.tabs[0]
  if (console === undefined) return null
  const pane = Object.values(console.tree.panes).find((p) => p.kind === 'claude')
  return pane === undefined ? null : { project: project.id, pane: pane.id }
}

/**
 * The absolute path of the file in the focused tab, or `null` when it is not a file tab.
 *
 * `file.reveal` and `claude.mention.file` both name a path, and both are reached from the
 * palette, which passes no arguments at all. Before this they read `args.path` and did
 * nothing when it was absent — a palette row that works only when a key with a hand-written
 * `args` in `keymap.json` runs it. The tab's own `TabKind::File` carries the path, so the
 * argument is now an override rather than a requirement.
 */
export function focusedFilePath(boot: Bootstrap | null): string | null {
  const tab = activeTabOf(boot)
  if (tab === null || tab.kind.kind !== 'file') return null
  return tab.kind.path
}

/** Every repository in the project this window is showing, in root order. */
export function reposOf(boot: Bootstrap | null): string[] {
  const project = activeProjectOf(boot)
  if (project === null) return []
  const seen: string[] = []
  for (const root of project.roots) {
    if (root.repo !== null && !seen.includes(root.repo)) seen.push(root.repo)
  }
  return seen
}
