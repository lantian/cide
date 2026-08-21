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
// The wording, not a fifth copy of it — see the note on the same import in `FileTree.tsx`.
import { NO_HOST } from '@/chrome/menuModel'
import { copyText } from '../copyText'
import { repoRoot } from './repoRoots'
import { GitPanelView } from './GitPanel'
import { useGitPanel } from './useGitPanel'
import { DEFAULT_CHANGELIST, changelistIdOf, groupOf, type Row } from './model'
import { grab, plural, trackMenuLabel } from './dragDrop'

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
  /**
   * *Show File History* on a file row — the tool window's per-file tab. (M18)
   *
   * Takes an **absolute** path, like every other caller of `git.history.file`, which is why the
   * menu item below joins the row's path to its repository root before calling this. Routed by
   * the host through `runCommand('git.history.file', …)` — the `file.reveal` precedent: one
   * handler, one copy of the preconditions, four surfaces.
   *
   * Optional for the reason `onOpenDiff` is: a window with no tool window must not have this
   * panel pretend otherwise, and the item is drawn disabled with the reason on it.
   */
  onShowHistory?: ((path: string) => void) | undefined
  /**
   * *Update project* — the toolbar button that was disabled with *"needs a pull command"* from
   * the day it was written until M20. `git_pull` had existed since M10; nothing ever passed it.
   *
   * Routed by the host through `runCommand('git.pull', …)`, the same way `onShowHistory` is,
   * and for the same reason: one handler, one copy of the preconditions, several surfaces.
   */
  onUpdate?: (() => void) | undefined
}

