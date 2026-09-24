/**
 * The demo repository's working state: the coalescer change the Claude panes are talking about,
 * spread over two changelists, the way an IDEA user actually keeps unrelated edits apart.
 *
 * Built from the generated wire types directly (as `sidebar/GitPanel/fixture.ts` is, and for its
 * reason — a fixture in a panel's own invented shape renders beautifully against nothing real).
 */
import type {
  BranchInfo,
  BranchList,
  BranchRef,
  ChangeEntry,
  ChangesTree,
  FileState,
  RepoInfo,
} from '../../ipc/generated'
import { HOME, wire } from '../world'

export const REPO: RepoInfo = {
  id: '3f2a1b00-0000-4000-8000-00000000c1de',
  root: HOME,
  name: 'cide',
  parent: null,
  isSubmodule: false,
}

export const BRANCH: BranchInfo = {
  head: 'pty-backpressure',
  detached: false,
  upstream: 'origin/pty-backpressure',
  ahead: 2,
  behind: 0,
  operation: null,
  unborn: false,
}

function entry(path: string, index: FileState, worktree: FileState, changelist: string): ChangeEntry {
  return {
    path,
    origPath: null,
    index,
    worktree,
    staged: index !== 'unmodified',
    binary: false,
    submodule: false,
    changelist,
  }
}

export const STATUS: ChangesTree = {
  repos: [
    {
      repo: REPO,
      branch: BRANCH,
      changelists: [
        {
          id: 'default',
          name: 'Backpressure',
          comment: '',
          active: true,
          changes: [
            entry('crates/cide-pty/src/coalesce.rs', 'added', 'unmodified', 'default'),
            entry('crates/cide-pty/src/session.rs', 'unmodified', 'modified', 'default'),
            entry('crates/cide-pty/src/lib.rs', 'unmodified', 'modified', 'default'),
            entry('crates/cide-pty/tests/backpressure.rs', 'added', 'unmodified', 'default'),
          ],
        },
        {
          id: 'docs',
          name: 'Docs',
          comment: '',
          active: false,
          changes: [entry('README.md', 'unmodified', 'modified', 'docs'), entry('docs/architecture.md', 'unmodified', 'modified', 'docs')],
        },
      ],
      unversioned: [entry('notes/pty-bench.md', 'unmodified', 'untracked', 'default')],
      ignored: [],
      conflicts: [],
      indexChangedExternally: false,
      useStagingArea: false,
    },
  ],
}

const NOW = 1_790_000_000
const ref = (name: string, subject: string, remote = false): BranchRef => ({
  name,
  remote,
  current: !remote && name === BRANCH.head,
  upstream: remote ? null : `origin/${name}`,
  ahead: name === BRANCH.head ? 2 : 0,
  behind: 0,
  tip: 'a41c9e07d2b6f5e8c3a1b0d9e8f7a6b5c4d3e2f1',
  subject,
  committed: wire(NOW - 3600),
})

export const BRANCHES: BranchList[] = [
  {
    repo: REPO,
    head: BRANCH,
    local: [
      ref('master', 'Journal: small freezes, found by reading'),
      ref('pty-backpressure', 'Hold frames past the ack window'),
      ref('gitlab-review', 'Draft comments from an agent review'),
    ],
    remote: [ref('origin/master', 'Journal: small freezes, found by reading', true), ref('origin/pty-backpressure', 'Split the coalescer', true)],
  },
]
