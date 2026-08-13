/**
 * Renders the git panel under node and checks what came out.
 *
 * `pnpm build` proves the panel compiles; this proves it paints. It bundles
 * `src/sidebar/GitPanel/smokeEntry.tsx` for SSR, runs it, and asserts on the digest it prints:
 * that the repo level really is elided with one repo and present with two, that a submodule
 * renders as a nested repository four levels deep rather than an opaque row, that the guard bar
 * names its repository in a multi-root workspace, and that the mock story's footer reads
 * `2 modified`.
 *
 * Every story goes through `normalizeStatus` from a real `cide_ipc::git::ChangesTree`, so this
 * check now covers the seam that used to be uncovered: the fixtures used to be written in the
 * panel's own invented shape, which is why they rendered perfectly while the panel was empty
 * against every real repository.
 *
 * Two globals are stubbed before the bundle loads. `location` is what `storyFromQuery`
 * reads; `window` is touched by `@tauri-apps/api` on import. Neither is faked deeper than
 * that — anything that needs a real browser is not something this check can speak to.
 *
 * Run: `pnpm --dir ui run check:render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

/*
 * Built inside `node_modules/.cache` rather than in the system temp dir, which is what
 * `check-status-format.mjs` uses. The bundle keeps `react-dom/server` external, so node
 * resolves it from wherever the output sits: under /tmp there is no `node_modules` above
 * it and the import fails.
 */
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-render-'))
/*
 * The exit code is carried out of the `try` rather than taken there with `process.exit`:
 * `process.exit` skips `finally`, and this build directory lives under `node_modules`, so
 * every failing run would leave one behind. (`check-status-format.mjs` builds into the
 * system temp dir, where the same pattern costs nothing.)
 */
let failed = 0
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/sidebar/GitPanel/smokeEntry.tsx',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { stdio: 'inherit' },
  )

  globalThis.window = globalThis
  globalThis.location = { search: '' }
  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  await import(`file://${resolve(out, 'smokeEntry.js')}`)
  console.log = log

  const digests = JSON.parse(printed.at(-1))
  const byName = Object.fromEntries(digests.map((d) => [d.story, d]))

  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }

  // --- one repository ------------------------------------------------------------------

  eq(byName.mock.repoRows, 0, 'one repo: no repo rows, so the panel is the mock exactly')
  eq(
    byName.mock.rows.length > 0,
    true,
    'and it renders rows at all — a fixture that reaches `normalizeStatus` in the wrong '
      + 'shape produces exactly zero, which is what the empty-panel bug looked like',
  )
  eq(
    byName.mock.rows.slice(0, 5),
    ['L1:mixed', 'L2:true', 'L3:true', 'L2:mixed', 'L3:mixed'],
    'Changes is partial because one of its two files is staged only in part: index and '
      + 'worktree both dirty. The group must not claim a full tick. Its two files are in two '
      + 'directories, so each one now sits under a directory row of its own (L2) with the file '
      + 'inside it (L3) — the row a drag onto another changelist grabs, and the partiality '
      + 'climbs through it exactly as it climbs to the group',
  )
  /*
   * The row selection, and the ARIA claim it made honest.
   *
   * The tree has carried `aria-multiselectable="true"` since it shipped, on markup whose
   * `aria-selected` tracked a single *cursor*. That is a promise to a screen reader the app
   * could not keep, and it stayed unnoticed because nothing rendered the tree and read the two
   * attributes together. Both are pinned here, from both ends.
   *
   * `selectedRows` is 0 on a panel that has just painted, and that is the assertion, not an
   * accident of the fixture: `defaultSelection` *ticks* the active changelist so that "Claude
   * edited a file, commit it" is one click, and `rows` above shows those ticks as `true`. The
   * row selection starts empty regardless, because selecting rows nobody pointed at is what
   * made the drag carry a whole changelist when the user grabbed one file out of it. A
   * non-zero count here means the two concepts have been wired back together.
   */
  eq(byName.mock.multiSelectable, true, 'the tree claims multi-selection to assistive tech')
  eq(
    byName.mock.selectedRows,
    0,
    'and nothing is selected on a freshly painted panel, while its checkboxes are already '
      + 'ticked — the ticks are what a commit takes, the selection is what a gesture is about, '
      + 'and this is the one place the gap between them is visible',
  )
  eq(byName.mock.summary, '2 modified', "the mock's footer, verbatim")
  eq(byName.mock.guard, null, 'no guard bar unless the index actually moved')
  eq(
    byName.mock.commitEnabled,
    false,
    'Commit stays disabled while the message is empty — git would abort anyway, and after '
      + 'the index had been rewritten',
  )

  // --- the external-staging guard --------------------------------------------------------

  eq(byName.guard.guard, '⚠ Staging changed outside cide Reload Overwrite', 'the guard bar')
  eq(
    byName.guard.rows,
    byName.mock.rows,
    'the guard bar changes nothing about the tree below it — it is a warning, not a mode',
  )

  // --- two roots and a submodule ----------------------------------------------------------

  eq(
    byName.multi.repoRows,
    3,
    'two roots and a submodule are three repositories: a submodule has its own index and its '
      + 'own changelists, so it gets a repo row rather than a group inside its parent',
  )
  eq(byName.multi.rows[0], 'L1:mixed', 'the repo row is the new top level')
  eq(
    byName.multi.rows.slice(23, 31),
    ['L1:mixed', 'L2:true', 'L3:true', 'L4:true', 'L2:mixed', 'L3:mixed', 'L4:true', 'L4:mixed'],
    'the second root, its own changelist, the directory its one file is in and the file, then '
      + 'the submodule nested inside it with its changelist and its two files — five levels '
      + 'deep, not one opaque row',
  )
  eq(
    byName.multi.summary,
    '5 modified',
    "the footer counts every root's active changelist, the submodule's included",
  )
  eq(
    byName.multi.guard,
    '⚠ Staging changed outside cide · hub-core Reload Overwrite',
    'with more than one root the bar names the repo whose index moved',
  )

  // --- nothing to commit -------------------------------------------------------------------

  eq(byName.empty.rows, [], 'a clean tree renders no rows')
  eq(byName.empty.summary, 'nothing selected', 'and says so in the footer')

  if (failed === 0) console.log('git panel render: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
