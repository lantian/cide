/**
 * The git panel's types: the wire shapes, re-exported, and the view shapes derived from them.
 *
 * # There used to be hand-written DTOs here
 *
 * They were a *mirror* of what `cide-git` was expected to send, written while the Rust side
 * was still being built. The mirror and the original then disagreed — the mirror had a
 * top-level `root` and a recursive `groups` tree, `cide-ipc::git::RepoChanges` has
 * `repo.{id,root,name}`, `branch`, and four sibling lists — and because `normalizeStatus`
 * validated against the mirror, **every repository was dropped and the panel was empty against
 * any real repository**. Nothing failed; there was simply nothing to draw.
 *
 * So the wire types are now the generated ones and nothing else, per the house rule that wire
 * types live in `cide-ipc`. `cargo xtask codegen` regenerates them from the Rust, so a field
 * rename is a TypeScript error here rather than an empty panel.
 *
 * # What is still declared here, and why
 *
 * The *view* types below are not wire types and must not become any. `RepoChanges` is what one
 * repository is; `RepoView` is what the tree draws — four sibling lists folded into one ordered
 * list of collapsible groups, and submodules hung under the repository that contains them.
 * That fold is a display decision (which group comes first, which starts collapsed), it is
 * tested by `ui/scripts/check-git-tree.mjs`, and putting it in Rust would put a scroll-position
 * concern in a crate that has no window.
 */
export type {
  BranchInfo,
  ChangeEntry,
  ChangelistView,
  ChangesTree,
  DiffSide,
  FileDiff,
  FileState,
  RepoChanges,
  RepoId,
  RepoInfo,
  ShelfEntry,
} from '@/ipc/generated'

import type { BranchInfo, ChangeEntry, RepoId, ShelfEntry } from '@/ipc/generated'

/**
 * Why a group exists, which decides where it sits and whether it starts open.
 *
 * `conflicts`, `unversioned` and `ignored` are not changelists in git's sense — they are the
 * three sibling lists `RepoChanges` carries beside `changelists` — but IDEA draws them in the
 * same tree and so does the mock. The kind is what tells them apart: committing an unversioned
 * file means adding it, and conflicts cannot be committed at all.
 */
export type GroupKind = 'changelist' | 'conflicts' | 'unversioned' | 'ignored'

/** One collapsible group of files inside one repository. */
export interface GroupView {
  /** Unique within its repo, and used verbatim as the row id's suffix. */
  id: string
  /** `Changes`, `fixes`, `Merge Conflicts`, `Unversioned Files`, `Ignored Files`. */
  name: string
  kind: GroupKind
  /** IDEA's bold "active changelist" — new changes land here, and commit defaults to it. */
  active: boolean
  entries: readonly ChangeEntry[]
}

/**
 * One repository and everything uncommitted in it.
 *
 * # Submodules are repositories, not groups
 *
 * `ChangesTree.repos` is flat, with `RepoInfo.parent` naming the containing repository. The
 * panel re-nests it here so a submodule's changes are drawn *inside* the root that holds them,
 * which is the only arrangement in which a monorepo user can see what is dirty (§5.3).
 *
 * They are nested as repositories with their own changelists rather than as a group inside the
 * parent's `Changes`, which is what the pre-wire mirror did. A submodule's files are in the
 * submodule's index and cannot be part of the parent's commit; drawing them inside the parent's
 * changelist would put files in a group whose checkbox cannot commit them.
 */
export interface RepoView {
  /** What every `git_*` command takes. **Not** a path — see `cmd/git.rs::repo_root`. */
  id: RepoId
  /** Absolute work-tree path. Shown in a tooltip; never sent to Rust as an identifier. */
  root: string
  /** The last path component, or the submodule's name. */
  name: string
  branch: BranchInfo
  isSubmodule: boolean
  /** Someone ran `git add` in a pane. Raises the guard bar for this repo alone. */
  indexChangedExternally: boolean
  /** IDEA's "use Git staging area instead": the index is the truth and cide never rebuilds it. */
  useStagingArea: boolean
  /** Empty groups are dropped, so this is empty for a clean repository. */
  groups: GroupView[]
  /** Submodules of this repository, in the order Rust listed them. */
  children: RepoView[]
}

/** What the panel draws: `git_status`'s payload, folded into groups and re-nested. */
export interface StatusView {
  repos: RepoView[]
}

/**
 * One shelved patch, and the repository whose shelf it is on.
 *
 * `git_shelf_list` is per repo and the Shelf tab is not, so the tab's list is the concatenation
 * of every repo's — and `ShelfEntry.id` is only unique within one shelf, which is why `key`
 * exists. Unshelving needs the `RepoId` as much as the entry id.
 */
export interface ShelfRow {
  repo: RepoId
  /** Unique across repositories — what React keys the row by. */
  key: string
  entry: ShelfEntry
}

