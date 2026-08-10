/**
 * Fixed panel states, reachable without arranging the repository to match.
 *
 * # Why this file exists
 *
 * Three of the states this panel has to get right are ones nobody sees while building it:
 * a second repository, a submodule with changes of its own, and the "staging changed
 * outside cide" bar. That bar in particular only appears when someone ran `git add` in a
 * bash pane between a refresh and a commit — which is to say, it is the bar nobody tests
 * until it matters, and the first time it renders should not be the first time anyone has
 * looked at it.
 *
 * # The shape is the wire's, not the panel's
 *
 * Every story here is a real `cide_ipc::git::ChangesTree` and goes through `normalizeStatus`
 * exactly like a payload from Rust. That is deliberate and it is the lesson of the bug this
 * file is part of the fix for: the fixtures used to be written in the panel's *own* invented
 * shape, so every story rendered beautifully while the panel was empty against every real
 * repository. A fixture that does not travel the same road as the data proves nothing.
 *
 * Reach one with `?git-story=<name>` on the window URL — `mock`, `guard`, `multi`, `empty`.
 * No git command runs in story mode, so a story is safe against any working tree.
 */
import type {
  BranchInfo,
  ChangeEntry,
  ChangelistView,
  ChangesTree,
  FileState,
  RepoChanges,
  ShelfRow,
} from './types'

export type GitStoryName = 'mock' | 'guard' | 'multi' | 'empty'

const CIDE_ID = '3f2a1b00-0000-4000-8000-000000000001'
const VENDOR_ID = '3f2a1b00-0000-4000-8000-000000000002'
const ZLIB_ID = '3f2a1b00-0000-4000-8000-000000000003'

const CIDE_ROOT = '/home/dev/work/cide'
const VENDOR_ROOT = '/home/dev/work/cide/vendor/hub-core'
const ZLIB_ROOT = '/home/dev/work/cide/vendor/hub-core/third_party/zlib'

/**
 * One changed path.
 *
 * `index`/`worktree` are the pair the tri-state is a function of, so the helper takes them
 * both rather than a single collapsed status — writing fixtures in terms of one status is how
 * the old ones ended up unable to express "half of this file is staged" at all.
 */
function entry(
  path: string,
  index: FileState,
  worktree: FileState,
  changelist: string,
  extra: Partial<ChangeEntry> = {},
): ChangeEntry {
  return {
    path,
    origPath: null,
    index,
    worktree,
    staged: index !== 'unmodified',
    binary: false,
    submodule: false,
    changelist,
    ...extra,
  }
}

function changelist(
  id: string,
  name: string,
  active: boolean,
  changes: ChangeEntry[],
): ChangelistView {
  return { id, name, comment: '', active, changes }
}

function branch(head: string, unborn = false): BranchInfo {
  return {
    head,
    detached: false,
    upstream: `origin/${head}`,
    ahead: 1,
    behind: 0,
    operation: null,
    unborn,
  }
}

/**
 * The design mock's repository, row for row: `Changes`, `fixes`, `tomaster`,
 * `Unversioned Files`, `Ignored Files`, and a footer that reads `2 modified`.
 *
 * `Changes` is the active changelist and holds exactly two modified files, one of them
 * partially staged — index *and* worktree both dirty — which is what puts a `–` on the group
 * row rather than a `✓`, so the leaf-level tri-state is visible in the default state and not
 * only under interaction.
 */
const CIDE_REPO: RepoChanges = {
  repo: {
    id: CIDE_ID,
    root: CIDE_ROOT,
    name: 'cide',
    parent: null,
    isSubmodule: false,
  },
  branch: branch('hub-provider-config'),
  changelists: [
    changelist('default', 'Changes', true, [
      entry('crates/cide-git/src/lib.rs', 'unmodified', 'modified', 'default'),
      entry('ui/src/sidebar/GitPanel/GitPanel.tsx', 'modified', 'modified', 'default'),
    ]),
    changelist('fixes', 'fixes', false, [
      entry('contract/events.json', 'unmodified', 'modified', 'fixes'),
      entry('crates/cide-app/src/emit.rs', 'unmodified', 'modified', 'fixes'),
      entry('crates/cide-pty/src/reader.rs', 'unmodified', 'modified', 'fixes'),
    ]),
    changelist('tomaster', 'tomaster', false, [
      entry('docs/adr/0004-changelists-not-index.md', 'added', 'unmodified', 'tomaster'),
      entry('docs/adr/0005-claude-hosting-hybrid.md', 'renamed', 'unmodified', 'tomaster', {
        origPath: 'docs/adr/0005-hosting.md',
      }),
    ]),
  ],
  unversioned: [
    entry('notes.md', 'unmodified', 'untracked', 'default'),
    entry('ui/src/sidebar/GitPanel/fixture.ts', 'unmodified', 'untracked', 'default'),
  ],
  ignored: [
    entry('target/debug/incremental', 'unmodified', 'ignored', 'default'),
    entry('ui/node_modules', 'unmodified', 'ignored', 'default'),
  ],
  conflicts: [],
  indexChangedExternally: false,
  useStagingArea: false,
}

