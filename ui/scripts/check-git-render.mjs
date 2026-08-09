/**
 * Renders the git panel under node and checks what came out.
 *
 * The panel is not mounted by `App.tsx` in this milestone — the sidebar that hosts it is a
 * separate surface — so `pnpm build` proves only that it compiles. This bundles
 * `src/sidebar/GitPanel/smokeEntry.tsx` for SSR, runs it, and asserts on the digest it
 * prints: that the repo level really is elided with one repo and present with two, that
 * the guard bar names its repository in a multi-root workspace, and that the mock story's
 * footer reads `2 modified`.
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

  eq(byName.mock.repoRows, 0, 'one repo: no repo rows, so the panel is the mock exactly')
  eq(
    byName.mock.rows.slice(0, 3),
    ['L1:mixed', 'L2:true', 'L2:mixed'],
    'Changes is partial because one of its two files is: the group must not claim a full tick',
  )
  eq(byName.mock.summary, '2 modified', "the mock's footer, verbatim")
  eq(byName.mock.guard, null, 'no guard bar unless the index actually moved')
  eq(
    byName.mock.commitEnabled,
    false,
    'Commit stays disabled while the message is empty — git would abort anyway, and after '
      + 'the index had been rewritten',
  )

  eq(byName.guard.guard, '⚠ Staging changed outside cide Reload Overwrite', 'the guard bar')
  eq(
    byName.guard.rows,
    byName.mock.rows,
    'the guard bar changes nothing about the tree below it — it is a warning, not a mode',
  )

  eq(byName.multi.repoRows, 2, 'two repos: a repo row each, above the changelists')
  eq(byName.multi.rows[0], 'L1:mixed', 'the repo row is the new top level')
  eq(
    byName.multi.rows.slice(16, 21),
    ['L2:mixed', 'L3:mixed', 'L4:true', 'L4:mixed', 'L3:true'],
    'a submodule nests inside its changelist and its files inside it — four levels deep, '
      + 'not one opaque row',
  )
  eq(
    byName.multi.guard,
    '⚠ Staging changed outside cide · hub-core Reload Overwrite',
    'with more than one root the bar names the repo whose index moved',
  )

  eq(byName.empty.rows, [], 'a clean tree renders no rows')
  eq(byName.empty.summary, 'nothing selected', 'and says so in the footer')

  if (failed === 0) console.log('git panel render: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
