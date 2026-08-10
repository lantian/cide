/**
 * Renders the git diff view under node and checks the markup.
 *
 * Companion to `check-diff-selection.mjs`, which pins the *model*: that the set a `Selection`
 * denotes is the set the model calls highlighted. This one closes the last gap between that
 * claim and the screen — it asserts that the rows the component actually marks with
 * `data-selected="true"` are exactly the positions the wire `Selection` resolves to. A
 * component that painted from `marks.has(...)` instead of from `highlightedKeys` would pass
 * the model check and fail this one, which is the whole reason it exists.
 *
 * It also pins the affordances M10's engine needs and had none of: a tri-state box per hunk,
 * a gutter box per line, and a stage/unstage/hold button whose label follows the side.
 *
 * Same harness as `check-git-render.mjs`: SSR-bundle an entry, run it, read its digest.
 *
 * Run: `pnpm --dir ui run check:diff-render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

// Under `node_modules/.cache` rather than the system temp dir: the bundle keeps
// `react-dom/server` external, so node resolves it relative to wherever the output sits.
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-diffrender-'))
let failed = 0
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/panes/gitDiffSmoke.tsx',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { stdio: 'inherit' },
  )

  // `@tauri-apps/api` touches `window` on import, and the pane imports the IPC client.
  globalThis.window = globalThis
  globalThis.location = { search: '' }
  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  await import(`file://${resolve(out, 'gitDiffSmoke.js')}`)
  console.log = log

  const byName = Object.fromEntries(
    JSON.parse(printed.at(-1)).map((d) => [d.name, d]),
  )

  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }

  // --- the claim ---------------------------------------------------------------------------

  for (const d of Object.values(byName)) {
    eq(
      [...d.selected].sort(),
      d.sent,
      `${d.name}: every row drawn as selected is a position the Selection sends, and no other`,
    )
  }

  // --- and the rows are really there ---------------------------------------------------------

  eq(byName.empty.rows, 11, 'every line of every hunk is drawn, context included')
  eq(byName.empty.selected, [], 'nothing is selected until something is ticked')
  eq(byName.empty.hunkBoxes, ['none', 'none', 'none'], 'one tri-state box per hunk')
  eq(byName.empty.applyDisabled, true, 'and the action is disabled with nothing to apply')
  eq(byName.empty.count, '0 of 6 lines selected', 'the footer counts the change lines')

  eq(byName.oneLine.selected, ['1:2'], 'one line ticks exactly one row')
  eq(
    byName.oneLine.hunkBoxes,
    ['none', 'some', 'none'],
    'its hunk goes mixed — `some`, not `all`, is what stops a half-selected hunk claiming a tick',
  )
  eq(byName.oneLine.count, '1 of 6 lines selected · lines', 'and the footer names the encoding')
  eq(byName.oneLine.applyLabel, 'Stage selection', 'the unstaged side stages')

  eq(byName.wholeHunk.hunkBoxes, ['none', 'none', 'all'], 'a full hunk reads as full')
  eq(byName.wholeHunk.count, '3 of 6 lines selected · hunks', 'and is sent as `hunks`')

  eq(byName.ragged.selected, ['0:1', '2:0', '2:2'], 'a ragged selection paints every row it names')
  eq(byName.ragged.count, '3 of 6 lines selected · lines', 'and is sent as `lines`')
  eq(byName.ragged.applyLabel, 'Unstage selection', 'the staged side unstages')

  eq(byName.everything.count, '6 of 6 lines selected · whole', 'everything selected is `whole`')
  eq(
    byName.everything.applyLabel,
    'Use for commit',
    'the combined side hands the selection to the commit rather than staging it, because '
      + '`git_stage` re-derives against a different diff',
  )

  eq(
    byName.junk.selected,
    [],
    'a context row and a position past the end of the diff are painted by nothing — '
      + 'a highlighted row that stages nothing is the quiet half of the same bug',
  )
  eq(byName.junk.hunkBoxes, ['none', 'none', 'none'], 'and neither lights a hunk box')

  eq(
    byName.binary.hunkBoxes,
    [],
    'a file that cannot be partially staged is offered no boxes at all, rather than controls '
      + 'that would be refused',
  )
  eq(byName.binary.selected, [], 'and nothing paints as selected in it')

  eq(byName.gone.rows, 0, 'a side with no diff draws no rows')
  eq(
    byName.gone.selected,
    [],
    'and nothing lingers from the side that had one',
  )

  if (failed === 0) console.log('git diff view render: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
