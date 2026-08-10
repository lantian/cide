/**
 * The arithmetic behind "which projects need a file index".
 *
 * Pure and import-free on purpose: `ui/scripts/check-picker.mjs` compiles this file with
 * `tsc` and exercises it, which nothing that imports the IPC client or a zustand store can
 * be. And it is worth exercising — every way of getting this wrong is silent. Index a
 * project twice and a large repository is re-walked from scratch on a snapshot the user
 * caused by renaming a tab; index it never and the tree and Ctrl+P are empty for the whole
 * session with no error anywhere, which is exactly the bug this file exists to close.
 */

/**
 * Is this rejection Rust's `FsError::NoIndex`?
 *
 * It means "the walk for this project has not started yet, or the project is closed" — the
 * answer every `fs_*` and `picker_query` handler gives before `fs.index` has run. The
 * distinction is worth a predicate because the callers treat it as a *state*, not a failure:
 * the tree draws zero rows and waits for `cide://fs-status`, the picker keeps polling. Both
 * of them previously routed it through `pendingCommand`, which files any rejection under
 * "this command is not registered in this build" — so opening a project showed the user the
 * words *File tree unavailable — fs_tree_count is not registered in this build* for the
 * whole of the first walk, in a build where it was registered and about to answer.
 *
 * Matched on the tag rather than the message: `FsError` is `#[serde(tag = "kind")]`
 * precisely so that error handling does not have to read prose. See `cide-fs/src/error.rs`.
 */
export function isNoIndex(error: unknown): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    (error as { kind?: unknown }).kind === 'noIndex'
  )
}

/** A project as the plan cares about it: an id, and the roots it is open over. */
export interface IndexTarget {
  project: string
  roots: readonly string[]
}

export interface IndexPlan {
  /** Projects to call `fs.index` for, in workspace order. */
  index: string[]
  /** Projects to call `fs.close` for — they are no longer in the workspace. */
  close: string[]
  /**
   * What to remember for the next snapshot: project id to a digest of the roots that were
   * indexed. Returned rather than mutated in place so the whole function stays pure.
   */
  known: Map<string, string>
}

/**
 * The roots as one comparable value.
 *
 * `\0` as the separator because it is the one byte a POSIX path cannot contain, so two
 * different root lists can never collide into one key. Order is part of the identity: Rust
 * compares the root vector element-wise (`FsRegistry::claim`), so a reordered list is a
 * re-index there and pretending otherwise here would leave the two disagreeing.
 */
function digest(roots: readonly string[]): string {
  return roots.join('\u0000')
}

/**
 * Diff the open projects against what has already been indexed.
 *
 * A project is (re-)indexed when it is new, or when its roots have changed — a project can
 * gain a submodule root, and the entry Rust holds is keyed by the old list. It is *not*
 * re-indexed merely because the workspace changed, which is the difference between a walk
 * per open and a walk per keystroke.
 *
 * A project with no roots is skipped rather than indexed: `fs.index` answers `NoIndex` for
 * one, so asking would cost a round trip to be told nothing, and recording it would mean
 * never asking again once it gained a root. It is not *closed* either — nothing was ever
 * opened for it.
 */
export function planFileIndex(
  known: ReadonlyMap<string, string>,
  open: readonly IndexTarget[],
): IndexPlan {
  const next = new Map<string, string>()
  const index: string[] = []

  for (const target of open) {
    if (target.roots.length === 0) {
      // Still "present", so the close pass below must not reap whatever it had before.
      const previous = known.get(target.project)
      if (previous !== undefined) next.set(target.project, previous)
      continue
    }
    const key = digest(target.roots)
    next.set(target.project, key)
    if (known.get(target.project) !== key) index.push(target.project)
  }

  const close: string[] = []
  for (const project of known.keys()) {
    if (!next.has(project)) close.push(project)
  }

  return { index, close, known: next }
}
