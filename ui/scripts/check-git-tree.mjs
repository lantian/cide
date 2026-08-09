/**
 * Checks `src/sidebar/GitPanel/model.ts` — the git panel's pure core.
 *
 * Same shape as `check-status-format.mjs`, and for the same reason: this project has no JS
 * test runner, and adding one for a handful of pure functions would be a larger commitment
 * than the code it tests. What is pinned here is the behaviour that is invisible in a
 * screenshot and expensive to get wrong:
 *
 *   * the repo level is elided with one repo and present with two (§5.3),
 *   * a submodule is a nested group whose files roll up into its parent's count,
 *   * tri-state distinguishes "none of these" from "some of the hunks of one of them",
 *   * a collapsed group still commits its ticked files — the one bug in this panel that
 *     would silently produce a wrong commit,
 *   * `normalizeStatus` never throws, whatever the backend sends.
 *
 * Run: `pnpm --dir ui run check:git`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-git-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/GitPanel/model.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'model.js')}`)

  let failed = 0
  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }

  // Two changelists, one of them holding a submodule group, plus unversioned files.
  const repo = (root, label) => ({
    root,
    label,
    headMessage: 'previous commit',
    groups: [
      {
        id: 'default',
        name: 'Changes',
        kind: 'changelist',
        active: true,
        files: [
          { path: 'a.rs', status: 'modified' },
          { path: 'src/b.rs', status: 'modified', partial: true },
        ],
        groups: [
          {
            id: 'sub',
            name: 'vendor/zlib',
            kind: 'submodule',
            files: [{ path: 'vendor/zlib/z.c', status: 'modified' }],
          },
        ],
      },
      {
        id: 'fixes',
        name: 'fixes',
        kind: 'changelist',
        files: [{ path: 'c.rs', status: 'added' }],
      },
      {
        id: 'unversioned',
        name: 'Unversioned Files',
        kind: 'unversioned',
        files: [{ path: 'notes.md', status: 'unversioned' }],
      },
    ],
  })

  const one = { repos: [repo('/w/app', 'app')] }
  const two = { repos: [repo('/w/app', 'app'), repo('/w/lib', 'lib')] }

  // --- shape ---------------------------------------------------------------------------

  const openAll = (tree) => m.allGroups(tree)
  const rows1 = m.buildRows(one, openAll(one))
  eq(
    rows1.filter((r) => r.kind === 'repo').length,
    0,
    'one repo: the repo level is elided, exactly as the mock has it',
  )
  eq(rows1[0].label, 'Changes', 'the first row is the first changelist')
  eq(rows1[0].depth, 0, 'with one repo, changelists sit flush at depth 0')
  eq(
    rows1.find((r) => r.label === 'vendor/zlib').depth,
    1,
    'a submodule is a group one level inside its changelist',
  )
  eq(
    rows1.find((r) => r.label === 'vendor/zlib/z.c').depth,
    2,
    'and its files are one level inside that — not a single opaque row',
  )

  const rows2 = m.buildRows(two, openAll(two))
  eq(rows2[0].kind, 'repo', 'two repos: a repo row appears above the changelists')
  eq(rows2[0].depth, 0, 'the repo row is the new depth 0')
  eq(rows2[1].depth, 1, 'and every changelist moves one level in')

  // --- counts --------------------------------------------------------------------------

  eq(
    rows1.find((r) => r.label === 'Changes').count,
    3,
    "a changelist's count includes the files inside its submodule",
  )
  eq(rows2[0].count, 5, "a repo's count is every file under it")

  // --- tri-state -----------------------------------------------------------------------

  const partial = m.partialFiles(one)
  const changes = rows1.find((r) => r.label === 'Changes')
  const none = new Set()
  eq(m.checkState(changes, none, (id) => partial.has(id)), 'unchecked', 'nothing ticked')

  const all = m.toggleRow(changes, none)
  eq(all.size, 3, 'ticking a changelist reaches every file below it, submodule included')
  eq(
    m.checkState(changes, all, (id) => partial.has(id)),
    'partial',
    'one half-staged file makes the whole group partial, not checked — the group must not '
      + 'claim to contain more than the commit will',
  )

  const clean = new Set([...all].filter((id) => !partial.has(id)))
  eq(
    m.checkState(changes, clean, (id) => partial.has(id)),
    'partial',
    'some files ticked is partial too',
  )
  eq(m.toggleRow(changes, all).size, 0, 'a second click clears rather than completing')

  const fixes = rows1.find((r) => r.label === 'fixes')
  eq(
    m.checkState(fixes, m.toggleRow(fixes, none), () => false),
    'checked',
    'a group with every file ticked and no partiality is checked',
  )

  // --- defaults ------------------------------------------------------------------------

  const defaults = m.defaultSelection(one)
  eq(defaults.size, 3, 'only the active changelist is ticked on open — `fixes` is not')
  eq(
    [...defaults].every((id) => !id.includes('notes.md')),
    true,
    'unversioned files are never ticked by default',
  )
  eq(
    m.summarize(m.selectedFiles(one, defaults)),
    '3 modified',
    "the footer's summary counts by status and does not pluralise the adjective",
  )
  eq(m.summarize([]), 'nothing selected', 'an empty selection says so in words')
  eq(
    m.summarize([
      { path: 'a', status: 'modified' },
      { path: 'b', status: 'added' },
      { path: 'c', status: 'added' },
    ]),
    '1 modified · 2 added',
    'several statuses join in SUMMARY_ORDER, not in the order the files happened to arrive',
  )

  const expanded = m.defaultExpanded({
    repos: [
      {
        root: '/w/app',
        label: 'app',
        groups: [
          { id: 'i', name: 'Ignore', kind: 'ignored', files: [] },
          { id: 'd', name: 'Changes', kind: 'changelist', files: [] },
        ],
      },
    ],
  })
  eq(expanded.size, 2, 'the ignored group starts collapsed; the repo and Changes do not')

  // --- commit units --------------------------------------------------------------------

  // The bug this exists to prevent: `Changes` collapsed, its files still ticked.
  const collapsed = new Set([...openAll(one)].filter((id) => !id.includes('default')))
  const rowsCollapsed = m.buildRows(one, collapsed)
  eq(
    rowsCollapsed.some((r) => r.kind === 'file' && r.file.path === 'a.rs'),
    false,
    'a collapsed changelist renders no file rows',
  )
  const units = m.commitUnits(one, defaults)
  eq(units.length, 1, 'one repo, one commit')
  eq(
    units[0].paths.sort(),
    ['a.rs', 'src/b.rs', 'vendor/zlib/z.c'],
    'commit reads the tree, not the rows: a collapsed group still commits its ticked files',
  )
  eq(units[0].changelist, 'default', 'the changelist is named when every tick came from one')

  const mixed = new Set([...m.allFiles(one)])
  eq(
    m.commitUnits(one, mixed)[0].changelist,
    null,
    'a selection spanning changelists names none — the backend commits the paths given',
  )
  eq(
    m.commitUnits(two, m.defaultSelection(two)).map((u) => u.repo),
    ['/w/app', '/w/lib'],
    'two repos are two commits, split by root',
  )

  eq(m.inRepo(m.fileRowId('/w/app', 'a.rs'), '/w/app'), true, 'a row belongs to its own repo')
  eq(
    m.inRepo(m.fileRowId('/w/app-ui', 'a.rs'), '/w/app'),
    false,
    'and not to a repo whose path is merely a prefix of it',
  )

  // --- normalisation -------------------------------------------------------------------

  eq(m.normalizeStatus(undefined), { repos: [] }, 'no payload is an empty panel, not a throw')
  eq(m.normalizeStatus(null), { repos: [] }, 'null too')
  eq(m.normalizeStatus('nope'), { repos: [] }, 'a string too')
  eq(m.normalizeStatus({ repos: 'nope' }), { repos: [] }, 'a wrongly-typed field too')
  eq(
    m.normalizeStatus({ repos: [{ label: 'no root' }] }),
    { repos: [] },
    'a repo with no root is dropped: every git command needs one',
  )
  eq(
    m.normalizeStatus({ repos: [{ root: '/w/app', groups: [{ name: 'Changes' }] }] }),
    { repos: [{ root: '/w/app', label: 'app', groups: [{ id: '0:Changes', name: 'Changes', kind: 'changelist', files: [] }] }] },
    'missing optionals are filled from what is there, not invented',
  )
  eq(
    m.normalizeStatus({
      repos: [{ root: '/w/app', groups: [{ name: 'C', files: [{ path: 'a', status: 'exploded' }] }] }],
    }).repos[0].groups[0].files[0].status,
    'modified',
    'an unknown status shows as modified rather than hiding the file from the commit',
  )
  eq(
    m.normalizeStatus({
      repos: [{ root: '/w/app', groups: [{ name: 'C', files: [{ path: 'a', original_path: 'b' }] }] }],
    }).repos[0].groups[0].files[0].originalPath,
    undefined,
    'snake_case is NOT accepted: a missing rename_all_fields is a Rust bug to fix at source',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('git panel model: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
