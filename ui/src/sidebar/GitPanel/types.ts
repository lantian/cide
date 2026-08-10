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

/*
 * Nothing in this module has a runtime value, deliberately.
 *
 * `ui/scripts/check-git-tree.mjs` compiles `model.ts` and imports the JavaScript under node.
 * A type-only module is erased, so `model.js` comes out with no imports at all and needs no
 * module resolution at run time. A single `const` here would emit `import { … } from './types'`
 * — an extensionless specifier node refuses — and the check would fail for a reason that has
 * nothing to do with the panel. Constants live in `model.ts`.
 */
