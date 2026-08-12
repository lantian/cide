/**
 * What has to change for a pane to be on screen — the arithmetic, with nothing that can move.
 *
 * > *"send lines to Claude also should switch to console that added a selected text"*
 *
 * The mention lands in a Claude pane's prompt. `useMentionTarget` picks that pane, and its
 * fallback is *the project's console — its first tab's Claude pane*, so the destination is
 * routinely a tab the user is not looking at and sometimes a window they are not looking at.
 * A prompt the user cannot see is indistinguishable from a send that did nothing, which is
 * the complaint this whole capability was reported under two rounds ago in a different
 * disguise.
 *
 * This module answers one question — *given the workspace, what stands between this window
 * and that pane?* — and answers it as a plan rather than by doing anything. Import-free for
 * the same reason `sendToClaude.ts` is: `check-editor.mjs` compiles the editor's pure modules
 * with `tsc` and runs them under node, so anything reaching `@/ipc/client` or `@/store` falls
 * out of that pipe. `revealPane.ts` is the half that acts, and it is four statements long
 * precisely because the deciding is here.
 *
 * # Why the input types are structural and local
 *
 * Nothing here imports `Bootstrap`. The check compiles this file with an explicit file list
 * and no `paths` mapping, so an `@/ipc/generated` import would not resolve under node. The
 * shapes below are therefore the narrowest subset of the wire types this reasoning needs, and
 * the real `Bootstrap` being assignable to them is checked where it matters — `revealPane.ts`
 * passes one in, and `tsc --noEmit` over the project fails if the wire shape ever moves.
 */

/** The parts of a `PaneTree` a reveal reasons about. `panes` is consulted for its keys only. */
export interface TreeLike {
  readonly focused: string
  readonly maximized: string | null
  readonly panes: Readonly<Record<string, unknown>>
}

export interface TabLike {
  readonly id: string
  readonly tree: TreeLike
}

export interface ProjectLike {
  readonly tabs: readonly TabLike[]
  readonly activeTab: string
  /** Panes torn out into windows of their own. Consulted for its keys only. */
  readonly detached: Readonly<Record<string, unknown>>
}

/** What a window is showing. Mirrors `WindowRole`, minus the fields not read here. */
export type RoleLike =
  | {
      readonly kind: 'shell'
      readonly projects: readonly string[]
      /**
       * The one project a shell **draws**. `projects` is what it holds.
       *
       * The distinction is the whole of `activateProject` below: in the default `stacked`
       * window mode every project docks into one shell and only the active one is rendered,
       * so a shell can hold a project it is showing nothing of.
       */
      readonly active: string | null
    }
  | { readonly kind: 'detachedPane'; readonly pane: string }
  | { readonly kind: 'detachedTab'; readonly tab: string }

export interface BootLike {
  readonly role: RoleLike
  readonly workspace: {
    readonly projects: Readonly<Record<string, ProjectLike>>
    /** Every window the workspace names. Read for the shells' `active`; keys are not used. */
    readonly windows: Readonly<Record<string, RoleLike>>
  }
}

/**
 * Everything a reveal would have to do, decided in one pass over the mirror.
 *
 * Flags rather than a list of steps: the caller runs them in a fixed order (domain first,
 * desktop last — see `revealPane.ts`), and a list would let a future caller run them in an
 * order that raises a window before the tab behind it has changed.
 */
export interface RevealPlan {
  /**
   * Whether the window asking is the window showing the pane.
   *
   * Derived from this window's own role rather than by looking the pane up in
   * `workspace.windows`, because the two can disagree and only one of them is about *this*
   * process: the map names labels, and a label is not a thing a webview can compare itself
   * against without also trusting that its own `boot.window` survived a restore.
   */
  readonly here: boolean
  /** The tab holding the pane; `null` for a detached pane, which is in no tab. */
  readonly tab: string | null
  /**
   * No shell that holds this project is currently *drawing* it.
   *
   * `stacked` is the default window mode and it docks every open project into one shell that
   * renders only `role.active` — so `tab_activate` on a project the shell is not showing
   * moves a tab nobody can see, and the raise that follows brings a window forward still
   * displaying a different project. That is the reported "nothing happened" exactly, which is
   * why this is a step of the plan and not an assumption.
   *
   * Answered from `workspace.windows` rather than from `boot.role`, because the window that
   * has to change is usually not this one: the case that reaches it is a detached editor
   * mentioning into its project's console while the shell has been switched elsewhere.
   */
  readonly activateProject: boolean
  /** The tab is not its project's active one. */
  readonly activateTab: boolean
  /** Another pane in that tab is maximized, so this one is laid out but not visible. */
  readonly clearMaximize: boolean
  /** The tab's tree does not already name this pane as focused. */
  readonly focusPane: boolean
  /** Why the pane cannot be brought into view at all, in words a user can read. */
  readonly blocked: string | null
}

