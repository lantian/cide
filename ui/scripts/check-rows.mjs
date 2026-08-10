/**
 * The rows layout, proved on the markup.
 *
 * The user's complaint was that a vertical divider resized the whole window rather than the
 * row it sits in. That is now structural rather than arithmetic: a chain of same-axis splits
 * is rendered as **one** grid, and a divider is a track of *that* grid — so a vertical
 * divider's box is one row tall because its grid's box is. This script is what pins it.
 *
 * There is no browser and no jsdom in this harness — `ui/scripts/*.mjs` are SSR bundles run
 * under node — so nothing here measures a pixel. It asserts the track templates and the
 * containment structure instead, which is a *stronger* gate: invariance under a drag is
 * checked exactly, on the numbers the browser would be handed, rather than to within a pixel.
 *
 * Same harness as `check-diff-render.mjs`: SSR-bundle an entry, run it, read its digest.
 *
 * Run: `pnpm --dir ui run check:rows`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

// Under `node_modules/.cache` rather than the system temp dir: the bundle keeps
// `react-dom/server` external, so node resolves it relative to wherever the output sits.
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-rows-'))
let failed = 0
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/layout/rowsSmoke.tsx',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { stdio: 'inherit' },
  )

  // `@tauri-apps/api` touches `window` on import, and the tree imports the IPC types.
  globalThis.window = globalThis
  globalThis.location = { search: '' }
  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  await import(`file://${resolve(out, 'rowsSmoke.js')}`)
  console.log = log

  const d = JSON.parse(printed.at(-1))

  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }
  const near = (actual, expected, what) => {
    const ok =
      Array.isArray(actual)
      && actual.length === expected.length
      && actual.every((v, i) => Math.abs(v - expected[i]) < 1e-9)
    if (!ok) {
      console.error(`FAIL ${what}\n  actual:   ${JSON.stringify(actual)}\n  expected: ~${JSON.stringify(expected)}`)
      failed++
    }
  }
  const ok = (cond, what) => {
    if (!cond) {
      console.error(`FAIL ${what}`)
      failed++
    }
  }

  // --- one grid per chain, and only per chain ---------------------------------------------

  const { chains, splitters } = d.plain
  eq(chains.length, 3, 'the Col spine and the two Row chains are three grids, not five')
  eq(
    chains.map((c) => c.axis),
    ['col', 'row', 'row'],
    'outermost is the spine; the rows nest inside it',
  )
  eq(
    chains.map((c) => c.depth),
    [0, 1, 1],
    'both rows are children of the spine, and neither is inside the other',
  )

  // --- the tracks are the shares, with a splitter track between each pair ------------------

  eq(
    chains[0].tracks,
    '0.5fr var(--w-splitter) 0.5fr',
    'the spine is two rows at 50/50 — the user asked for it in those words',
  )
  near(chains[1].fractions, [0.25, 0.25, 0.25, 0.25], 'row one is four equal tiles in one grid')
  near(chains[2].fractions, [0.5, 0.5], 'row two is two tiles, independent of row one')
  eq(
    chains.map((c) => c.tracks.split('var(--w-splitter)').length - 1),
    [1, 3, 1],
    'n members leave n-1 splitter tracks: a comb three levels deep still flattens to one grid',
  )

  // --- the claim the user actually made ---------------------------------------------------

  eq(splitters.length, 5, 'five splitters, one per SplitId in the fixture')
  const vertical = splitters.filter((s) => s.orientation === 'vertical')
  eq(vertical.length, 4, 'four vertical dividers: three in row one, one in row two')
  ok(
    vertical.every((s) => chains[s.chain]?.axis === 'row'),
    'every vertical divider is a track of a `row` chain',
  )
  ok(
    vertical.every((s) => s.chain !== 0),
    'and none of them is a track of the outermost grid — which is why one cannot span the window',
  )
  eq(
    splitters.filter((s) => s.orientation === 'horizontal').map((s) => s.chain),
    [0],
    'the one horizontal divider is the spine’s, and it is the only track that spans the tab',
  )

  // --- moving a divider copies every other track -------------------------------------------

  eq(d.drag.out, [0.375, 0.125, 0.25, 0.25], 'the pair splits 75/25 of its own 0.5')
  ok(d.drag.untouchedIdentical, 'members outside the pair are copied, not recomputed')
  eq(d.drag.pairSum, 0.5, 'and the pair keeps its own budget exactly, so nothing else has to give')
  eq(d.drag.inputUnchanged, '0.25,0.25,0.25,0.25', 'the input vector is not mutated')

  // --- the control the user could not find -------------------------------------------------

  eq(d.plain.addRow, 1, 'the `+ row` strip renders once')
  eq(
    d.plain.buttons,
    ['Add a row with a shell', 'Add a row with a Claude session'],
    'both rows a new row can hold are one click each, and both carry an accessible name',
  )
  eq(d.noStrip.addRow, 0, 'and nothing renders when the host offers no such gesture')
  eq(d.noStrip.splitters, 5, 'the tree still draws, and still resizes, without it')

  // --- maximize still hides everything it used to -------------------------------------------

  eq(d.maximized.addRow, 0, 'the strip is withheld while a pane is maximized')
  eq(d.maximized.splittersByChain, [1, 3, 1], 'the fixture is unchanged by maximizing')
  eq(
    d.maximized.hiddenByChain,
    [1, 3, 0],
    'every divider on the path to the maximized pane is hidden — the spine’s and all three '
      + 'of row one’s. Row two’s is left alone on purpose: it is already inside a member the '
      + 'spine hid, and `visibility` inherits.',
  )

  if (failed === 0) console.log('rows layout: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
