/**
 * Checks `src/chrome/tabDrag.ts` — what a tab drag picks up, where the caret lands, and what a
 * drop actually commits.
 *
 * > *"drag to reorder"*
 *
 * The rules live in a pure module and this script runs them, because every one of them has a
 * wrong version that type-checks and is invisible in a screenshot. Two in particular:
 *
 * * the **boundary arithmetic**, where the off-by-one makes every rightward drag a no-op while
 *   every leftward drag works perfectly — the sort of half-working that survives a manual test;
 * * the **pinned console**, which `cide_core::workspace::reorder_tab` refuses in both directions.
 *   The frontend's job is to make that refusal unreachable, because a drop that bounces back is
 *   worse than a gesture that never starts.
 *
 * The tail reads `TabStrip.tsx`, `useTabDrag.ts`, `App.tsx` and the stylesheet as *source*,
 * because the wiring cannot be executed here: there is no DOM and no Tauri. That matters more
 * than usual for this feature — `tab_reorder` did not exist at all until this change, while
 * `cide_core::workspace::reorder_tab` had been complete and covered by three passing tests since
 * M4. Rules that pass with nothing calling them is this project's signature defect, and a drag is
 * exactly the shape that hides it, because no check in this repo can move a pointer.
 *
 * Every source assertion runs against **comment-stripped** text. A grep over raw source matches
 * the feature's own name in the paragraph explaining it, so deleting the code leaves the gate
 * green; the mutation transcript for this file includes exactly that case.
 *
 * Run: `pnpm --dir ui run check:tab-drag`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-tab-drag-'))
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
 * Remove comments before grepping.
 *
 * The reason is at the top of this file, and it is not hypothetical: every module in this feature
 * carries long prose explaining `onReorder`, `data-dragging` and `edgeScroll` by name, so a grep
 * over raw source matches the *explanation* of a feature that has been deleted. The mutation
 * transcript for this check includes exactly that case — the line moved into a comment, the
 * assertion still failing.
 *
 * String literals are deliberately **kept**. They are not prose here, they are the feature: the
 * DOM attribute names the stylesheet keys on (`data-drag`, `data-dragging`) and the key name the
 * Escape handler compares against exist nowhere else. Blanking them would make every assertion
 * about them unwritable, which is a worse failure than the one being guarded against — and
 * comments are where the hazard actually is.
 *
 * Deliberately naive: it does not parse regex literals, which is safe here because none of the
 * files it is applied to contains a regex containing `//` or `/*`. The `[^:]` guard keeps it
 * from eating the rest of a line after a `https://` in a comment-free string.
 */
