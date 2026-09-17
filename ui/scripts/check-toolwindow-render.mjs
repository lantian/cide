/**
 * Renders the git tool window's frame under node and checks what came out.
 *
 * `check:toolwindow` proves the rules as *functions*: which tab comes back after a close, that the
 * Log tab is never closable, that the height clamps. This proves the frame **paints** them. The
 * two are not the same claim, and the gap between them is where this class of bug lives: a tab row
 * that lost its close buttons, or grew one on the Log tab, still renders something that looks like
 * a tab row, still passes every model assertion, and is only wrong on screen.
 *
 * Pattern B, the same shape as `check-git-render.mjs` and `check-diff-render.mjs`: SSR-bundle one
 * entry with Vite, run it, assert on the digest it prints.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that the panel is the right height, or that dragging its edge resizes it. There is no DOM
 *     and no layout in this process; `check:toolwindow` owns the arithmetic and `--h-toolwindow`
 *     reaching the screen rests on the cascade, which neither can test.
 *   - that clicking a tab activates it. The handlers are stubs here; `check:toolwindow` drives
 *     the state machine those handlers call.
 *
 * Run: `pnpm --dir ui run check:toolwindow-render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

/*
 * Inside `node_modules/.cache` rather than the system temp dir: the bundle keeps
 * `react-dom/server` external, so node resolves it relative to the output, and under /tmp there
 * is no `node_modules` above it.
 */
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-toolwindow-render-'))

/* Carried out of the `try` rather than taken with `process.exit`, which would skip the `finally`
   and leave a build directory under `node_modules` on every failing run. */
let failed = 0

try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/toolwindow/toolWindowSmoke.tsx',
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
  await import(`file://${resolve(out, 'toolWindowSmoke.js')}`)
  console.log = log

  const byName = Object.fromEntries(JSON.parse(printed.at(-1)).map((d) => [d.story, d]))

  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }

  // --- the Log tab is a position, not an entity ------------------------------------------
  //
  // It is always there, always first, and can never be closed. The model encodes that by giving
  // it no id and keeping it out of `history`; this is the half that proves the row agrees.

  eq(byName.logOnly.labels, ['Log', 'Docker'], 'a project with no history tabs shows exactly the Log tab')
  eq(byName.logOnly.active, 'Log', 'and it is the one selected')
  eq(byName.logOnly.closable, [], 'the Log tab renders NO close control — it cannot be closed')
  eq(byName.logOnly.hide, true, 'the panel can still be hidden, which is the way out')

  eq(
    byName.twoHistories.labels,
    ['Log', 'Docker', 'log.rs', 'App.tsx'],
    'history tabs follow the Log tab in the order they were opened, labelled by basename',
  )
  eq(
    byName.twoHistories.closable,
    ['log.rs', 'App.tsx'],
    'every history tab renders a close control and the Log tab still does not — the whole '
      + 'asymmetry, in the markup rather than in the model',
  )
  eq(byName.twoHistories.active, 'App.tsx', 'opening a history tab puts it in front')
  eq(
    byName.twoHistories.tabRoles,
    4,
    'four tabs claim role="tab" and nothing else does — Log, Docker and the two histories. The '
      + 'count moved with M46 and the claim did not: the close buttons are siblings of the '
      + 'tabs, not children, because a <button> inside a role="tab" is unreachable in the tab’s '
      + 'own reading order and a close control a keyboard cannot reach is one only a mouse can use',
  )

  eq(byName.oneHistory.labels, ['Log', 'Docker', 'log.rs'], 'one history tab, after the Log tab')
  eq(byName.logInFront.active, 'Log', 'showing the Log tab does not close the history tabs')
  eq(
    byName.logInFront.labels,
    ['Log', 'Docker', 'log.rs', 'App.tsx'],
    'and leaves them exactly where they were',
  )

  // --- the body follows the selection ----------------------------------------------------
  //
  // The host slots the active tab's view in here. If the frame drew the wrong one the panel
  // would show one tab's content under another tab's heading, which no model assertion can see.

  eq(byName.twoHistories.body, 'body for App.tsx', 'the body is the active tab’s')
  eq(byName.logInFront.body, 'body for Log', 'and follows the selection when it moves')

  if (failed === 0) console.log('tool window render: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