export function GitPanel({ project, onOpenDiff, onShowHistory, onUpdate }: GitPanelProps) {
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
      if (entry === undefined) return row.kind === 'dir' ? dirMenu(row, git) : groupMenu(row, git)

      const path = entry.path
      /*
       * Which files this gesture is about — the row alone, or the whole row selection in the
       * same repo when the row is part of it. The count goes in the label; see
       * `dragDrop.ts::grab`.
       *
       * Through `grab`, which is also what a *drag* from this row picks up. The two routes to
       * "move these files" are then the same set by construction rather than by two
       * implementations that agree today: a menu that moved four files where a drag moved one
       * would be the panel disagreeing with itself about what the user is pointing at. That
       * is also why `git.carried` is threaded from the model rather than derived here — one
       * resolution of the selection, read by both routes.
       */
      const carried = grab(git.view, git.carried, row)
      const scope = { repo: row.repo, paths: carried?.files.map((f) => f.path) ?? [path] }
      const staged = entry.index !== 'unmodified'
      const unstaged = entry.worktree !== 'unmodified'
      const conflicted = entry.index === 'conflicted' || entry.worktree === 'conflicted'
      /*
       * From the *group* the row is drawn in, not from `entry.worktree`.
       *
       * They agree today, and one of them is the question being asked: "is this row in the
       * `Unversioned Files` list", which is what decides whether filing it has to add it to
       * git first. `grab` narrows a widened selection to one group kind, so `scope.paths` is
       * all-untracked or none-untracked and this one flag answers for the whole set.
       */
      const untracked = row.groupKind === 'unversioned'
      /** This repository's work tree, for the one item below that needs an absolute path. */
      const root = repoRoot(row.repo)

      /*
       * The resolution verbs, first, and only on a conflicted row. (M20)
       *
       * First because a conflicted row has exactly one useful next action and every item below
       * it is refused — *Resolve the conflict before staging this file*, *Resolve the conflict
       * first — conflicts are listed apart from changelists*. A menu whose first four entries
       * are all disabled is a menu that reads as broken.
       *
       * **The two side labels come from the file, not from the words "yours" and "theirs".**
       * During a rebase git swaps stage 2 and stage 3 — `HEAD` is the branch being rebased
       * *onto* and your commit is the incoming side — so a menu that said *Accept Yours* over
       * stage 2 would hand the user the other branch's work under their own name.
       * `cide_git::conflict::side_labels` decides the wording; this only draws it.
       */
      const merge = git.merges[row.repo]
      const conflictEntry = merge?.entries.find((e) => e.path === path)
      const conflictItems: MenuEntry[] =
        conflictEntry === undefined
          ? []
          : conflictEntry.resolved
            ? [
                {
                  id: 'unresolve',
                  label: 'Un-resolve',
                  run: () => git.unresolveFile(row.repo, path),
                },
              ]
            : [
                {
                  id: 'merge-open',
                  label: 'Resolve…',
                  run: () => git.openMerge(row.repo, path),
                },
                {
                  id: 'take-ours',
                  label: `Accept ${merge?.ours ?? 'Yours'}`,
                  run: () => git.resolveWith(row.repo, path, 'ours'),
                },
                {
                  id: 'take-theirs',
                  label: `Accept ${merge?.theirs ?? 'Theirs'}`,
                  run: () => git.resolveWith(row.repo, path, 'theirs'),
                },
                { kind: 'separator' },
              ]

      const entries: MenuEntry[] = [
        ...conflictItems,
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
           * `git_rollback` destroys uncommitted work and Rust will not ask first, so this
           * opens `ConfirmDestructive` rather than running — `rollbackFile` is
           * `revertFiles([path])` and the unguarded call is private to `useGitPanel`. This
           * comment used to say there was no confirmation and defend it; the dialog arrived
           * with the group menu and the file verb was moved behind it at the same time.
           */
          ...(staged || unstaged
            ? { run: () => git.rollbackFile(row.repo, path) }
            : { disabledReason: 'This file has no changes to roll back' }),
        },
        { kind: 'separator' },
        {
          id: 'move',
          /*
           * The count is the safeguard. `actOn` widens to the ticked files when the clicked
           * row is one of them — IDEA's behaviour, and the only multi-row gesture this panel
           * has — and *Move 4 Files to Changelist…* is a different sentence from *Move to
           * Changelist…*. The dialog then names every path before anything happens, so there
           * is no step at which the set being moved is invisible.
           *
           * An unversioned row gets the third sentence, from `dragDrop.ts` so that the menu
           * and the drag ghost make the same claim: the files are **added to git** on the way
           * into the list. It used to be disabled here with *"Only tracked changes belong to a
           * changelist"*, which is the state of the world and not an answer to what was asked
           * — the user wants that state changed, and IDEA changes it. See
           * `useGitPanel::trackPaths` for what it does to the repository.
           */
          label: untracked
            ? trackMenuLabel(scope.paths.length)
            : scope.paths.length > 1
              ? `Move ${scope.paths.length} Files to Changelist…`
              : 'Move to Changelist…',
          ...(row.groupKind === 'changelist' || untracked
            ? { run: () => git.moveToChangelist(scope.repo, scope.paths, untracked) }
            : {
                // Filing an ignored or conflicted path is a write nothing can see:
                // `cide_git::status` builds its `live` set from paths that are neither
                // `Untracked` nor `Ignored`, so `reconcile` drops the assignment on the next
                // status walk, and a conflicted path is drawn in the conflicts list whatever
                // it is filed under. Unversioned paths used to be refused here for the same
                // reason and now have a verb that removes it — an ignored one does not, and an
                // ignored file dragged into `Changes` would be a very surprising way to
                // un-ignore something.
                disabledReason:
                  row.groupKind === 'conflicts'
                    ? 'Resolve the conflict first — conflicts are listed apart from changelists'
                    : 'Only tracked changes belong to a changelist',
              }),
        },
        { kind: 'separator' },
        { id: 'diff', label: 'Show Diff', run: () => git.openDiff(row) },
        {
          /*
           * *Show File History*, beside *Show Diff* because they are the two read-only
           * questions about this row: what changed here now, and what changed here before.
           *
           * **The path has to be joined here**, and this is the only one of the four surfaces
           * that offering this item where that is true. This panel's rows carry a
           * *repo-relative* path plus a `RepoId`, because that is how git spells a path and how
           * every command in `cide_ipc::git` takes one. Every other surface — the tab strip, the
           * file tree, the editor — hands over an absolute path, `git.history.file` resolves it
           * through `git_locate`, and the tool window keys its tabs by the absolute path so that
           * the four routes land on **one** tab rather than four. Sending `src/main.rs` from
           * here would either resolve to nothing or, worse, open a second tab for a file that
           * already has one.
           *
           * `repoRoot` comes from the registry `GitDiffPane` already uses: every `ChangesTree`
           * this window sees carries `RepoInfo.root`, so the answer is noted in passing and
           * costs no round trip. It is `null` only before any tree has arrived — which cannot
           * happen while this menu is open, since the rows it is drawn over came from a tree —
           * but the type says it can, and inventing a root to satisfy it would be a guess about
           * where a repository is.
           */
          id: 'history',
          label: 'Show File History',
          command: 'git.history.file',
          ...(root !== null && onShowHistory !== undefined
            ? { run: () => onShowHistory(`${root.replace(/\/+$/, '')}/${path}`) }
            : {
                disabledReason:
                  root === null
                    ? "This repository's root has not been read yet — refresh the panel"
                    : NO_HOST,
              }),
        },
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
      {...(onUpdate !== undefined ? { onUpdate } : {})}
    />
  )
}

