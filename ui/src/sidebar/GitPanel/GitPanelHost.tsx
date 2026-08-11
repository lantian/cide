/**
 * The git panel as the app mounts it: `GitPanelView`, plus the three things that need a
 * window.
 *
 * ```tsx
 * {view === 'git' && <GitPanel project={activeProjectId} />}
 * ```
 *
 * # Why this file exists at all
 *
 * `GitPanel.tsx` is rendered under node by `ui/scripts/check-git-render.mjs`, with `window`
 * and `location` stubbed and no DOM. That check is the only thing standing between this panel
 * and the failure it has already had once — compiling, mounting, and drawing nothing.
 *
 * Two of the things this panel now needs cannot be reached from inside that render.
 * `useContextMenu` reads the window's live keymap and `useIconTheme` reads the theme, both
 * from `@/store/workspace`, and that store imports `layout/paneHosts`, which calls
 * `document.createElement` at module scope and drags the xterm addons in behind it. Extending
 * the check's stubs until that import survives would mean faking a browser, and a check that
 * renders against a fake proves things about the fake.
 *
 * So the window-shaped half lives here, one import above the view, and the model is built here
 * too — `useGitPanel` must be called exactly once for the tree and the menu to act on the same
 * state, and the menu needs it at open time. The view gets a theme string and two menu
 * handles, and keeps an import graph a server render can follow.
 *
 * # The menu reads the DOM, not a hover state
 *
 * `items` is called at open time with the element under the pointer, so the row is found by
 * walking up to the nearest `[data-row-id]` and looking it up in `git.rows`. That is the
 * difference between a menu that acts on what you right-clicked and one that acts on whatever
 * was last hovered.
 */
import { useCallback } from 'react'
import type { FileDiff, ProjectId, RepoId } from '@/ipc/client'
import { useIconTheme } from '@/icons'
import { useContextMenu, type MenuEntry } from '@/menus'
import { copyText } from '../copyText'
import { GitPanelView } from './GitPanel'
import { useGitPanel } from './useGitPanel'
import type { Row } from './model'

export interface GitPanelProps {
  /** The active project, or `null` before one is open — then the panel is simply empty. */
  project: ProjectId | null
  /**
   * Where a double-clicked file's diff goes.
   *
   * Optional because the panel cannot open a pane by itself and must not pretend to. Without
   * it the diff opens as a workspace *tab* instead, which is the normal path — see the note on
   * `showDiff` in `useGitPanel`. Passing it takes the payload directly and opens no tab.
   */
  onOpenDiff?: ((diff: FileDiff, repo: RepoId) => void) | undefined
}

export function GitPanel({ project, onOpenDiff }: GitPanelProps) {
  const git = useGitPanel(project, { onOpenDiff })
  const iconTheme = useIconTheme()

  /** The `Row` a menu gesture landed on, or `null` for the empty space below the tree. */
  const rowAt = useCallback(
    (target: HTMLElement | null): Row | null => {
      const id = target?.closest<HTMLElement>('[data-row-id]')?.dataset['rowId']
      if (id === undefined) return null
      return git.rows.find((row) => row.id === id) ?? null
    },
    [git.rows],
  )

  const { onContextMenu, menu } = useContextMenu({
    label: 'Git changes',
    items: ({ target }) => {
      const row = rowAt(target)
      // The empty space under the last row opens nothing: an empty box at the pointer reads
      // as a broken surface rather than as a surface with nothing to offer.
      if (row === null) return []

      const entry = row.entry
      if (entry === undefined) {
        // A repository or changelist row. It has no file verbs at all, and the one thing it
        // does have — fold — is already what a click on it does. `[]` declines to open.
        return []
      }
      const path = entry.path
      const staged = entry.index !== 'unmodified'
      const unstaged = entry.worktree !== 'unmodified'
      const conflicted = entry.index === 'conflicted' || entry.worktree === 'conflicted'

      const entries: MenuEntry[] = [
        {
          id: 'stage',
          label: 'Stage',
          command: 'git.stageSelected',
          ...(conflicted
            ? { disabledReason: 'Resolve the conflict before staging this file' }
            : unstaged
              ? { run: () => git.stageFile(row.repo, path) }
              : { disabledReason: 'Nothing outside the index to stage' }),
        },
        {
          id: 'unstage',
          label: 'Unstage',
          ...(staged
            ? { run: () => git.unstageFile(row.repo, path) }
            : { disabledReason: 'Nothing staged for this file' }),
        },
        {
          id: 'rollback',
          label: 'Roll Back Changes',
          danger: true,
          /*
           * `git.rollback` destroys uncommitted work and Rust will not ask first. There is no
           * confirmation here either, and that is a decision rather than an omission: the
           * sidebar has no dialog primitive of its own, and the honest alternatives were a
           * `window.confirm` (which blocks the webview's event loop and is intercepted by the
           * runtime) or a half-built modal. What it gets instead is `danger` styling, a label
           * that says what happens, and the fact that it acts on one named file rather than on
           * a selection the user may have forgotten about.
           */
          ...(staged || unstaged
            ? { run: () => git.rollbackFile(row.repo, path) }
            : { disabledReason: 'This file has no changes to roll back' }),
        },
        { kind: 'separator' },
        { id: 'diff', label: 'Show Diff', run: () => git.openDiff(row) },
        { id: 'copyPath', label: 'Copy Path', run: () => void copyText(path) },
      ]
      return entries
    },
  })

  return (
    <GitPanelView
      project={project}
      git={git}
      iconTheme={iconTheme}
      treeMenu={{ onContextMenu, menu }}
    />
  )
}
