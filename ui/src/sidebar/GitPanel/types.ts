/**
 * The shapes the git panel reads off the wire.
 *
 * # Why these live here and not in `ipc/generated.ts`
 *
 * They are a *mirror*, not the source of truth. The `cide-git` DTOs are being written in
 * parallel with this panel and are not in `cide-ipc` yet, so `cargo xtask codegen` has
 * nothing to emit for them. Declaring them here keeps the panel typed today without
 * hand-editing a generated file that the gate would immediately revert.
 *
 * When the Rust side lands, the fix is mechanical: delete the interfaces below and
 * re-export the generated ones from this module, so no component import has to move. What
 * must NOT happen is these definitions quietly becoming the contract — `normalizeStatus`
 * in `model.ts` is the only place that turns wire data into these types, and it treats
 * every field as absent-until-proven, so a shape mismatch costs a missing row rather than
 * a white window.
 *
 * Names and casing are chosen to match what `#[serde(rename_all = "camelCase")]` will
 * produce. Deliberately *not* accepted anywhere: snake_case keys. An enum with struct
 * variants that forgets `rename_all_fields = "camelCase"` leaks `original_path`, and that
 * is a Rust bug with a one-line fix; teaching the frontend to accept both spellings would
 * hide it and leave the two casings alive forever.
 */

/** How a file differs from HEAD. `renamed` carries `originalPath`. */
export type FileStatus =
  | 'modified'
  | 'added'
  | 'deleted'
  | 'renamed'
  | 'unversioned'
  | 'ignored'
  | 'conflicted'

/** One changed path inside one repo. */
export interface ChangeFile {
  /** Repo-relative, forward slashes, as git reports it. */
  path: string
  status: FileStatus
  /** Rename source, repo-relative. Absent unless `status === 'renamed'`. */
  originalPath?: string
  /**
   * True when only some of this file's hunks are in the commit.
   *
   * This is what makes the tree tri-state at the *leaf*, which a plain checked/unchecked
   * model cannot express: hunk staging is the whole point of the panel, and a file with 3
   * of 7 lines staged must not render as fully checked.
   */
  partial?: boolean
}

/**
 * A changelist, or a submodule's changes nested inside one.
 *
 * Submodules are a group with children rather than a single opaque row (§5.3): a submodule
 * bump that shows as one unexpandable line is exactly the entry people commit by accident.
 */
export interface ChangeGroup {
  /** Unique within its repo. Used verbatim as the row id suffix. */
  id: string
  /** `Changes`, `fixes`, `Unversioned Files`, or a submodule's path. */
  name: string
  kind: ChangeGroupKind
  files: ChangeFile[]
  groups?: ChangeGroup[]
  /** IDEA's bold "active changelist" — new changes land here, and commit defaults to it. */
  active?: boolean
}

/**
 * Why a group exists, which decides how it is drawn and whether it can be committed.
 *
 * `unversioned` and `ignored` are not changelists in git's sense; IDEA shows them in the
 * same tree and so does the mock, but committing them means adding files, so the panel has
 * to be able to tell them apart from `Changes`.
 */
export type ChangeGroupKind = 'changelist' | 'unversioned' | 'ignored' | 'submodule'

/** One repository root and everything uncommitted in it. */
export interface RepoChanges {
  /** Absolute path of the work tree root. The row id namespace. */
  root: string
  /** Last path segment of `root`, or whatever the domain calls this root. */
  label: string
  branch?: string
  groups: ChangeGroup[]
  /**
   * HEAD's commit message, for prefilling the box when `Amend` is ticked.
   *
   * Absent on an unborn branch, which is also the case where `Amend` must stay disabled.
   */
  headMessage?: string
  /**
   * The index no longer matches what cide last wrote to it — someone ran `git add` in a
   * bash pane. Raised per repo, because in a multi-repo workspace only one of them
   * diverged and the bar should say which.
   */
  indexDiverged?: boolean
  /**
   * Opaque handle for the index state this payload describes.
   *
   * Passed straight back to `git_commit` as `expectIndex`, so the commit either applies to
   * the index the user was looking at or is refused. The frontend never interprets it —
   * it is a token, not a hash the panel is entitled to compare.
   */
  indexToken?: string
}

/** The whole panel's data: `git_status`'s payload. */
export interface ChangesTree {
  repos: RepoChanges[]
}

/** One entry in the Shelf tab. */
export interface ShelfEntry {
  id: string
  name: string
  /** Unix seconds. Rendered relative; absent means the backend did not report one. */
  createdAt?: number
  fileCount: number
}