/**
 * The menu for a directory row — the keyboard's half of dragging a folder.
 *
 * Drag and drop is unusable without a pointer, so every gesture it offers has to exist here
 * too: this is where Shift+F10 on a focused directory row lands. `useContextMenu` treats a
 * `contextmenu` event at 0,0 as a keyboard invocation and anchors it to the focused element,
 * and the tree's rows are focusable, so the route is already wired — what was missing was a
 * menu for these rows at all. Without this branch a directory would open the *changelist*
 * menu, offering to rename and delete the list it happens to sit in.
 *
 * `grab` supplies the files, so the menu moves exactly what a drag from the same row would.
 */
function dirMenu(row: Row, git: ReturnType<typeof useGitPanel>): MenuEntry[] {
  const carried = grab(git.view, git.carried, row)
  const paths = carried?.files.map((f) => f.path) ?? []
  const count = paths.length
  const path = row.path ?? row.label
  // What `Roll Back` would actually do to these files. An untracked path has nothing in HEAD,
  // so `stage::rollback` deletes it; an ignored one is not part of any change at all. It is
  // also what decides whether filing this directory into a changelist has to add its files to
  // git on the way — see the `move` item below.
  const untracked = row.groupKind === 'unversioned'
  const ignored = row.groupKind === 'ignored'
  return [
    {
      id: 'move',
      // The count is the safeguard, exactly as on a file row: *Move 4 Files to Changelist…*
      // is a sentence the user reads before clicking, and the dialog then names every path.
      // A directory of unversioned files says the other sentence, the one that admits the
      // `git add` — same string as the drag ghost and the file row's menu.
      label: untracked ? trackMenuLabel(count) : `Move ${plural(count)} to Changelist…`,
      ...((row.groupKind === 'changelist' || untracked) && count > 0
        ? { run: () => git.moveToChangelist(row.repo, paths, untracked) }
        : {
            disabledReason:
              row.groupKind === 'conflicts'
                ? 'Resolve the conflict first — conflicts are listed apart from changelists'
                : count === 0
                  ? 'Nothing under this directory to file'
                  : 'Only tracked changes belong to a changelist',
          }),
    },
    { kind: 'separator' },
    {
      id: 'revertDir',
      /*
       * The label is per kind, exactly as on the group row, because the command is not the
       * same command: `stage::rollback` restores a tracked path from HEAD and *deletes* an
       * untracked one, and this row can name two hundred of them at once. `Roll Back 214
       * files` over a directory of unversioned work is a sentence that promises a restore and
       * performs a delete, which is the one thing in this panel with no undo at all.
       *
       * The ignored list is refused outright, which is the stance the group menu already
       * takes: those files are not part of any change, and a directory row inside it is
       * `target/` or `node_modules/` — nothing a context menu should be able to erase in one
       * click. The group row's own `Revert Group` has been disabled there since it shipped;
       * a directory row inside the same list must not be the way around it.
       */
      label: untracked ? `Delete ${plural(count)}` : `Roll Back ${plural(count)}`,
      danger: true,
      ...(ignored
        ? { disabledReason: 'Ignored files are not part of any change — nothing to roll back' }
        : count > 0
          ? { run: () => git.revertFiles(row.repo, paths, untracked) }
          : { disabledReason: 'Nothing under this directory to roll back' }),
    },
    { id: 'copyPath', label: 'Copy Path', run: () => void copyText(path) },
  ]
}

/**
 * The menu for a repository row or a group row — the one the user asked for.
 *
 * > *"git changes tree - need also an context menu for group, i should be able to revert the
 * > group."*
 *
 * These rows used to return `[]`, which declined to open at all: every changelist operation the
 * backend has had since M8 (`git_changelist_create` / `_rename` / `_delete` / `_move_paths` /
 * `_set_active`, `git_shelve`) was wired end to end and reachable from nothing.
 *
 * Every branch here ends in at least one *enabled* item, `New Changelist…`, so no right-click
 * on a group opens a box of grey lines. That matters most on the ignored and conflicts groups,
 * where nothing else applies — an inert menu reads as a broken surface.
 *
 * Taken as a parameter rather than closed over, and pure apart from the calls it schedules, so
 * that the shape of the menu is visible in one screen instead of nested three deep inside
 * `useContextMenu`.
 */
