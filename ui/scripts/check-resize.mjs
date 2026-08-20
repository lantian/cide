/**
 * Checks `layout/resizeGesture.ts` — the queue that keeps a splitter drag smooth — and greps the
 * five call sites that have to keep using it.
 *
 * # Why this file exists
 *
 * The failure it guards is a performance one, which is exactly the kind this project has no other
 * way to catch. There is no frame timer in CI, no DOM, and no `claude` to resize; a regression
 * here does not throw and does not change a pixel in any snapshot. It shows up only as the app
 * going unresponsive while somebody drags a divider — the bug this module was written for, where
 * every terminal in every tab was refitting and re-reflowing its scrollback on every pointer
 * event, at the mouse's report rate rather than the frame rate.
 *
 * So the rules are asserted as *rules*: deferred work runs once, it runs after the last gesture
 * closes and not before, a key can be withdrawn, and a throwing callback does not strand the
 * others. Each of those is a real failure mode with a real symptom —
 *
 *   - runs more than once   → the storm is back, just later
 *   - runs before the end   → the storm is back, in the middle of the drag
 *   - cannot be withdrawn   → `PaneSlot` resurrects a host for a pane that has closed
 *   - one throw strands all → one bad pane freezes every other pane's geometry for the session
 *
 * — and none of them is visible in a type.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that a `pointermove` is smooth. There is no compositor in this process.
 *   - that `fit()` is expensive, or that a `session_resize` blocks the IPC thread. Those are
 *     facts about xterm and about `crates/cide-app/src/cmd/session.rs`, argued in the comments
 *     at both ends and measured by nothing here.
 *   - the `window` listener at the bottom of the module. There is no window; `noteExternalResize`
 *     is exported and called directly instead.
 *
 * Run: `pnpm --dir ui run check:resize`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-resize-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const ok = (actual, what) => eq(actual, true, what)

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      // Import-free on purpose, so this one `tsc` can take it and node can load the result with
      // no resolver. Keep it that way: the moment it imports React or the IPC client, the module
      // becomes unrunnable here and these rules go back to being untested prose.
      'src/layout/resizeGesture.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const {
    EXTERNAL_SETTLE_MS,
    beginResizeGesture,
    endResizeGesture,
    isResizeGesturing,
    whenResizeSettles,
    cancelResizeSettle,
    noteExternalResize,
  } = await import(`file://${join(out, 'resizeGesture.js')}`)

  /* ------------------------------------------------------------------ the idle path --- */

  ok(!isResizeGesturing(), 'nothing is gesturing at rest')

  let ran = 0
  whenResizeSettles('a', () => { ran++ })
  eq(
    ran,
    1,
    'with no gesture in flight the work runs IMMEDIATELY. Every consumer of this module is on '
      + 'a `ResizeObserver`, and those fire for reasons that are not drags — a pane mounting, a '
      + 'font change, the compositor tiling the window. Deferring those by a frame would make '
      + 'this module a latency tax on the common case',
  )

  /* --------------------------------------------------------------- deferring at all --- */

  ran = 0
  beginResizeGesture()
  ok(isResizeGesturing(), 'a begun gesture reads as in flight')
  whenResizeSettles('a', () => { ran++ })
  eq(ran, 0, 'work offered during a gesture does not run during it')
  endResizeGesture()
  eq(ran, 1, 'and runs when the gesture ends')
  ok(!isResizeGesturing(), 'the gesture is over')

  /* -------------------------------------------------------------- once, not per call --- */

  ran = 0
  beginResizeGesture()
  for (let i = 0; i < 200; i++) whenResizeSettles('a', () => { ran++ })
  endResizeGesture()
  eq(
    ran,
    1,
    'a key offered 200 times in one drag runs ONCE. This is the whole point: a pane reports on '
      + 'every observer callback, and one refit per callback is the bug',
  )

  /* ------------------------------------------------------------- the freshest closure --- */

  let seen = null
  beginResizeGesture()
  whenResizeSettles('a', () => { seen = 'first' })
  whenResizeSettles('a', () => { seen = 'last' })
  endResizeGesture()
  eq(
    seen,
    'last',
    'a re-offer REPLACES the pending callback rather than queueing beside it — the caller is '
      + 're-deferring the same work with a fresher size, and running the stale one too is the '
      + 'duplicate this exists to remove',
  )

  /* ----------------------------------------------------------------- several keys ----- */

  const order = []
  beginResizeGesture()
  whenResizeSettles('pane-1', () => order.push('pane-1'))
  whenResizeSettles('pane-2', () => order.push('pane-2'))
  whenResizeSettles('pane-3', () => order.push('pane-3'))
  endResizeGesture()
  eq(order, ['pane-1', 'pane-2', 'pane-3'], 'distinct keys each run, in the order they were offered')

  /* ------------------------------------------------------------------ object keys ----- */

  const one = {}
  const two = {}
  ran = 0
  beginResizeGesture()
  whenResizeSettles(one, () => { ran++ })
  whenResizeSettles(two, () => { ran++ })
  endResizeGesture()
  eq(
    ran,
    2,
    'an object identity is a legal key. Component instances have no name to key on, and two '
      + 'instances sharing a string key would silently lose one of them',
  )

  /* --------------------------------------------------------------------- nesting ------ */

  ran = 0
  beginResizeGesture()
  beginResizeGesture()
  whenResizeSettles('a', () => { ran++ })
  endResizeGesture()
  eq(
    ran,
    0,
    'a nested gesture ending does NOT flush. A window manager can resize the window while a '
      + 'splitter is held; with a boolean instead of a count, the first to finish would flush the '
      + "other's work in the middle of it, which is the per-frame refit this module prevents",
  )
  endResizeGesture()
  eq(ran, 1, 'the outermost end is the one that flushes')

  ran = 0
  endResizeGesture()
  eq(
    ran,
    0,
    'an unbalanced end is a no-op rather than driving the count negative, which would make the '
      + 'next real gesture flush on its own first frame',
  )

  /* ------------------------------------------------------------------- cancelling ----- */

  ran = 0
  beginResizeGesture()
  whenResizeSettles('a', () => { ran++ })
  cancelResizeSettle('a')
  endResizeGesture()
  eq(
    ran,
    0,
    'a withdrawn key does not run. `PaneSlot` withdraws on unmount, and its deferred work reaches '
      + '`getHost`, which BUILDS a host for a pane that has none — so a flush landing after the '
      + 'slot has gone would resurrect a host for a pane that is no longer in the tree',
  )

  /* ------------------------------------------------------------------- one throwing --- */

  const survived = []
  const errors = console.error
  console.error = () => {}
  beginResizeGesture()
  whenResizeSettles('bad', () => { throw new Error('one pane could not refit') })
  whenResizeSettles('good', () => survived.push('good'))
  endResizeGesture()
  console.error = errors
  eq(
    survived,
    ['good'],
    'one callback throwing does not strand the rest. A single pane whose terminal has gone would '
      + "otherwise freeze every other pane's geometry for the rest of the session",
  )

  /* ---------------------------------------------------- re-deferring from a callback --- */

  ran = 0
  beginResizeGesture()
  whenResizeSettles('a', () => {
    // A refit changes a box, which re-enters the observer it came from. By the time a callback
    // runs the gesture is over, so this must run immediately rather than queue into a drain
    // that is already in progress and be dropped.
    whenResizeSettles('b', () => { ran++ })
  })
  endResizeGesture()
  eq(ran, 1, 'work offered from inside a flushed callback runs, rather than being swallowed')

  /* ------------------------------------------------- the frame, where there is one ---- */

  let frame = null
  globalThis.requestAnimationFrame = (fn) => {
    frame = fn
    return 1
  }
  globalThis.cancelAnimationFrame = () => {
    frame = null
  }
  ran = 0
  beginResizeGesture()
  whenResizeSettles('a', () => { ran++ })
  endResizeGesture()
  eq(
    ran,
    0,
    'where there IS a `requestAnimationFrame`, the flush waits for it — so the browser paints the '
      + 'divider where the user let go before the terminals reflow, rather than the settle '
      + "delaying the gesture's last frame",
  )
  frame?.()
  eq(ran, 1, 'and runs on that frame')
  delete globalThis.requestAnimationFrame
  delete globalThis.cancelAnimationFrame

  /* ------------------------------------------------------------- the window's resize -- */

  ran = 0
  noteExternalResize()
  ok(
    isResizeGesturing(),
    'a window resize opens a gesture. The OS window edge has no `pointerup` this window ever '
      + 'sees — the compositor owns that gesture — so a quiet period is the only end available',
  )
  whenResizeSettles('a', () => { ran++ })
  noteExternalResize()
  noteExternalResize()
  eq(ran, 0, 'and a burst of resize events is ONE gesture, not one per event')
  await sleep(EXTERNAL_SETTLE_MS + 60)
  eq(ran, 1, `it settles ${EXTERNAL_SETTLE_MS}ms after the last event`)
  ok(!isResizeGesturing(), 'and closes itself')

  /* ------------------------------------------------------------------- the call sites -- */
  //
  // Greps, because the rule is about which code path a file takes and there is no way to observe
  // that from here. Every one of these was the bug at some point.

  const read = (p) => readFileSync(p, 'utf8')

  ok(
    /typeof window\.addEventListener === 'function'/.test(read('src/layout/resizeGesture.ts')),
    "the module's own `resize` listener is guarded on `addEventListener`, not merely on `window`. "
      + '`check-rows.mjs` and `check-diff-render.mjs` SSR-bundle their entry and run it against a '
      + 'stub `window` that has no such method, so the weaker guard throws at module-evaluation '
      + 'time and takes down two checks that have nothing to do with resizing',
  )

  const paneSlot = read('src/layout/PaneSlot.tsx')
  ok(
    /whenResizeSettles\(paneId/.test(paneSlot),
    "`PaneSlot`'s ResizeObserver goes through `whenResizeSettles`. Calling `onResize` directly is "
      + 'the original bug: its only consumer is `syncSize`, which calls `FitAddon.fit()` — a '
      + 'DOM-renderer reflow over 5000 lines of scrollback — and then a synchronous '
      + '`session_resize` that reflows the vt100 mirror on the IPC thread',
  )
  ok(
    /cancelResizeSettle\(paneId\)/.test(paneSlot),
    '`PaneSlot` withdraws its key on unmount, before `parkHost`',
  )

  const splitter = read('src/layout/Splitter.tsx')
  ok(
    !/onPointerMove[\s\S]{0,600}?getBoundingClientRect/.test(splitter),
    "`Splitter`'s `onPointerMove` does not measure. The read used to sit immediately after the "
      + "previous move's style write, which is a forced synchronous layout of the whole pane "
      + "subtree — terminals included — at the mouse's report rate. The box is measured once, at "
      + 'pointerdown',
  )
  ok(
    /schedulePaint\(\)/.test(splitter),
    "`Splitter` coalesces its style write onto a frame rather than writing per pointer event",
  )

  for (const [file, source] of [
    ['src/layout/Splitter.tsx', splitter],
    ['src/chrome/SidebarSplitter.tsx', read('src/chrome/SidebarSplitter.tsx')],
    ['src/toolwindow/ToolWindowSplitter.tsx', read('src/toolwindow/ToolWindowSplitter.tsx')],
  ]) {
    const begins = (source.match(/beginResizeGesture\(\)/g) ?? []).length
    const ends = (source.match(/endResizeGesture\(\)/g) ?? []).length
    ok(
      begins > 0 && ends >= begins,
      `${file} opens a resize gesture and closes it at least as often as it opens one. `
        + 'A gesture left open stops every terminal in the window refitting until the watchdog '
        + 'notices — three places have to close it: `stop`, the keyboard flush, and unmount',
    )
  }

  for (const [file, source] of [
    ['src/chrome/SidebarSplitter.tsx', read('src/chrome/SidebarSplitter.tsx')],
    ['src/toolwindow/ToolWindowSplitter.tsx', read('src/toolwindow/ToolWindowSplitter.tsx')],
  ]) {
    ok(
      /function writeToken\(/.test(source),
      `${file} writes its token through \`writeToken\`, which drops a write that changes nothing. `
        + 'These are inherited custom properties on `<html>`: every write invalidates style for '
        + 'the whole document, and most of this document is terminal rows',
    )
    ok(
      !/const show = \([a-z]+: number\) => \{\n    live\.current = [a-z]+\n    document\./.test(
        source,
      ),
      `${file}'s \`show\` does not write the DOM inline — the write is coalesced onto a frame, `
        + 'because WebKitGTK reports `pointermove` at the mouse rate and not the frame rate',
    )
  }

  const store = read('src/store/workspace.ts')
  ok(
    /setRatio: async \(project, tab, split, ratio\) => \{\n\s*await paneApi\.setRatio\([^)]*\)\n\s*\},/
      .test(store),
    '`setRatio` does not re-hydrate. `WorkspaceState::update` already broadcasts to every window '
      + 'including this one, so the extra round trip was a SECOND whole-app re-render landing in '
      + 'the frame the user let go of the divider',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`resize gesture: ok (window resize settles after ${EXTERNAL_SETTLE_MS}ms)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
