/**
 * Which of a project's tabs is showing a given git diff, and whether that tab is in front.
 *
 * # Why the pane works this out for itself
 *
 * `TabContent` keeps every tab of the active project mounted and hides all but one with
 * `visibility: hidden` — that is what makes switching tabs free — and it already hands
 * `renderTree` an `active` flag for the things that cannot read their own state off the DOM.
 * Threading that flag down to the diff pane is one prop, and the pane still honours it when a
 * host passes it.
 *
 * It is deliberately not the *only* way the pane can find out. A pane whose deferral is
 * switched on by a prop is a pane whose deferral is off by default, and a diff tab that
 * refetches while nobody is looking at it looks exactly like a diff tab that is working: the
 * cost is a `git_diff_file` per hidden tab per event, which is invisible until a workspace has
 * eight diff tabs open and an agent is writing. So the pane also works it out for itself, out
 * of the `Workspace` that `cide://workspace-changed` already delivers to every window —
 * activating a tab is a workspace mutation, so the answer arrives on the same event that
 * changes it.
 *
 * A plain function rather than a `store/workspace` selector, which follows that event already:
 * that module reaches xterm through `paneHosts`, and `check-diff-render.mjs` SSR-renders this
 * pane's view under node, where a terminal addon touching `self` at import time is a hard
 * failure. Type-only imports here, so this module carries no runtime graph at all.
 *
 * # Keyed on the file, not on a tab id
 *
 * The pane is handed a `DiffSpec`, never its own `TabId`. That is enough, because
 * `(repo, path)` *is* a diff tab's identity: `cmd::file::shows_git_diff` keys the
 * open-or-activate check on exactly that pair and on nothing else, so a project holds at most
 * one git diff tab per file and finding it by pair cannot be ambiguous. Adding a `TabId` to
 * `DiffSpec` to avoid the search would put a tab's identity inside the value that is persisted
 * in `workspace.json` and compared for equality — a worse trade for a `find` over a handful of
 * tabs.
 *
 * # Every uncertain answer is "on screen"
 *
 * No snapshot yet, a project the mirror does not hold, a tab that is not in the tree: all
 * answer `true`. Those are the states in which this module knows nothing, and the behaviour it
 * falls back to is the behaviour the pane had before any of this existed — refetch when the
 * file moves. A wrong `false` is a diff that silently stops following its file, which is the
 * one failure worth spending a redundant fetch to avoid.
 *
 * The one state it cannot answer for is a *second window*: `activeTab` is a fact about the
 * project, and a window that is showing something else has no separate entry to consult. That
 * is why `GitDiffPane` keeps its `visible` prop — a host outside the shell's tab stack must
 * say so — and why nothing here tries to guess a window role.
 */
import type { RepoId, TabId, TabKind } from '@/ipc/generated'

/**
 * The part of a `Project` this needs.
 *
 * Structural rather than `Project` itself so that the check script can hand it two fields
 * instead of ten; a real `Project` satisfies it without a cast.
 */
export interface DiffTabHost {
  tabs: ReadonlyArray<{ id: TabId; kind: TabKind }>
  activeTab: TabId
}

/** Whether a tab is the git diff of this exact file. Mirrors `cmd::file::shows_git_diff`. */
export function showsGitDiff(kind: TabKind, repo: RepoId, path: string): boolean {
  return (
    kind.kind === 'diff' &&
    kind.spec.origin.kind === 'git' &&
    kind.spec.origin.repo === repo &&
    kind.spec.origin.path === path
  )
}

/**
 * Whether the tab showing `repo`'s `path` is the one the project is drawing.
 *
 * Note the side it is keyed on is *not* compared: the pane switches sides in place, so the
 * open tab for a file is the same tab whichever side it happens to be showing — the same
 * reason `shows_git_diff` leaves the side out.
 *
 * `some` over every match rather than `find` on the first. `tab_open_diff` activates an
 * existing tab instead of opening a second, so there is at most one — but a `workspace.json`
 * is a file on disk, and if two ever did exist, `find` would answer for whichever came first
 * and could report the tab in front as hidden. `some` degrades the impossible case in the
 * direction the rest of this module degrades in.
 */
export function diffTabOnScreen(
  project: DiffTabHost | undefined,
  repo: RepoId,
  path: string,
): boolean {
  if (project === undefined) return true
  const mine = project.tabs.filter((tab) => showsGitDiff(tab.kind, repo, path))
  return mine.length === 0 || mine.some((tab) => tab.id === project.activeTab)
}