/**
 * What activating a file row means — the two operations a `RowAction` with `open` set can be.
 *
 * > *"Only when diff is already opened one click should change current diff to selected
 * > file."*
 *
 * `gitTreeClick` decides *whether* a gesture opens; this says *how*, and the two were the
 * same thing until a single click down a 30-file changelist produced 30 tabs. A double-click
 * (and Enter, and the toolbar's ◫) is `'open'`: a tab of its own, kept. A single click while
 * a diff is already up is `'retarget'`: the diff the user is reading follows the pointer, and
 * no tab is added.
 *
 * A string union rather than a boolean `retarget` flag, because `showDiff(repo, entry, true)`
 * at a call site says nothing about which way round the argument goes and the wrong answer is
 * silent — it either loses the tab the user was reading or gives them another one.
 */
export type DiffOpenMode = 'open' | 'retarget'

/**
 * One changelist as the chooser offers it.
 *
 * Deliberately not `ChangelistView`: that carries every `ChangeEntry` in the list, and the
 * chooser needs a name, an id and a count. Passing the wire type would put a few hundred
 * entries into dialog state that is re-rendered on every keystroke in the name field.
 *
 * `id` is the **raw** changelist id Rust knows (`default`, `fixes`), not the `cl:`-prefixed
 * group id `normalizeStatus` mints — see `changelistIdOf`. Sending a group id to
 * `git_changelist_move_paths` is a `NoSuchChangelist` on every row of the dialog.
 */
export interface ChangelistTarget {
  id: string
  name: string
  active: boolean
  count: number
}

/** Which of the three things the one chooser dialog is doing. */
export type ChangelistDialogMode = 'create' | 'rename' | 'move'

/**
 * The chooser, as `useGitPanel` holds it.
 *
 * One state and one component for create, rename and move, because the move dialog has to be
 * able to create a list inline — *"that inline-create is the part people actually use; a
 * dialog that only picks is half the feature"* — and once it can, a separate create dialog
 * would be the same field and the same duplicate-name rule written twice.
 */
export interface ChangelistDialogState {
  mode: ChangelistDialogMode
  /** Every changelist command is per repository; a list cannot span two. */
  repo: RepoId
  /** Shown for the workspace where more than one repository is open. */
  repoName: string
  /** What `move` picks from, and what a new name is checked against. */
  lists: readonly ChangelistTarget[]
  /** `rename`: the list being renamed. `move`: the list the paths are in now, if they share one. */
  id: string | null
  /** `rename`: the current name, so the field opens with it rather than empty. */
  name: string
  /** `move`: the repo-relative paths that will be moved. Named in the dialog, not counted. */
  paths: readonly string[]
  /**
   * `move`: these paths are unversioned, so picking a list **adds them to git** first.
   *
   * The context menu's half of `dragDrop.ts`'s `track` outcome. It rides on the dialog state
   * rather than being re-derived when the chooser is answered, because by then the tree has
   * very possibly refreshed — this panel repaints several times a second while an agent edits
   * — and a path that was untracked when the menu opened would be looked up again in a view
   * that no longer says so. The gesture decided what it was; the answer to the chooser must
   * not quietly become a different operation.
   *
   * It is also what the dialog reads to say, in words, that git is being written to. A
   * chooser that looked identical for the two operations would be the silent staging this
   * whole path exists to avoid, arrived at through the keyboard instead of the pointer.
   */
  track: boolean
}

/*
 * `ConfirmState` used to be declared here and is now in `chrome/ConfirmDestructive.tsx`, with
 * the dialog that renders it — the file tree raises the same dialog for *Move to Trash*, so
 * the component moved out of this folder and the state that is its only argument went with it.
 * A state and the one component that consumes it are a single contract; splitting them across
 * two features is how the two drift.
 *
 * Not re-exported from here, and that is not an oversight. This module is reached by
 * `check-git-tree.mjs`, which compiles `model.ts` with a bare `tsc` and **no `--jsx`**; naming
 * a `.tsx` module here — even in a type-only re-export, which emits nothing — pulls it into
 * the type graph and fails the compile. `useGitPanel.ts` imports it from its own home instead,
 * which is one import longer and keeps this module's dependency surface as narrow as the check
 * script needs it to be. See the note at the foot of this file for the wider rule.
 */

/*
 * Nothing in this module has a runtime value, deliberately.
 *
 * `ui/scripts/check-git-tree.mjs` compiles `model.ts` and imports the JavaScript under node.
 * A type-only module is erased, so `model.js` comes out with no imports at all and needs no
 * module resolution at run time. A single `const` here would emit `import { … } from './types'`
 * — an extensionless specifier node refuses — and the check would fail for a reason that has
 * nothing to do with the panel. Constants live in `model.ts`.
 */