function groupMenu(
  row: Row,
  git: ReturnType<typeof useGitPanel>,
): MenuEntry[] {
  const repo = row.repo
  const newList: MenuEntry = {
    id: 'newChangelist',
    label: 'New Changelist…',
    run: () => git.newChangelist(repo),
  }

  // A repository row. Its groups are what the rest of this menu is about, and it has none of
  // their verbs — but it is the only row that says *which* repository, which is exactly what a
  // new changelist in a monorepo needs.
  if (row.kind === 'repo') return [newList]

  const id = changelistIdOf(row.group)
  const count = row.files.length
  const empty = count === 0

  if (id === null) {
    // Conflicts, unversioned, ignored: the three sibling lists. They are not changelists, so
    // they cannot be renamed, deleted or made active, and only the unversioned one has an
    // operation at all — one that deletes files rather than restoring them, which is why it is
    // worded as a delete. See `useGitPanel::revertGroup`.
    const unversioned = row.groupKind === 'unversioned'
    return [
      newList,
      {
        id: 'move',
        /*
         * The whole unversioned list, added to git and filed — the keyboard's answer to
         * dragging a group header, which the pointer deliberately cannot do (a press on one
         * folds it; see `dragDrop.ts`). Same string as the file row's item and the drag ghost,
         * so all three describe the `git add` the same way.
         *
         * Only on the unversioned group. The conflicts and ignored lists have no such verb —
         * `dropOutcome` refuses both, and this menu must not become the way around a rule the
         * drag enforces.
         */
        label: unversioned ? trackMenuLabel(count) : 'Move to Changelist…',
        ...(unversioned && !empty
          ? {
              run: () => {
                const found = groupOf(git.view, repo, row.group ?? '')
                if (found !== undefined) {
                  git.moveToChangelist(repo, [...found.entries.map((e) => e.path)], true)
                }
              },
            }
          : {
              disabledReason: unversioned
                ? 'Nothing in this group'
                : row.groupKind === 'conflicts'
                  ? 'Resolve the conflict first — conflicts are listed apart from changelists'
                  : 'Only tracked changes belong to a changelist',
            }),
      },
      { kind: 'separator' },
      {
        id: 'revertGroup',
        // The label is per kind, not one sentence with a count in it. It read
        // `Delete N Unversioned Files` on every one of the three, so a right-click on the
        // *Ignored* group offered to delete files it does not contain — disabled, but a
        // disabled item still tells the user what the app thinks it is pointing at.
        label: unversioned
          ? `Delete ${count} Unversioned File${count === 1 ? '' : 's'}`
          : 'Revert Group',
        danger: true,
        ...(unversioned && !empty
          ? { run: () => git.revertGroup(repo, row.group ?? '') }
          : {
              disabledReason: unversioned
                ? 'Nothing in this group'
                : 'This is not a changelist — nothing here to revert as a group',
            }),
      },
    ]
  }

  return [
    {
      id: 'setActive',
      label: 'Set Active Changelist',
      ...(row.active === true
        ? { disabledReason: 'New changes already land in this changelist' }
        : { run: () => git.setActiveChangelist(repo, id) }),
    },
    newList,
    { id: 'rename', label: 'Rename Changelist…', run: () => git.renameChangelist(repo, id) },
    {
      id: 'delete',
      label: 'Delete Changelist',
      // Not `danger`: `Sidecar::delete` moves the list's paths into the default one, so no
      // work is lost and there is nothing to confirm. Painting it red would spend the colour
      // that means "cannot be undone" on something that can.
      ...(id === DEFAULT_CHANGELIST
        ? { disabledReason: 'The default changelist cannot be deleted' }
        : { run: () => git.deleteChangelist(repo, id) }),
    },
    { kind: 'separator' },
    {
      id: 'move',
      /*
       * A whole changelist, moved. This is the keyboard's answer to dragging a changelist
       * header, which the pointer deliberately cannot do — a press on a group row folds it
       * (`gitTreeClick`), so a drag from one would begin by collapsing the thing being dragged.
       * See the header of `dragDrop.ts`.
       */
      label: `Move ${plural(count)} to Changelist…`,
      ...(empty
        ? { disabledReason: 'Nothing in this changelist to move' }
        : {
            run: () => {
              const found = groupOf(git.view, repo, row.group ?? '')
              if (found !== undefined) {
                git.moveToChangelist(repo, [...found.entries.map((e) => e.path)], false)
              }
            },
          }),
    },
    {
      id: 'shelve',
      label: 'Shelve Changelist',
      ...(empty
        ? { disabledReason: 'Nothing in this changelist to shelve' }
        : { run: () => git.shelveGroup(repo, row.group ?? '') }),
    },
    {
      id: 'revertGroup',
      label: `Revert Changelist`,
      danger: true,
      ...(empty
        ? { disabledReason: 'Nothing in this changelist to revert' }
        : { run: () => git.revertGroup(repo, row.group ?? '') }),
    },
  ]
}
