/**
 * Checks `src/chrome/tabOverflow.ts` — which tabs the strip is hiding, and that the `▾` control
 * built on top of it is actually wired.
 *
 * > *"When opened files more then current width of window - i doesn't see all other tabs to the
 * > right. Need to create a dropdown icon (like > but down) that will allow to open a list of
 * > opened files that is outside of current view."*
 *
 * The rule is arithmetic over boxes, and every wrong version of it type-checks and is invisible
 * in a screenshot. Three in particular, and each has an assertion below:
 *
 * * the **edge case**, where a tab whose right edge lands exactly on the viewport is reported as
 *   clipped — a chevron that is permanently on, listing a tab the user is looking at, which is
 *   how a control stops being trusted;
 * * the **unlaid-out strip**, where `clientWidth` is 0 on the first commit and a missing guard
 *   makes *every* tab hidden, so the control flashes on for a frame on every window open;
 * * the **scrolled** strip, where the hidden tabs are the leading ones rather than the trailing
 *   ones — reachable in the shipped app because `overflow: hidden` is still a scroll container
 *   and focusing a clipped tab's button scrolls it.
 *
 * The tail reads `TabStrip.tsx` and the stylesheet as *source*, because the wiring cannot be
 * executed here: there is no DOM, no ResizeObserver and no font loader. That matters more than
 * usual for this feature — a correct rule that nothing calls is this repository's signature
 * defect, and a geometry detector is exactly the shape that hides it, because nothing here can
 * resize a window.
 *
 * Every source assertion runs against **comment-stripped** text, for the reason
 * `check-tab-drag.mjs` states at length: this feature's modules carry paragraphs naming
 * `clippedTabs`, `ResizeObserver` and `overflowSlot`, so a grep over raw source certifies the
 * prose of a feature that has been deleted.
 *
 * Run: `pnpm --dir ui run check:tab-overflow`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-tab-overflow-'))
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

/**
 * Remove comments before grepping. Lifted from `check-tab-drag.mjs`, deliberately verbatim.
 *
 * String literals are kept: the DOM attribute name (`data-tab-id`) and the ARIA values this
 * control is asserted on exist nowhere else, and blanking them would make those assertions
 * unwritable — a worse failure than the one being guarded against. The stripper is naive about
 * regex literals, which is safe for the files it is applied to here.
 */
