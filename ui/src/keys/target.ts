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

  return consolePaneOf(boot)
}

/**
 * The project console's Claude pane — `tabs[0]`'s, unconditionally.
 *
 * The fallback branch of [`claudeTargetOf`], lifted out because `tab.console` (Ctrl+1) wants
 * *only* that branch: "go to the Claude console" means the console whether or not some other
 * Claude pane happens to have focus, where a mention means "the conversation I am in, or failing
 * that the project's". Two questions, one of which is a special case of the other, and the way
 * that goes wrong is a second copy of `tabs[0]` and the pane walk drifting from this one.
 *
 * `tabs[0]` is the pinned console by invariant — `open_tab` refuses a second `ClaudeHome`,
 * `close_tab` refuses index 0, and `close_pane` refuses its primary pane — so the `undefined`
 * arms below are the ones that fire only while a window has no project.
 */
export function consolePaneOf(boot: Bootstrap | null): { project: ProjectId; pane: PaneId } | null {
  const project = activeProjectOf(boot)
  if (project === null) return null
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

/**
 * The absolute path the focused tab is *about*, or `null` when it is about no file.
 *
 * Wider than [`focusedFilePath`] by exactly one case: a **diff** tab, which names the file it is
 * diffing. That is what *Select opened file* means when the thing on screen is a diff of
 * `src/main.rs` — IDEA reveals `src/main.rs` — and the alternative is a command that goes dead
 * on a tab where the answer is obvious.
 *
 * And only when that name is **absolute**, which is the half a reader will assume is missing.
 * `DiffSpec.newPath` is absolute for a `ClaudeMcp` diff, which names files on disk, and
 * *repo-relative* for a `Git` one — its own doc comment says so — because that is how git
 * spells a path. Handing a relative string to `fs_reveal` would find nothing and the command
 * would report that a file *is not in this project's file tree* while naming a path that is not
 * a path. Answering `null` instead makes `fileTabActive` false there, so the palette hides the
 * row and the chord says there is no file tab, which are both true.
 *
 * `focusedFilePath` is deliberately left as it was rather than widened, because its other caller
 * is `claude.mention.file` and "mention the file I am looking at" over a diff is a different
 * question with a different answer (which side?), which nothing has asked yet.
 *
 * The two arms mirror `chrome/menuModel.ts::pathOf`, which cannot be imported here: that module
 * is compiled standalone by `check-menu-model.mjs` and its header forbids a value import, so a
 * dependency in this direction would eventually be reversed by somebody. `check:select-opened`
 * asserts the two agree.
 *
 * # What this is the *only* honest source for
 *
 * There is no most-recently-used file **here**, and there must not be one: a "last file tab"
 * fallback in this function would be a webview-side cache with no invalidation, which is
 * verbatim the `repoOpen` mistake recorded at the foot of this file. A tab that is not about a
 * file answers `null`, and the handler says so.
 *
 * The rule is about *caches*, not about the idea of an order, and M14 drew the line where it
 * belongs. `store/workspace.ts` now keeps a per-project tab MRU stack, and it is admissible for
 * precisely the reason the sentence above is not: it is re-derived from **every**
 * `cide://workspace-changed` snapshot and reconciled against `project.tabs`, over
 * `Project::active_tab` — a field Rust writes on every open, every activation and every close.
 * That is the invalidation this note demands. `ProjectRoot::repo`, by contrast, was a constant.
 * Deriving a *file* answer from it here would still be wrong, because the question this function
 * answers is "what is the tab in front of me about", which has one honest source.
 */
export function focusedTabPath(boot: Bootstrap | null): string | null {
  const tab = activeTabOf(boot)
  if (tab === null) return null
  if (tab.kind.kind === 'file') return tab.kind.path
  // Absolute only — see above. A leading `/` is the whole test: cide is Linux-first and every
  // path on the wire is POSIX, which is the same shape `groupRows.isSyntheticPath` keys on.
  if (tab.kind.kind === 'diff' && tab.kind.spec.newPath.startsWith('/')) {
    return tab.kind.spec.newPath
  }
  return null
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