const strip = (src) =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/tabDrag.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // `tabDrag.ts` imports nothing at all, so plain ESM output loads under node directly —
      // unlike `treeDrag.ts`, which needs CommonJS because it imports four modules beside it.
      // The import-free shape is the point: it is what lets this script compile the file alone.
      '--module', 'esnext',
      '--moduleResolution', 'bundler',
      '--target', 'es2022',
      '--strict',
      // Both flags the app's own tsconfig sets. `noUncheckedIndexedAccess` is what makes
      // `tabs[boundary]` a `DragTab | undefined`, which is the difference between "dropped at the
      // end" and a crash on `.id` of undefined.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
      '--lib', 'es2023',
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const d = await import(`file://${join(out, 'chrome', 'tabDrag.js')}`)
  const { FIRST_BOUNDARY, caretIndex, dropOutcome, grabTab, movable } = d

  // A strip as the app has it: the pinned console, then whatever is open.
  const t = (id, kind) => ({ id, kind: { kind } })
  const console_ = t('t0', 'claudeHome')
  const tabs = [console_, t('t1', 'file'), t('t2', 'claudeFull'), t('t3', 'settings')]

  // =====================================================================================
  // 1. What may be picked up
  // =====================================================================================

  ok(!movable(console_), 'the pinned console cannot be moved')
  eq(grabTab(tabs, console_), null, 'so a press on it never becomes a gesture')
  /*
   * Off the KIND, never off the index. This is the assertion that would have caught the obvious
   * shortcut — `tabs.indexOf(tab) === 0` — which is right today and wrong the moment reordering
   * works, since reordering is the one gesture whose entire purpose is to make the index lie.
   * A console that has somehow been handed a later position is still not movable.
   */
  eq(
    grabTab([t('x', 'file'), console_], console_),
    null,
    'and it is refused by kind, not by position: a console at index 1 is still pinned',
  )
  ok(movable(tabs[1]) && movable(tabs[2]) && movable(tabs[3]), 'everything else moves')
  eq(grabTab(tabs, tabs[2]), { tab: 't2', from: 2 }, 'a grab carries the id and where it started')
  eq(
    grabTab(tabs, t('gone', 'file')),
    null,
    'a tab that is not in this strip cannot be dragged: the strip can change under a press',
  )

  // =====================================================================================
  // 2. The caret — where the drop is *shown* to land
  // =====================================================================================

  eq(FIRST_BOUNDARY, 1, 'boundary 0 is in front of the pinned console and is never offered')

  eq(caretIndex(4, 2, 0.1), 2, 'the left half of a tab means the gap before it')
  eq(caretIndex(4, 2, 0.9), 3, 'the right half means the gap after it')
  eq(caretIndex(4, 2, 0.5), 3, 'the midpoint rounds forward, so there is no dead pixel')
  eq(caretIndex(4, -1, 0), 4, 'past the last tab — the spacer — means the end of the strip')

  /*
   * The clamp, which is the pinned console expressed as a caret.
   *
   * Clamping rather than hiding: a caret that vanishes over the left of the strip is
   * indistinguishable from a drag the app has stopped tracking, and it leaves the user with no
   * idea where a release would put the tab. Clamped, it says exactly where it will go.
   */
  eq(caretIndex(4, 0, 0.1), 1, 'aiming at the left half of the console clamps to boundary 1')
  eq(caretIndex(4, 0, 0.9), 1, 'and so does its right half, which IS boundary 1')
  eq(caretIndex(4, 0, -3), 1, 'and a pointer left of the whole strip, which reads as negative')
  for (let over = -1; over < 4; over++) {
    for (const f of [-2, 0, 0.25, 0.5, 0.75, 1, 4]) {
      const c = caretIndex(4, over, f)
      ok(
        c >= FIRST_BOUNDARY && c <= 4,
        `the caret is always a legal boundary (over=${over} fraction=${f} gave ${c})`,
      )
    }
  }

  // =====================================================================================
  // 3. The drop — what is actually committed
  // =====================================================================================

  const drag = grabTab(tabs, tabs[1]) // t1, from index 1

  /*
   * The two no-ops, and the second is the one worth having a check for.
   *
   * Dropping at the boundary *before* the dragged tab obviously changes nothing. Dropping at the
   * boundary *after* it does too, because removing the tab closes the gap that boundary was
   * addressing. Sending either to Rust is a round trip, a `rev` bump and a broadcast to every
   * window to achieve nothing — and it would make a 5px twitch indistinguishable, in the saved
   * workspace, from a deliberate reorder.
   */
  eq(dropOutcome(tabs, drag, 1), null, 'dropped at its own left edge: nothing to do')
  eq(dropOutcome(tabs, drag, 2), null, 'dropped at its own right edge: also nothing to do')

  eq(
    dropOutcome(tabs, drag, 3),
    { tab: 't1', before: 't3' },
    'dropped one place right, it lands in front of the tab that was two along',
  )
  eq(
    dropOutcome(tabs, drag, 4),
    { tab: 't1', before: null },
    'and dropped past the end, `before` is null — the shape `tab_reorder` reads as "the end"',
  )

  const back = grabTab(tabs, tabs[3]) // t3, from index 3
  eq(
    dropOutcome(tabs, back, 1),
    { tab: 't3', before: 't1' },
    'dragging leftward to the first legal boundary lands in front of the first movable tab',
  )
  eq(dropOutcome(tabs, back, 3), null, 'and its own left edge is still a no-op')

  /*
   * Boundary 0 can never be committed even if something upstream produced it.
   *
   * `caretIndex` clamps, so this is unreachable through the hook — which is exactly why it is
   * asserted here. The clamp is one line in one function; this is the second lock, and it is the
   * one that decides whether a bug in the clamp becomes a rejected IPC call the user sees as a
   * tab snapping back, or is simply never sent.
   */
  eq(dropOutcome(tabs, drag, 0), null, 'a drop in front of the pinned console commits nothing')
  eq(dropOutcome(tabs, drag, 9), null, 'and neither does one past the end of the strip')

  /*
   * The round trip: what the caret shows is what Rust is asked for.
   *
   * Driven over every (source, boundary) pair by *simulating* `reorder_tab` — remove then insert,
   * with the same `to = boundary > from ? boundary - 1 : boundary` conversion the Rust command
   * performs — and asserting the dragged tab really ends up in front of the tab the caret named.
   * This is the assertion that fails if either side's off-by-one is fixed without the other.
   */
  for (let from = 1; from < tabs.length; from++) {
    const d = grabTab(tabs, tabs[from])
    for (let boundary = FIRST_BOUNDARY; boundary <= tabs.length; boundary++) {
      const outcome = dropOutcome(tabs, d, boundary)
      if (outcome === null) continue
      const ids = tabs.map((x) => x.id)
      const to = boundary > from ? boundary - 1 : boundary
      const moved = ids.slice()
      moved.splice(from, 1)
      moved.splice(to, 0, ids[from])
      const landed = moved.indexOf(ids[from])
      const after = moved[landed + 1] ?? null
      eq(
        after,
        outcome.before,
        `from ${from} to boundary ${boundary}: the tab ends up immediately before the tab the `
          + `caret named (${JSON.stringify(moved)})`,
      )
      ok(landed > 0, `and never in front of the pinned console (from ${from}, boundary ${boundary})`)
      eq(moved[0], 't0', `and the console stays at index 0 (from ${from}, boundary ${boundary})`)
    }
  }

  // =====================================================================================
  // 4. The wiring, as source
  // =====================================================================================

  const strip_ = strip(readFileSync('src/chrome/TabStrip.tsx', 'utf8'))
  const hook = strip(readFileSync('src/chrome/useTabDrag.ts', 'utf8'))
  const app = strip(readFileSync('src/App.tsx', 'utf8'))
  const client = strip(readFileSync('src/ipc/client.ts', 'utf8'))
  const store = strip(readFileSync('src/store/workspace.ts', 'utf8'))
  // Stripped like the rest, and this file is the one where it is least optional: a stylesheet is
  // nothing *but* `/* … */`, and the paragraphs below `.caret` and `.tab[data-drag]` explain the
  // very selectors these assertions look for. Delete the rules and leave the prose and a raw
  // grep still passes.
  const css = strip(readFileSync('src/chrome/TabStrip.module.css', 'utf8'))

  /*
   * The comment stripper has to actually work, or every assertion below is decorative — it would
   * pass against a feature that had been reduced to the paragraph describing it.
   */
  ok(
    !strip('const x = 1 /* onReorder */\nconst y = 2 // onReorder').includes('onReorder'),
    'the stripper removes block comments and line comments',
  )
  ok(
    strip('const onReorder = "data-drag"').includes('onReorder')
      && strip('const onReorder = "data-drag"').includes('data-drag'),
    'and leaves real code — including the DOM attribute names — alone',
  )

  /*
   * The rules above are dead if nothing calls them. `tab_reorder` did not exist until this
   * change while `reorder_tab` did, so this feature spent several milestones as a complete,
   * correct, unreachable implementation — the exact defect these greps exist to catch.
   */
  ok(
    /const drag = useTabDrag\(\{/.test(strip_) && /onReorder:/.test(strip_),
    'the strip builds the drag AND passes `onReorder` through — without it the hook refuses to '
      + 'start a gesture, which is deliberate: a drag that cannot land is the silent failure '
      + 'dressed as the feature',
  )
  ok(
    /onPointerDown=\{\(e\) => onPick\(e, tab\)\}/.test(strip_) && /onPick=\{drag\.onPointerDown\}/.test(strip_),
    'and every tab row carries the press that can become one',
  )
  ok(
    /onReorder=\{\(id, before\) => void reorderTab\(activeProject\.id, id, before\)\}/.test(app),
    'App.tsx hands the strip a real reorder — the one call site, and the whole difference '
      + 'between a feature and a module nobody reaches',
  )
  ok(
    /reorder: \(projectId: ProjectId, id: TabId, before: TabId \| null\) =>[\s\S]{0,140}?invoke<\{ rev: number \}>\('tab_reorder', \{ project: projectId, tab: id, before \}\)/.test(
      client,
    ),
    'the client seam calls `tab_reorder`, and takes ids rather than indices — an index computed '
      + 'at pointer-move time names a different tab by the time the drop lands',
  )
  ok(
    /reorderTab: async \(project, tab, before\) => \{[\s\S]{0,200}?await tabApi\.reorder\(project, tab, before\)[\s\S]{0,120}?await synced\(rev\)/.test(
      store,
    ),
    'and the store waits for the snapshot the reorder broadcast, or the strip paints the old '
      + 'order until something else moves the mirror — `synced(rev)` is the wait, and the '
      + 'broadcast out of `WorkspaceState::update` is the repaint',
  )

  // The click guard. Without it a drop ALSO activates the tab it just moved, yanking the user
  // out of whatever they were reading as a side effect of a gesture about order alone.
  ok(
    /if \(dragged\(\)\) return/.test(strip_) && /dragged=\{drag\.dragged\}/.test(strip_),
    'a drop does not also activate the tab it moved',
  )

  // ...and the guard is ONE-SHOT: reading it clears it.
  //
  // It was cleared only in `onPointerDown`, which every click has and no keypress does. The drag
  // deliberately does not `preventDefault()` the press, so focus stays on the dragged tab; the
  // user then presses Enter, `click` fires with no `pointerdown` before it, and the latch left
  // over from the drop ate the activation — permanently, until a pointer press happened to land
  // on a tab again. A keyboard user who dragged once could not activate a tab by keyboard at all.
  //
  // Asserted on the source of `dragged` rather than by driving it, because the latch lives in a
  // hook this harness cannot mount. The shape is specific enough to be worth pinning: it must
  // read the ref, write false back, and return what it read.
  const body = /const dragged = useCallback\(\(\) => \{([\s\S]*?)\}, \[\]\)/.exec(hook)?.[1] ?? ''
  ok(body !== '', '`dragged` is a callback with a body, not a bare ref read')
  ok(
    /wasDrag\.current = false/.test(body),
    '`dragged()` clears the latch as it reads it — a keypress has no `pointerdown` to clear it',
  )
  // BOTH clears, and they answer different questions — which is why this is two assertions and
  // not one. `onPointerDown` clears so a press that returns early (a ctrl+click, the middle
  // button) cannot leave the *previous* drag's verdict standing for the click that follows it;
  // `dragged()` clears so an activation with no press before it at all is not eaten. Deleting
  // either one reintroduces a different swallowed gesture.
  ok(
    /wasDrag\.current = false/.test(hook.replace(body, '')),
    'the pointerdown clear survives too — it guards the press path, which the one-shot does not',
  )
  /*
   * BOTH buttons in the row, and the second is the one that costs something.
   *
   * A press that starts on `×` and becomes a reorder must not close the tab it just moved.
   * Pointer capture normally retargets the `click` to the row so the close button never hears
   * it — but capture can fail (a pointer released before the 4px threshold, an engine that
   * refuses the call), and a guard whose fallback discards the user's tab is not a guard. A
   * count, because one `dragged()` satisfies the assertion above while the other is missing.
   */
  eq(
    (strip_.match(/if \(dragged\(\)\) return/g) ?? []).length,
    2,
    'and neither the label NOR the close button acts on the click that ends a drag',
  )
  /*
   * `wasDrag` is cleared above the button/modifier guard, not below it.
   *
   * `dragged()` is read from a `click` fired after the *next* press, so a press that returns
   * early without clearing it leaves the previous drag's verdict standing. Below the guard, the
   * first ctrl+click on any tab after any drag is swallowed by a rule about a gesture that had
   * already finished — and it stays swallowed until some unmodified press resets it. An
   * ordering assertion because both lines are present either way.
   */
  ok(
    hook.indexOf('wasDrag.current = false') > 0
      && hook.indexOf('wasDrag.current = false') < hook.indexOf('e.button !== 0'),
    'the click/drag verdict is cleared BEFORE the press is filtered, or a modified press after '
      + 'a drag inherits that drag’s verdict and is swallowed',
  )
  ok(
    /if \(drag\.state !== null\) return/.test(strip_),
    'and a right-click mid-drag does not open a menu over a caret the user is still aiming',
  )

  // The three feedback channels.
  ok(
    /'data-dragging': ''/.test(strip_) && /'data-drag': ''/.test(strip_),
    'the strip says a drag is in flight and marks the tab being moved',
  )
  ok(
    /className=\{styles\.caret\}/.test(strip_) && /left: `\$\{drag\.state\.caretX\}px`/.test(strip_),
    'and the caret is drawn where the hook measured it',
  )
  ok(
    /\.caret \{/.test(css) && /\[data-drag\]/.test(css) && /\[data-dragging\]/.test(css),
    'and the stylesheet actually draws all three',
  )
  /*
   * The caret must not be a flex child and `.tabs` must be positioned, or `offsetLeft` measures
   * from the window and the caret lands somewhere in the header. Pinned as the pair, because
   * either alone is silently wrong.
   */
  ok(
    /\.caret \{[^}]*position: absolute/.test(css) && /\.tabs \{[^}]*position: relative/.test(css),
    'the caret is absolutely positioned inside a positioned tablist — a flex child would shift '
      + 'every tab after it by 2px, which the layout audit measures',
  )
  ok(
    /\.strip\[data-dragging\] \.tab:not\(\.tabActive\):hover/.test(css),
    'the hover wash is suppressed during a drag WITHOUT out-specifying the active tab, which '
      + 'would make it visibly deactivate for the length of every drag',
  )

  // The hook's own contract: the parts a missing line makes silently wrong.
  ok(
    /window\.addEventListener\('pointermove', move\)/.test(hook)
      && /window\.addEventListener\('pointerup', up\)/.test(hook)
      && /window\.addEventListener\('pointercancel', cancel\)/.test(hook),
    'the hook listens on the WINDOW rather than on the row, so the drag survives the pointer '
      + 'leaving the 30px strip — which it does constantly',
  )
  ok(
    /ev\.key !== 'Escape'/.test(hook)
      && /window\.addEventListener\('keydown', key, true\)/.test(hook)
      && /ev\.stopPropagation\(\)/.test(hook),
    '⎋ cancels a drag in flight, in the capture phase and with a stop, so nothing behind the '
      + 'strip also answers it',
  )
  ok(
    /useEffect\(\(\) => \(\) => finish\(false\), \[finish\]\)/.test(hook),
    'an unmount mid-drag takes the window listeners with it',
  )
  ok(
    /if \(live\.current\.onReorder === undefined\) return/.test(hook),
    'the hook refuses to start when there is nothing to land on — which is what makes the '
      + 'chrome audit’s handler-free strip inert by construction rather than by remembering',
  )
  ok(
    /if \(grabTab\(live\.current\.tabs, tab\) === null\) return/.test(hook),
    'and it asks the RULE whether this tab can be picked up, rather than testing the kind itself',
  )
  ok(
    /Math\.abs\(ev\.clientX - active\.from\.x\) > THRESHOLD/.test(hook),
    'a press is not a drag until it has moved, or every click on a tab would be one',
  )
  ok(
    /active\.outcome = dropOutcome\(/.test(hook) && /live\.current\.onReorder\?\.\(outcome\.tab, outcome\.before\)/.test(hook),
    'the verdict is kept in a REF and committed from it — a state updater is replayed under '
      + 'StrictMode, which is the wrong place to fire a workspace mutation from',
  )
  /*
   * `edgeScroll` must NOT be here. `.tabs` is `overflow: hidden` with `flex: none` tabs — the
   * strip clips, it does not scroll — so a copied edge-scroll would be a call on a box whose
   * `scrollTop` is permanently 0: dead code that reads as a working feature.
   */
  ok(
    !/edgeScroll/.test(hook) && !/scrollTop/.test(hook),
    'and no edge auto-scroll was copied in from the other two drags: this strip clips rather '
      + 'than scrolls, so it would be dead code that looks like a feature',
  )
  ok(
    !/preventDefault\(\)/.test(hook.slice(hook.indexOf('const onPointerDown'), hook.indexOf('const move ='))),
    'the press is NOT defaultPrevented: that would stop the tab’s own button ever taking focus, '
      + 'and the focus ring is the tabs’ only keyboard affordance',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-tab-drag: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-tab-drag: ok')
