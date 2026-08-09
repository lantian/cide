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
 * Every story is plain data fed to the same `GitPanel` the app renders, through the same
 * props. Nothing here is a stand-in for the component: if the panel breaks, the story
 * breaks with it.
 *
 * Reach one with `?git-story=<name>` on the window URL — `mock`, `guard`, `multi`, `empty`.
 * No git command runs in story mode, so a story is safe against any working tree.
 */
import type { ChangesTree, RepoChanges, ShelfEntry } from './types'

export type GitStoryName = 'mock' | 'guard' | 'multi' | 'empty'

const CIDE_ROOT = '/home/dev/work/cide'
const VENDOR_ROOT = '/home/dev/work/cide/vendor/hub-core'

/**
 * The design mock's repository, row for row: `Changes`, `fixes`, `Ignore`, `tomaster`,
 * `Unversioned Files`, and a footer that reads `2 modified`.
 *
 * `Changes` is the active changelist and holds exactly two modified files, one of them
 * partially staged — which is what puts a `–` on the group row rather than a `✓`, so the
 * leaf-level tri-state is visible in the default state and not only under interaction.
 */
const CIDE_REPO: RepoChanges = {
  root: CIDE_ROOT,
  label: 'cide',
  branch: 'hub-provider-config',
  headMessage: 'Wire the IDE MCP server into the app',
  groups: [
    {
      id: 'default',
      name: 'Changes',
      kind: 'changelist',
      active: true,
      files: [
        { path: 'crates/cide-git/src/lib.rs', status: 'modified' },
        { path: 'ui/src/sidebar/GitPanel/GitPanel.tsx', status: 'modified', partial: true },
      ],
    },
    {
      id: 'fixes',
      name: 'fixes',
      kind: 'changelist',
      files: [
        { path: 'crates/cide-app/src/emit.rs', status: 'modified' },
        { path: 'crates/cide-pty/src/reader.rs', status: 'modified' },
        { path: 'contract/events.json', status: 'modified' },
      ],
    },
    {
      id: 'ignore',
      name: 'Ignore',
      kind: 'ignored',
      files: [
        { path: 'target/debug/incremental', status: 'ignored' },
        { path: 'ui/node_modules', status: 'ignored' },
      ],
    },
    {
      id: 'tomaster',
      name: 'tomaster',
      kind: 'changelist',
      files: [
        { path: 'docs/adr/0004-changelists-not-index.md', status: 'added' },
        {
          path: 'docs/adr/0005-claude-hosting-hybrid.md',
          status: 'renamed',
          originalPath: 'docs/adr/0005-hosting.md',
        },
      ],
    },
    {
      id: 'unversioned',
      name: 'Unversioned Files',
      kind: 'unversioned',
      files: [
        { path: 'ui/src/sidebar/GitPanel/fixture.ts', status: 'unversioned' },
        { path: 'notes.md', status: 'unversioned' },
      ],
    },
  ],
}

/**
 * A second root whose active changelist contains a submodule with its own changes.
 *
 * The submodule is a nested change group, not one opaque row (§5.3) — a submodule pointer
 * that reads as a single line is exactly the thing that gets committed without being read.
 */
const VENDOR_REPO: RepoChanges = {
  root: VENDOR_ROOT,
  label: 'hub-core',
  branch: 'main',
  headMessage: 'Bump protocol to 3',
  indexDiverged: true,
  groups: [
    {
      id: 'default',
      name: 'Changes',
      kind: 'changelist',
      active: true,
      files: [{ path: 'src/protocol.rs', status: 'modified' }],
      groups: [
        {
          id: 'sub:third_party/zlib',
          name: 'third_party/zlib',
          kind: 'submodule',
          files: [
            { path: 'third_party/zlib/deflate.c', status: 'modified' },
            { path: 'third_party/zlib/zconf.h', status: 'modified', partial: true },
          ],
        },
      ],
    },
  ],
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
export const GUARD_STATUS: ChangesTree = { repos: [{ ...CIDE_REPO, indexDiverged: true }] }

/** Two roots: the only story in which repo rows exist at all. */
export const MULTI_STATUS: ChangesTree = { repos: [CIDE_REPO, VENDOR_REPO] }

/** A clean tree. The empty state is a real state and has to say so in words. */
export const EMPTY_STATUS: ChangesTree = { repos: [{ root: CIDE_ROOT, label: 'cide', groups: [] }] }

export const SHELF_FIXTURE: readonly ShelfEntry[] = [
  { id: 'shelf-1', name: 'wip: statusline parser', createdAt: 1_754_700_000, fileCount: 4 },
  { id: 'shelf-2', name: 'revert pty coalescing', createdAt: 1_754_100_000, fileCount: 2 },
]

export interface GitStory {
  status: ChangesTree
  shelf: readonly ShelfEntry[]
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
