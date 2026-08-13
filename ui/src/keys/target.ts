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
 *   rather than in any tab's tree. [`activeTabOf`] and so [`focusTarget`] answer `null`
 *   there: a pane that is not in a tree cannot be split, closed or navigated away from, and
 *   every `pane_*` command takes the tab that holds it.
 *
 *   That guard is not defensive tidiness, it is the whole reason this note exists. A
 *   `detachedPane` role carries `tab` — the tab the pane came *out of* — and that tab is
 *   still in the shell window. Reading it here made `focusTarget` resolve, in the detached
 *   window, to whatever pane the *shell* window had focused, and `App.tsx` installs the key
 *   gate before it branches on the role. Ctrl+W in a detached pane therefore closed a tab in
 *   another window, and Ctrl+Alt+arrow moved focus over there.
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
  // A detached-pane window shows no tab at all, so it must not name one. `role.tab` here
  // is the tab the pane was torn out of and it belongs to the shell window — see the module
  // note. Answering `null` is what keeps every tab and pane command in this window reporting
  // an unmet precondition instead of quietly acting on the other one.
  if (role.kind === 'detachedPane') return null
  const wanted = role.kind === 'shell' ? project.activeTab : role.tab
  return project.tabs.find((tab) => tab.id === wanted) ?? null
}

/**
 * The projects in *this window's* header strip, in header order.
 *
 * `role.projects`, not `workspace.projects`, and the difference is only invisible in the
 * default mode. In `WindowMode::PerProject` the workspace holds every project while each
 * shell window's strip holds exactly one, and `workspace::activate_project` only touches
 * windows whose strip contains the id — so cycling the workspace list there sends
 * `project_activate` for a project this window does not hold, which sets `active` on some
 * *other* window and leaves this one exactly as it was. A keystroke that does nothing, which
 * is the defect this round exists to remove.
 *
 * Empty for a detached window: it has no strip, so nothing to cycle.
 */
export function windowProjectsOf(boot: Bootstrap | null): readonly ProjectId[] {
  return boot !== null && boot.role.kind === 'shell' ? boot.role.projects : []
}

/**
 * Whether the tab this window shows can actually be closed.
 *
 * `tabs[0]` is the pinned project console and `cide_core::workspace::close_tab` answers
 * `TabPinned` for it. Shared by the `closableTab` flag and by `tab.close`'s handler on
 * purpose: the gate evaluates `Binding::when`, never `Command::when`, so the clause hides
 * the palette row but does not stop Ctrl+W — and the handler has to enforce the same fact or
 * the two disagree.
 */
export function isClosableTab(boot: Bootstrap | null): boolean {
  const project = activeProjectOf(boot)
  const tab = activeTabOf(boot)
  return project !== null && tab !== null && project.tabs[0]?.id !== tab.id
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

/*
 * There is deliberately no `reposOf` here any more, and no repository fact of any kind.
 *
 * It existed, it collected `ProjectRoot.repo` across a project's roots, and it returned `[]`
 * for every project any build has ever opened — Rust set that field to `None` in its one
 * constructor and nowhere else set it at all (see `cide_ipc::workspace::ProjectRoot`, which no
 * longer has the field). `context.ts` derived `repoOpen` from the length of that list, every
 * git command was gated on `repoOpen`, and so the whole Git group *and* `sidebar.git` were
 * filtered out of the command palette for everybody, for ever, with nothing failing anywhere.
 *
 * Which repositories a project contains is a question about the disk — `git init` in a bash
 * pane changes the answer, and no event this side subscribes to reports it — so it is asked
 * over IPC at the moment a command runs, by `git_repos`. A mirror of it here would be a cache
 * with no invalidation, which is the same bug again with a fresher-looking value in it.
 */