const strip = (src) =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/tabOverflow.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // `tabOverflow.ts` imports nothing at all, so plain ESM output loads under node directly.
      // The import-free shape is the point: it is what lets this script compile the file alone.
      '--module', 'esnext',
      '--moduleResolution', 'bundler',
      '--target', 'es2022',
      '--strict',
      // Both flags the app's own tsconfig sets, so the module is compiled the way it ships.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
      '--lib', 'es2023',
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'chrome', 'tabOverflow.js')}`)
  const { EDGE_TOLERANCE_PX, clippedTabs, overflowHint } = m

  /*
   * A strip as the app has it, in the units the DOM reports: `offsetLeft` and `offsetWidth`
   * against the tablist, with the 1px flex gap showing up as the 2px difference between one
   * tab's right edge and the next tab's left.
   *
   *   t0  0 …  78     t1  80 … 178     t2  180 … 298     t3  300 … 420
   */
  const boxes = [
    { id: 't0', left: 0, width: 78 },
    { id: 't1', left: 80, width: 98 },
    { id: 't2', left: 180, width: 118 },
    { id: 't3', left: 300, width: 120 },
  ]
  const at = (viewport, scroll = 0) => clippedTabs(boxes, { viewport, scroll })

  // =====================================================================================
  // 1. Nothing is hidden when nothing is hidden
  // =====================================================================================

  eq(at(420), [], 'a strip wide enough for every tab reports none of them')
  eq(at(900), [], 'and so does one with room to spare')
  eq(clippedTabs([], { viewport: 320, scroll: 0 }), [], 'an empty strip hides nothing')

  // =====================================================================================
  // 2. The trailing edge — the case the report is about
  // =====================================================================================

  eq(at(320), ['t3'], 'the tab past the right edge is the one reported')
  eq(at(200), ['t2', 't3'], 'two clipped tabs come back in strip order, ids only')
  eq(at(60), ['t0', 't1', 't2', 't3'], 'a strip too narrow for even the console reports all of it')

  /*
   * "Not *fully* visible", not "not visible at all". The tab the strip cuts in half is the one
   * the user complained about: its name is unreadable and its close button is past the edge, so
   * it is as unreachable as the tabs beyond it. `at(350)` leaves 50 of t3's 120px showing.
   */
  eq(at(350), ['t3'], 'a half-painted tab counts as hidden — half a tab is not a reachable tab')

  // =====================================================================================
  // 3. The edges, to the pixel
  // =====================================================================================
  //
  // The off-by-one here is the difference between a control that appears when it should and one
  // that is on permanently, listing a tab the user is looking at.

  eq(at(298), ['t3'], 'a tab whose right edge lands EXACTLY on the viewport edge is not clipped')
  eq(
    at(419),
    [],
    `an overhang of exactly EDGE_TOLERANCE_PX (${EDGE_TOLERANCE_PX}px) is rounding, not clipping`,
  )
  eq(at(418), ['t3'], 'and 2px over is a real clip — the tolerance is pinned in both directions')
  eq(EDGE_TOLERANCE_PX, 1, 'one pixel, because offsetLeft/offsetWidth are integers in WebKit')

  // =====================================================================================
  // 4. The unlaid-out strip
  // =====================================================================================
  //
  // React commits before layout runs, so the first measurement of a fresh window sees a
  // `clientWidth` of 0. Without the guard every tab is "clipped" and the chevron flashes on for a
  // frame on every window open — a control that comes and goes with no gesture behind it.

  eq(at(0), [], 'a strip that has not been laid out yet reports nothing, not everything')
  eq(at(-5), [], 'and neither does a negative width, which is not a state with an answer')

  // =====================================================================================
  // 5. The scrolled strip
  // =====================================================================================
  //
  // `overflow: hidden` is still a scroll container, and Tab-focusing a clipped tab's `<button>`
  // scrolls it — which happens in the shipped app, because the focus ring is the tabs' only
  // keyboard affordance. When it does, the hidden tabs are the LEADING ones, and a detector that
  // only looked at the right-hand edge would report an empty list on a strip missing three tabs.

  eq(
    at(320, 120),
    ['t0', 't1'],
    'scrolled right, the leading tabs are the hidden ones — and the trailing one that was '
      + 'clipped at scroll 0 no longer is',
  )
  eq(at(200, 120), ['t0', 't1', 't3'], 'and both ends can be clipped at once')
  /*
   * The pinned console is not special-cased, and that is deliberate rather than an oversight.
   * Its pin is about closing and moving — `menuModel.closable`, `tabDrag.movable` and
   * `cide_core::workspace` behind both — and neither has anything to do with reaching it.
   */
  ok(
    at(320, 120).includes('t0'),
    'the pinned console is listed when it is out of view: reachability is not pinning',
  )

  // =====================================================================================
  // 6. The sentence
  // =====================================================================================

  eq(overflowHint(1), '1 tab is not in view', 'the singular is spelled out')
  eq(overflowHint(3), '3 tabs are not in view', 'and so is the plural — "1 tabs" is what ships')

  // =====================================================================================
  // 7. The wiring, as source
  // =====================================================================================
  //
  // Everything above passes against a module nothing imports.

  const strip_ = strip(readFileSync('src/chrome/TabStrip.tsx', 'utf8'))
  // Stripped like the rest, and here it is least optional: a stylesheet is nothing but `/* … */`,
  // and the paragraph above `.overflowSlot` explains the very declarations these assertions look
  // for. Delete the rules, leave the prose, and a raw grep still passes.
  const css = strip(readFileSync('src/chrome/TabStrip.module.css', 'utf8'))

  // The stripper has to actually work, or every assertion below is decorative.
  ok(
    !strip('const x = 1 /* clippedTabs */\nconst y = 2 // clippedTabs').includes('clippedTabs'),
    'the stripper removes block comments and line comments',
  )
  ok(
    strip('const clippedTabs = "data-tab-id"').includes('clippedTabs')
      && strip('const clippedTabs = "data-tab-id"').includes('data-tab-id'),
    'and leaves real code — including the DOM attribute names — alone',
  )

  // The measurement, and the two numbers it must be taken from. `clientWidth` rather than
  // `offsetWidth` is the whole detector: the former is the box's *visible* width, which is what
  // shrinks when the strip overflows.
  ok(
    /clippedTabs\(boxes, \{ viewport: box\.clientWidth, scroll: box\.scrollLeft \}\)/.test(strip_),
    'the strip asks the rule, and asks it with the tablist’s visible width and its scroll offset',
  )
  ok(
    /querySelectorAll<HTMLElement>\('\[data-tab-id\]'\)/.test(strip_)
      && /offsetLeft/.test(strip_)
      && /offsetWidth/.test(strip_),
    'and measures the real tab boxes off the DOM rather than guessing from a tab count — widths '
      + 'vary with the filename, the badge, the shape class and the dirty dot',
  )

  /*
   * Three triggers, three assertions, because each covers a case the other two miss:
   *
   *  - the per-commit effect with no dep array: a tab going dirty swaps a 12px `×` for a 14px
   *    `•`, an agent renames a `claudeFull` tab. No box changes size, so the observer is silent;
   *  - the ResizeObserver: the window is resized, the sidebar splitter is dragged. No commit
   *    happens, so the effect never runs;
   *  - `fonts.ready`: the webfont swaps under an ALREADY overfull strip, so every tab changes
   *    width while `.tabs` does not — no commit and no resize.
   *
   * `useEffect`, NOT `useLayoutEffect`: this was a layout effect once, and its DOM reads before
   * paint forced a synchronous layout of the entire window — every editor, every terminal — on
   * every App commit. After paint the same reads are free. The regex pins the passive form so a
   * well-meaning "make it never a frame late" revert has to argue with this comment first.
   */
  ok(
    /\n {2}useEffect\(measure\)/.test(strip_),
    'the measurement runs on every commit with NO dependency array, as a PASSIVE effect '
      + '(useEffect, after paint) — an array here is how the list goes stale, and a '
      + 'useLayoutEffect here is a forced whole-document reflow per commit',
  )
  ok(
    /new ResizeObserver\(/.test(strip_) && /ro\??\.observe\(box\)/.test(strip_),
    'a ResizeObserver on the tablist catches a resize that re-renders nothing',
  )
  /*
   * The fourth trigger, and the one that decides whether `scroll` is an input or a decoration.
   * `clippedTabs` compares against BOTH edges because `overflow: hidden` is still a scroll
   * container and WebKit scrolls one to reveal a focused descendant — which puts the LEADING
   * tabs out of view. That scroll changes no box's size and re-renders nothing, so without a
   * listener the rule's `scroll` parameter could only ever be read as 0 plus whatever an
   * unrelated commit happened to catch: the scrolled cases asserted in section 5 would be
   * arithmetic nothing in the app could reach.
   */
  ok(
    /addEventListener\('scroll'/.test(strip_) && /removeEventListener\('scroll'/.test(strip_),
    'a scroll listener on the tablist feeds `scrollLeft` to the rule — and is torn down again',
  )
  ok(
    /document\.fonts\.ready\.then\(/.test(strip_),
    'and `fonts.ready` catches the webfont swap, which changes every tab’s width without '
      + 'changing the box’s — so neither of the other two fires',
  )
  ok(
    /typeof ResizeObserver === 'undefined'/.test(strip_),
    'the observer is guarded the way PaneSlot and GitDiffPane guard theirs',
  )

  // The control itself: present only when something is actually hidden.
  ok(
    /\{hidden\.length > 0 && \(/.test(strip_),
    'the chevron is rendered ONLY when at least one tab is clipped',
  )
  ok(
    /className=\{styles\.overflowSlot\}/.test(strip_),
    'and it sits in a slot of its own',
  )
  /*
   * The slot is OUTSIDE the tablist, which is two requirements at once: `role="tablist"` may not
   * contain a generic element without breaking the ownership the tabs pattern needs, and a
   * control listing what is clipped, placed inside the box that clips, eventually clips itself —
   * which is the bug `AppHeader` fixed by moving `<ProjectMenu>` out of its own `.tabs` box.
   */
  ok(
    strip_.indexOf('role="tablist"') < strip_.indexOf('className={styles.spacer}')
      && strip_.indexOf('className={styles.spacer}')
        < strip_.indexOf('className={styles.overflowSlot}'),
    'the slot is a SIBLING of the tablist, after the spacer — not a child of the box that clips',
  )

  // Reuse of the app's one menu, which is the whole keyboard and dismissal story: Escape,
  // arrows, Home/End, outside-pointerdown, scroll, blur, and portalling past `overflow: hidden`.
  ok(
    /const overflow = useContextMenu\(\{/.test(strip_) && /\{overflow\.menu\}/.test(strip_),
    'the list is a `useContextMenu`, and the menu it returns is actually rendered',
  )
  ok(
    !/Escape/.test(strip_),
    'and Escape is NOT hand-rolled here — a second dismissal path is a second thing to get wrong',
  )
  ok(
    /aria-haspopup="menu"/.test(strip_) && /aria-expanded=\{overflow\.isOpen\}/.test(strip_),
    'the button announces that it opens a menu, and whether it is open',
  )
  ok(
    /aria-label=\{overflowHint\(hidden\.length\)\}/.test(strip_)
      && /title=\{overflowHint\(hidden\.length\)\}/.test(strip_),
    'the count is in the accessible name and the tooltip, where it cannot change the button’s '
      + 'width and so cannot change what is clipped',
  )
  ok(
    /overflow\.openFor\(el\)/.test(strip_),
    'and the menu is anchored under the button rather than at a stale pointer position',
  )

  /*
   * It only ACTIVATES. `OverflowActions` has one member, so there is no field for a close to
   * arrive through — this pins the call site as well, because the enforcement is worth nothing if
   * a future edit copies `tabMenuEntries`'s five-field actions bag over it.
   */
  const call = /overflowEntries\(([\s\S]{0,160}?)\)\s*,?\s*\}\)/.exec(strip_)?.[1] ?? ''
  ok(call !== '' && /activate: onActivate/.test(call), 'the list is handed a real activate')
  ok(
    !/close/i.test(call),
    'and nothing resembling a close: this control reaches tabs, it never destroys one',
  )

  // =====================================================================================
  // 8. The stylesheet — and what it must NOT have gained
  // =====================================================================================

  ok(
    /\.overflowSlot \{[^}]*flex: none/.test(css)
      && /\.overflowSlot \{[^}]*width: var\(--tab-overflow-w\)/.test(css),
    'the slot is a fixed, non-shrinking reserve, so the tabs box’s width means the same thing '
      + 'whether or not the button is showing — without that the detector oscillates at the '
      + 'boundary, because showing the control clips one more tab',
  )
  ok(
    /\.overflow \[?/.test(css) || /\.overflow \{/.test(css),
    'and the button inside it is styled',
  )
  ok(
    /\.overflow\[aria-expanded='true'\]/.test(css),
    'the lit state is driven off `aria-expanded`, so there is no second open-flag to disagree '
      + 'with the one assistive tech reads',
  )
  /*
   * The tabs are NOT the space this came out of, and that is a requirement rather than taste.
   * `.tab { flex: none }` is what keeps the 12/13px paddings `layoutAudit` measures, and a
   * horizontal reserve taken out of the element being reserved against was rejected in this app
   * once already (the pane cluster over the find bar, README's M16).
   */
  const tabsBlock = /\.tabs \{([^}]*)\}/.exec(css)?.[1] ?? ''
  const tabBlock = /\.tab \{([^}]*)\}/.exec(css)?.[1] ?? ''
  ok(tabsBlock !== '' && tabBlock !== '', 'the two blocks this rule depends on are still there')
  ok(
    /min-width: 0/.test(tabsBlock) && /overflow: hidden/.test(tabsBlock),
    '`.tabs` still clips, which is the fact the whole feature is about',
  )
  ok(/flex: none/.test(tabBlock), '`.tab` still refuses to shrink')
  ok(
    !/(^|[^-a-z])width:/.test(tabBlock) && !/flex-basis/.test(tabBlock),
    'and no tab was given a width or a basis to squeeze it into the reserve',
  )
  ok(
    !/flex-basis/.test(tabsBlock),
    'nor was the tabs box, which would stop it tracking the window',
  )
  /*
   * A different token from the awaiting reserve, on purpose. `check-awaiting.mjs` counts
   * `--awaiting-w`'s readers in this exact stylesheet and expects three; borrowing it here would
   * break an unrelated gate for a reason nobody reading either file could guess.
   */
  ok(
    !/--awaiting-w/.test(/\.overflowSlot \{([^}]*)\}/.exec(css)?.[1] ?? ''),
    'the reserve has a token of its own rather than borrowing the awaiting chip’s',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-tab-overflow: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-tab-overflow: ok')