function nothing(blocked: string): RevealPlan {
  return {
    here: false,
    tab: null,
    activateProject: false,
    activateTab: false,
    clearMaximize: false,
    focusPane: false,
    blocked,
  }
}

/**
 * Whether the window with this role is the one `project`'s tabs live in.
 *
 * *Holds*, not *draws* — a `stacked` shell holds several projects and draws one. That is why
 * `here` alone is not enough to say the user can see the pane, and why `activateProject`
 * exists beside it.
 */
function showsProject(role: RoleLike, project: string): boolean {
  return role.kind === 'shell' && role.projects.includes(project)
}

/**
 * Whether a shell holds `project` and none of them is drawing it.
 *
 * False when no shell holds it at all: there is nothing for `project_activate` to move, and
 * the raise that follows answers "no window is showing it", which the caller says out loud.
 * Every holding shell is asked rather than the first, because `perProject` mode gives each
 * project a shell whose `active` is already it — and a spurious activation there would drag a
 * *second* shell onto the project for a pane it is not showing.
 */
function needsProjectActivated(boot: BootLike, project: string): boolean {
  const shells = Object.values(boot.workspace.windows).filter((r) => showsProject(r, project))
  return shells.length > 0 && !shells.some((r) => r.kind === 'shell' && r.active === project)
}

/**
 * What it would take to put `pane` in front of the user of the window described by `boot`.
 *
 * The three shapes it can answer with:
 *
 * * **a pane in a tab** — bring its project to the front of the shell holding it, activate the
 *   tab, drop a maximize that hides it, focus it, and raise its window if that is not this one;
 * * **a detached pane** — nothing but the raise. A detached-pane window shows exactly one
 *   pane: there is no tab to activate, nothing that could be maximized over it, and its tree
 *   names it focused by construction. Doing the tab work anyway would mean calling
 *   `tab_activate` with a tab the pane is no longer in;
 * * **gone** — the pane is in no tab and no holding map. The send still landed (the mention
 *   went to a *session*, which outlives the pane showing it), so this is not a failure of the
 *   send and is deliberately worded as a fact rather than an error.
 */
export function planReveal(boot: BootLike, project: string, pane: string): RevealPlan {
  const open = boot.workspace.projects[project]
  if (open === undefined) return nothing('that project is no longer open')

  const tab = open.tabs.find((t) => Object.hasOwn(t.tree.panes, pane))
  if (tab !== undefined) {
    // A tab window is written out even though nothing creates one yet (`window_close` refuses
    // to re-dock a detached tab, because `cide-core` has no `redock_tab`). Folding it into the
    // `shell` case would answer `here: false` for the window the pane is actually in, and the
    // reveal would ask a compositor to raise the window it was already looking at.
    const shows =
      showsProject(boot.role, project) ||
      (boot.role.kind === 'detachedTab' && boot.role.tab === tab.id)
    return {
      here: shows,
      tab: tab.id,
      activateProject: needsProjectActivated(boot, project),
      activateTab: open.activeTab !== tab.id,
      // Cleared rather than moved: maximizing the *target* would be a bigger rearrangement
      // than the gesture asked for, and it would leave the user's own maximize undone with
      // no gesture that restores it. `maximized === pane` is already the answer they want.
      clearMaximize: tab.tree.maximized !== null && tab.tree.maximized !== pane,
      focusPane: tab.tree.focused !== pane,
      blocked: null,
    }
  }

  if (Object.hasOwn(open.detached, pane)) {
    return {
      here: boot.role.kind === 'detachedPane' && boot.role.pane === pane,
      tab: null,
      // A torn-out pane has its own window and is drawn there whatever the shell has in
      // front, so switching the shell's project would move a window nobody is being sent to.
      activateProject: false,
      activateTab: false,
      clearMaximize: false,
      focusPane: false,
      blocked: null,
    }
  }

  return nothing('that Claude pane has since closed')
}