/** A second root, whose own submodule has changes of its own. */
const VENDOR_REPO: RepoChanges = {
  repo: {
    id: VENDOR_ID,
    root: VENDOR_ROOT,
    name: 'hub-core',
    parent: null,
    isSubmodule: false,
  },
  branch: branch('main'),
  changelists: [
    changelist('default', 'Changes', true, [
      entry('src/protocol.rs', 'unmodified', 'modified', 'default'),
    ]),
  ],
  unversioned: [],
  ignored: [],
  conflicts: [],
  indexChangedExternally: true,
  useStagingArea: false,
}

/**
 * The submodule — a repository, arriving flat with `parent` set (§5.3).
 *
 * `normalizeStatus` re-nests it under `hub-core`. It is *not* a group inside the parent's
 * `Changes`: its files live in its own index and no commit of the parent can include them.
 */
const ZLIB_REPO: RepoChanges = {
  repo: {
    id: ZLIB_ID,
    root: ZLIB_ROOT,
    name: 'third_party/zlib',
    parent: VENDOR_ID,
    isSubmodule: true,
  },
  branch: branch('release'),
  changelists: [
    changelist('default', 'Changes', true, [
      entry('deflate.c', 'unmodified', 'modified', 'default'),
      entry('zconf.h', 'modified', 'modified', 'default'),
    ]),
  ],
  unversioned: [],
  ignored: [],
  conflicts: [],
  indexChangedExternally: false,
  useStagingArea: false,
}

/** One repo, so the repo level is elided and the panel is the mock exactly. */
export const MOCK_STATUS: ChangesTree = { repos: [CIDE_REPO] }

/**
 * The mock, plus an index that changed underneath us.
 *
 * The same data otherwise, so flipping between `mock` and `guard` shows exactly what the
 * bar costs in vertical space, and that the commit button stays live — the guard is a
 * warning, not a modal.
 */
export const GUARD_STATUS: ChangesTree = {
  repos: [{ ...CIDE_REPO, indexChangedExternally: true }],
}

/** Two roots and a submodule: the only story in which repo rows exist at all. */
export const MULTI_STATUS: ChangesTree = { repos: [CIDE_REPO, VENDOR_REPO, ZLIB_REPO] }

/** A clean tree. The empty state is a real state and has to say so in words. */
export const EMPTY_STATUS: ChangesTree = {
  repos: [
    {
      ...CIDE_REPO,
      changelists: [changelist('default', 'Changes', true, [])],
      unversioned: [],
      ignored: [],
    },
  ],
}

export const SHELF_FIXTURE: readonly ShelfRow[] = [
  {
    repo: CIDE_ID,
    key: `${CIDE_ID}/shelf-1`,
    entry: {
      id: 'shelf-1',
      name: 'wip: statusline parser',
      // `bigint` is what ts-rs spells `i64`; Tauri's JSON delivers a number. The fixture uses
      // the declared type so the story exercises the same `Number(…)` widening the app does.
      created: 1_754_700_000n,
      files: ['ui/src/store/statusFormat.ts', 'ui/src/chrome/StatusBar.tsx'],
    },
  },
  {
    repo: CIDE_ID,
    key: `${CIDE_ID}/shelf-2`,
    entry: {
      id: 'shelf-2',
      name: 'revert pty coalescing',
      created: 1_754_100_000n,
      files: ['crates/cide-pty/src/reader.rs'],
    },
  },
]

export interface GitStory {
  status: ChangesTree
  shelf: readonly ShelfRow[]
}

const STORIES: Record<GitStoryName, GitStory> = {
  mock: { status: MOCK_STATUS, shelf: SHELF_FIXTURE },
  guard: { status: GUARD_STATUS, shelf: SHELF_FIXTURE },
  multi: { status: MULTI_STATUS, shelf: SHELF_FIXTURE },
  empty: { status: EMPTY_STATUS, shelf: [] },
}

/**
 * The story named on the window URL, if any. `null` means talk to Rust.
 *
 * An unrecognised name falls back to `mock` rather than to live git: someone who typed
 * `?git-story=gaurd` asked for a fixture, and quietly running real git commands instead is
 * the one answer that could surprise them.
 */
export function storyFromQuery(search: string = location.search): GitStory | null {
  const name = new URLSearchParams(search).get('git-story')
  if (name === null) return null
  return STORIES[name as GitStoryName] ?? STORIES.mock
}
