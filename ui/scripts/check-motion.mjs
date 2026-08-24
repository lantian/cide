/**
 * The fence around the app's motion, and around the two things a transition can silently break.
 *
 * # Why this exists at all
 *
 * Until this milestone the app had six `transition` declarations in sixty-two stylesheets, so
 * there was nothing to guard. There are now seventy-odd, on every button, row, tab and menu
 * item — and two mechanisms in this codebase stop working if one lands in the wrong place.
 * Neither failure throws, neither logs, and neither changes a pixel in any snapshot, which is
 * the same sentence `check-resize.mjs`'s header uses about itself.
 *
 * **Hazard 1 — the splitter drag.** `layout/Splitter.tsx` writes `gridTemplateColumns` straight
 * onto the DOM node on every `pointermove`, and `layout/resizeGesture.ts` defers the expensive
 * reactions — xterm's `fit()`, a `session_resize` that reflows the scrollback on the IPC
 * thread, a minimap repaint — to `pointerup`. A transition on a pane box's `width`, `height`,
 * `flex` or `inset` turns every pointer sample into an animation the `ResizeObserver`s keep
 * firing on *after* the gesture has flushed, and the deferral stops being the last word.
 * `check:resize` cannot see it: the module still queues once and still cancels. The CSS is
 * outside its world.
 *
 * **Hazard 2 — the terminal renderer.** `layout/TabContent.module.css` hides inactive tabs with
 * `visibility: hidden` as a hard switch, and `layout/paneHosts.ts`'s `repaintHost` repairs a
 * stalled xterm by nudging `el.style.bottom` from `0px` to `1px` and back two frames later — a
 * genuine layout change, which is the only thing that makes WebKit recompute the clip rect the
 * renderer's IntersectionObserver measures against. A transition on `bottom`, `inset` or `all`
 * on a pane host or any ancestor *interpolates that nudge*, so the repair arrives smeared over
 * the duration or never arrives at all, and the watchdog logs a fix that did nothing.
 *
 * # The four things it asserts
 *
 *   1. **No `transition: all`.** Banned as a word: it is the only spelling that reaches both
 *      hazards by accident, and it reaches them from a rule whose author was thinking about a
 *      hover colour.
 *   2. **Only paint properties are transitioned** — never geometry. The allowed list is below.
 *   3. **No transition at all in the files on the forbidden list**, whatever the property.
 *   4. **Exactly one `prefers-reduced-motion` block, and it is in `tokens.css`.** The setting is
 *      implemented by zeroing `--dur-1`/`--dur-2` there, so a rule with a literal duration is a
 *      rule that ignores it — hence every duration must be a token.
 *
 * Run: `pnpm --dir ui run check:motion`
 */
import { readFileSync, readdirSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

const SRC = resolve('src')
let failed = 0
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

/**
 * Properties a transition may name.
 *
 * Paint only. `transform` is on the list because the settings toggle's knob slides and that is
 * the one place a transform is doing decoration rather than layout; if a transform is ever used
 * to *place* a pane, this list is the wrong fence and the file below belongs on the forbidden
 * one instead.
 */
const ALLOWED = new Set([
  'background-color', 'border-color', 'box-shadow', 'color', 'fill', 'opacity',
  'outline-color', 'stroke', 'transform',
])

/**
 * Files where no transition may appear, whatever it names.
 *
 * The splitters and the pane grid are hazard 1; `TabContent` is hazard 2 and says so in its own
 * comment ("if a later change ever swaps the hiding mechanism for opacity or a transition…").
 * `PaneTitleBar` is the exception that proves the rule and is deliberately *not* here: its
 * `.reveal` fade is on a floating control cluster, out of flow, not on the frame that holds a
 * terminal.
 */
const FORBIDDEN = [
  'layout/SplitTree.module.css',
  'layout/TabContent.module.css',
  'chrome/SidebarSplitter.module.css',
  'toolwindow/ToolWindowSplitter.module.css',
]

/** Every CSS file under `src`, recursively. Enumerated so a new stylesheet is covered. */
function cssFiles(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...cssFiles(path))
    else if (entry.name.endsWith('.css')) out.push(path)
  }
  return out
}

/** Blank comments rather than removing them, so this file's own prose cannot trip its rules. */
const uncomment = (css) => css.replace(/\/\*[\s\S]*?\*\//g, (m) => ' '.repeat(m.length))

let transitions = 0
const reducedMotion = []

for (const file of cssFiles(SRC)) {
  const rel = relative(SRC, file)
  const css = uncomment(readFileSync(file, 'utf8'))

  if (css.includes('prefers-reduced-motion')) reducedMotion.push(rel)

  const forbidden = FORBIDDEN.includes(rel)
  for (const m of css.matchAll(/(?:^|[;{])\s*(transition(?:-property)?)\s*:\s*([^;}]+)/g)) {
    transitions++
    const value = m[2].replace(/\s+/g, ' ').trim()
    ok(!forbidden, `${rel} declares no transition — it is on the forbidden list (\`${value}\`)`)
    ok(
      !/\ball\b/.test(value),
      `${rel} does not say \`transition: all\`, which is the one spelling that reaches a `
        + `geometry property without anybody deciding to (\`${value}\`)`,
    )
    // A shorthand is comma-separated layers, each `<property> <duration> [easing] [delay]`.
    for (const layer of value.split(',')) {
      const prop = layer.trim().split(/\s+/)[0]
      if (!prop || prop === 'none') continue
      ok(
        ALLOWED.has(prop),
        `${rel} transitions \`${prop}\`, which is not a paint property. Only `
          + `${[...ALLOWED].join(', ')} may be transitioned — see the \`--dur-*\` comment in `
          + 'tokens.css for the two mechanisms a geometry transition breaks',
      )
      ok(
        /var\(--dur-[12]\)/.test(layer),
        `${rel}'s \`${prop}\` transition takes its duration from \`--dur-1\`/\`--dur-2\` rather `
          + 'than a literal — reduced motion is implemented by zeroing those tokens, so a '
          + `literal is a rule that ignores the setting (\`${layer.trim()}\`)`,
      )
    }
  }
}

ok(transitions > 0, 'the app has motion at all — this gate is not passing by describing nothing')
ok(
  reducedMotion.length === 1 && reducedMotion[0] === 'styles/tokens.css',
  'exactly one stylesheet answers `prefers-reduced-motion`, and it is `tokens.css`, which does '
    + 'it by zeroing the duration tokens for every rule at once. A second block is a surface '
    + `that opted out of the global answer without saying so (found: ${reducedMotion.join(', ') || 'none'})`,
)

if (failed) {
  console.error(`\n${failed} motion check(s) failed`)
  process.exit(1)
}
console.log(`motion: ok (${transitions} transitions, ${FORBIDDEN.length} files fenced off)`)
