/**
 * The `merge` scene's repository: `master` merged into `pty-backpressure`, stopped on
 * `session.rs`. The branch moved the coalescer out of the session while master shortened its
 * flush interval and started counting frames in the very lines that moved — four real
 * conflicts, and a handful of changes on either side that merge by themselves.
 */
import type { ChangeEntry, ChangesTree, ConflictFile, FileState, MergeState } from '../../ipc/generated'
import { BRANCH, REPO, STATUS } from './git'
import { SESSION_BASE, SESSION_OURS, SESSION_THEIRS } from './git-session'

export const CONFLICT_PATH = 'crates/cide-pty/src/session.rs'

export const CONFLICT: ConflictFile = {
  path: CONFLICT_PATH,
  base: SESSION_BASE,
  ours: SESSION_OURS,
  theirs: SESSION_THEIRS,
  ourLabel: 'pty-backpressure',
  theirLabel: 'master',
  binary: false,
  // What the wire really carries. `cide_ipc::git::ConflictFile::too_large` is `#[ts(optional)]`
  // with no `skip_serializing_if`, so Rust sends `null` while the generated type says the field
  // is absent — and `MergePane` tests `!== null`, so leaving it out (as the type invites) draws
  // every conflict as "too large to open".
  tooLarge: null as unknown as bigint,
}

/** Every path the merge stopped on; `lib.rs` already resolved, the lockfile still waiting. */
export const MERGE: MergeState = {
  operation: 'merge',
  ours: 'pty-backpressure',
  theirs: 'master',
  entries: [
    { path: CONFLICT_PATH, resolved: false, binary: false },
    { path: 'crates/cide-pty/src/lib.rs', resolved: true, binary: false },
    { path: 'Cargo.lock', resolved: false, binary: false },
  ],
}

function entry(path: string, index: FileState, worktree: FileState): ChangeEntry {
  return { path, origPath: null, index, worktree, staged: index !== 'unmodified', binary: false, submodule: false, changelist: 'default' }
}

/**
 * `git_status` mid-merge: what merged cleanly is staged in the active changelist, the
 * conflicted paths sit in their own group, and the branch reports the operation.
 */
export const MERGE_STATUS: ChangesTree = {
  repos: STATUS.repos.map((r) => ({
    ...r,
    branch: { ...BRANCH, operation: 'merge' },
    changelists: r.changelists.map((cl) =>
      cl.active
        ? {
            ...cl,
            changes: [
              entry('crates/cide-core/src/workspace.rs', 'modified', 'unmodified'),
              entry('crates/cide-app/src/cmd/session.rs', 'modified', 'unmodified'),
              entry('ui/src/store/workspace.ts', 'modified', 'unmodified'),
              entry('crates/cide-pty/src/lib.rs', 'modified', 'unmodified'),
            ],
          }
        : cl,
    ),
    unversioned: [],
    conflicts: [entry(CONFLICT_PATH, 'conflicted', 'conflicted'), entry('Cargo.lock', 'conflicted', 'conflicted')],
  })),
}

export { REPO }
