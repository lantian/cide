/**
 * Checks `src/chrome/PanelBoundary.tsx` and the sites that use it.
 *
 * # What this is defending
 *
 * React unmounts the entire root when a render throws with no boundary above it. This app had
 * none, so a bug in one panel emptied the whole window — and because the rail's choice is
 * restored on launch, the window came back empty on every restart, with no message and with
 * every recovery gesture living in the chrome that the throw had just unmounted.
 *
 * The two halves both have to hold, and only one of them is visible in the component:
 *
 * 1. the boundary really catches — a class with `getDerivedStateFromError`, which is the only
 *    form React treats as a boundary at all. A function component named `PanelBoundary` that
 *    wrapped its children would look right in every diff and catch nothing;
 * 2. every panel is actually wrapped. A boundary nobody uses is the same as no boundary, and
 *    the panel that gets added next is the one that will be forgotten.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
/*
 * Under `node_modules`, not the system temp directory. The emitted module imports `react` as a
 * *value* — a boundary is a class extending `Component` — so node resolves that specifier
 * relative to wherever the output sits, and anywhere outside the package tree cannot find it.
 * `check-log-render.mjs` places its SSR bundle here for the same reason.
 */
const out = mkdtempSync(join(UI, 'node_modules', '.cache', 'cide-boundary-'))
let failed = 0
let checked = 0

const eq = (actual, expected, what) => {
  checked += 1
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => eq(cond === true, true, what)

try {
  const src = readFileSync(join(UI, 'src', 'chrome', 'PanelBoundary.tsx'), 'utf8')
  const app = readFileSync(join(UI, 'src', 'App.tsx'), 'utf8')

  // --- it is a boundary, not something shaped like one ----------------------------------------

  ok(
    /class PanelBoundary extends Component/.test(src),
    'a class, because React has no hook or function form of an error boundary — the whole '
      + 'mechanism is `getDerivedStateFromError`/`componentDidCatch` on a class',
  )
  ok(
    /static getDerivedStateFromError/.test(src),
    'and it declares `getDerivedStateFromError`, which is what makes React route a throw here '
      + 'instead of unmounting the root. `componentDidCatch` alone logs and then still unmounts',
  )
  ok(
    /componentDidCatch/.test(src) && /console\.error/.test(src),
    '…and reports to the console, which `--inspect` routes into the Rust log — the only place a '
      + 'bug report can get the component stack from',
  )
  ok(
    !/from '\.\/PanelBoundary\.module\.css'/.test(src) && /const FALLBACK: React\.CSSProperties/.test(src),
    'the fallback is styled inline. A CSS module is one more import that could be the thing '
      + 'that failed, and an unstyled pile of text is what the user would get at the exact '
      + 'moment they most need to read it',
  )
  ok(
    /onClose/.test(src),
    'and it offers a close. Recovery has to be reachable from inside the failure — before this, '
      + 'every control that could have closed the panel was in the chrome the throw destroyed',
  )

  // --- every panel is wrapped -----------------------------------------------------------------
  //
  // Derived from `App.tsx` rather than listed here, so a panel added later is caught by this
  // check rather than by a user reporting an empty window.

  const views = [...app.matchAll(/\{sidebar\.view === '([a-z]+)' &&/g)].map((m) => m[1])
  ok(views.length >= 6, `the rail's panels were found in App.tsx (saw ${views.length})`)
  for (const view of views) {
    const at = app.indexOf(`{sidebar.view === '${view}' &&`)
    // To the *next* panel rather than a fixed window: the wrapped element can be preceded by a
    // long comment, and a fixed slice would then report a wrapped panel as unwrapped.
    const rest = app.slice(at + 1)
    const until = rest.search(/\{sidebar\.view === '[a-z]+' &&/)
    const next = until === -1 ? rest : rest.slice(0, until)
    ok(
      /<PanelBoundary/.test(next),
      `the ${view} panel is wrapped in a PanelBoundary — otherwise a throw in it empties the `
        + 'whole window and, because the rail\'s choice is restored on launch, keeps doing it',
    )
  }
  const toolAt = app.indexOf('<ToolWindowHost')
  ok(toolAt > 0, 'the tool window is rendered from App.tsx')
  ok(
    /<PanelBoundary[\s\S]{0,300}$/.test(app.slice(0, toolAt)),
    'and it is wrapped too. It is the one panel whose open/closed state is persisted in '
      + 'workspace.json, so an unguarded throw there is the version of this bug that survives a '
      + 'restart by design rather than by accident',
  )

  // --- the boundary compiles and behaves ------------------------------------------------------

  execFileSync(
    'node',
    [
      join(UI, 'node_modules', 'typescript', 'bin', 'tsc'),
      '--jsx', 'react-jsx',
      '--target', 'es2022',
      '--module', 'es2022',
      '--moduleResolution', 'bundler',
      '--skipLibCheck',
      '--outDir', out,
      join(UI, 'src', 'chrome', 'PanelBoundary.tsx'),
    ],
    { stdio: 'inherit', cwd: UI },
  )
  const mod = await import(`file://${join(out, 'PanelBoundary.js')}`)
  eq(
    mod.PanelBoundary.getDerivedStateFromError(new Error('boom')).error.message,
    'boom',
    'the error is kept, message and all — a fallback that will not say why it failed leaves the '
      + 'user nothing to paste into a report',
  )
  eq(
    mod.PanelBoundary.getDerivedStateFromError('a string thrown by a library').error.message,
    'a string thrown by a library',
    'and a non-Error throw is wrapped rather than crashing the fallback on `.message` — which '
      + 'would take the window down from inside the thing that exists to stop that',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`panel boundary: ok (${checked} checks, ${views.length} panels wrapped)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
