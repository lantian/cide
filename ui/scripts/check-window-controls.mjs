/**
 * Checks `src/chrome/windowControls.ts` — which side the window buttons go on, and in what
 * order, for the platform the webview reports.
 *
 * Worth a check because the failure is silent and remote: it is invisible to `tsc`, invisible
 * on the developing machine (Linux, which is the fallback branch — so a detection that never
 * matches anything still looks right here), and only shows up as a macOS user reaching for a
 * close button that is at the other end of the window. There is no macOS in this harness and
 * no browser, so the rule lives in a DOM-free module and this script feeds it user agent
 * strings directly.
 *
 * Same shape as `check-menu-model.mjs`: a bare `tsc` over one import-free file, then import
 * the output and assert.
 *
 * What this does NOT cover:
 *   - that the header renders the cluster at that end. That is JSX and CSS; the layout audit
 *     (`./run.sh --audit-chrome`) measures the buttons themselves, and neither of its three
 *     `trafficLight` rules — width, height, sibling gap — depends on which side they are on.
 *   - that the buttons act. `WindowFrame.tsx` binds `data-window-button`, and the capability
 *     files are what let the calls through; a missing permission fails at runtime only.
 *
 * Run: `pnpm --dir ui run check:window-controls`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-window-controls-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

/* Real agents, not invented ones: a wrong assumption about the string is exactly the bug this
   file exists to catch, so the fixtures are copied from the shipping web views. */
const MAC_WEBKIT =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15'
const LINUX_WEBKIT =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15'
const WINDOWS_EDGE =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 Edg/122.0.0.0'

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/windowControls.ts',
      '--outDir', out,
      // Pinned, not inferred. With one import-free input file tsc derives the root from that
      // file's own directory and emits to `<out>/windowControls.js`; the sibling checks read
      // `<out>/chrome/<module>.js` because their inputs pull in a second file from `src/`.
      // Stating the root keeps this script's path the same shape as theirs, and keeps it from
      // moving the day this module grows a type import.
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const { isMacUserAgent, windowControlLayout, currentUserAgent } = await import(
    `file://${join(out, 'chrome/windowControls.js')}`
  )

  // =====================================================================================
  // 1. Detection
  // =====================================================================================

  ok(isMacUserAgent(MAC_WEBKIT), "macOS WKWebView's agent is recognised")
  ok(!isMacUserAgent(LINUX_WEBKIT), 'the Linux WebKitGTK agent is not macOS')
  ok(!isMacUserAgent(WINDOWS_EDGE), 'the Windows WebView2 agent is not macOS')
  // `AppleWebKit` appears in all three above, which is the trap: every WebKit-derived engine
  // carries it, so keying on it would put the buttons on the left everywhere.
  ok(
    LINUX_WEBKIT.includes('AppleWebKit') && !isMacUserAgent(LINUX_WEBKIT),
    'detection does not key on `AppleWebKit`, which every WebKit engine carries',
  )
  ok(!isMacUserAgent(''), 'an absent agent is not macOS')

  // =====================================================================================
  // 2. Side and order move together
  // =====================================================================================

  eq(
    windowControlLayout(MAC_WEBKIT),
    { side: 'left', order: ['close', 'minimize', 'zoom'] },
    'macOS: leading edge, close first',
  )
  eq(
    windowControlLayout(LINUX_WEBKIT),
    { side: 'right', order: ['minimize', 'zoom', 'close'] },
    'Linux: trailing edge, close last',
  )
  eq(
    windowControlLayout(WINDOWS_EDGE),
    windowControlLayout(LINUX_WEBKIT),
    'Windows follows the same convention as Linux',
  )

  for (const agent of [MAC_WEBKIT, LINUX_WEBKIT, WINDOWS_EDGE, '']) {
    const { side, order } = windowControlLayout(agent)
    // The whole point of returning both: close belongs at the window's outer corner, which is
    // the start of the cluster on the left and the end of it on the right. An order that does
    // not follow the side is a close button buried between two other controls.
    eq(
      side === 'left' ? order[0] : order[order.length - 1],
      'close',
      `close is outermost for ${agent === '' ? '(no agent)' : side}`,
    )
    eq([...order].sort(), ['close', 'minimize', 'zoom'], `all three buttons are drawn for ${side}`)
  }

  // =====================================================================================
  // 3. The fallback
  // =====================================================================================

  eq(
    windowControlLayout(''),
    windowControlLayout(LINUX_WEBKIT),
    'no navigator falls back to the non-mac layout, which is what this project develops on',
  )
  // Called at module load in `AppHeader.tsx`, so it has to survive being imported outside a
  // browser. Node ≥21 supplies its own `navigator.userAgent` (`Node.js/22`), so the assertion
  // is not that this returns `''` — it is that it returns a string, does not throw, and lands
  // on the platform-default layout rather than the mac one.
  ok(typeof currentUserAgent() === 'string', 'currentUserAgent() is safe off a webview')
  eq(
    windowControlLayout(currentUserAgent()),
    windowControlLayout(LINUX_WEBKIT),
    'a non-browser runtime gets the non-mac layout, which is what this project develops on',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} check(s) failed`)
  process.exit(1)
}
console.log('window controls: OK')
