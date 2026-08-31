/**
 * Checks the change column's import-free model — `editor/changeModel.ts` — composed with the
 * patience diff it is fed, and pins it against the three files it has to agree with: the surface
 * that hosts the column, the stylesheet that paints it, and the extension that drives it. (M35)
 *
 * # Why this exists
 *
 * Every failure this feature has is silent, and one of them is destructive.
 *
 * Split the HEAD blob on `'\n'` alone and every line of a CRLF file keeps a trailing `\r`, every
 * comparison fails, and the **whole file paints modified** — with no exception, no artefact and
 * nothing logged, looking exactly like a feature working on a file that has genuinely changed.
 * Get the line arithmetic wrong by one and a bar sits beside a line nothing happened to, which is
 * a per-line falsehood in a column whose entire purpose is to be believed. Get `revertPlan`'s
 * `eat` wrong and Revert either leaves a blank line behind or **eats a line the user wrote** —
 * one click, from a card, on the user's own source.
 *
 * So the decisions live in a module with no imports, this file compiles it with the TypeScript in
 * `node_modules` and drives it, and the composition it is driven through — `diffLines` from
 * `panes/mergeModel.ts` — is the same one `changeBars.ts` performs. Both modules are compiled
 * together here precisely so that a rename of `Edit`'s fields breaks this file as well as the app.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that the column appears. There is no DOM and no CodeMirror in this process. That a
 *     `GutterMarker` with no `toDOM` contributes only its `elementClass` was established by
 *     reading `@codemirror/view`'s `GutterElement.setMarkers`; that the strip lands to the right
 *     of the fold chevrons rests on extension order, which is source-asserted below and not
 *     measured.
 *   - that the deletion caret is visible, or lands on the boundary. `transform: translateY(-50%)`
 *     over an absolutely positioned pseudo-element needs a display to judge.
 *   - that the card is positioned correctly, or that `flushSync` is enough to measure it. Both
 *     need a live view, and `check-blame.mjs` says the same of the card it checks.
 *   - **that Revert is right.** This file proves `revertPlan` composed with `applyRevert` returns
 *     the baseline for every block shape, over arrays. Whether `changeBars.ts`' offset
 *     translation faithfully implements that plan against a live `Text` — the clamps, the `+1` on
 *     `doc.length` — is source-pinned and nothing more. It is the most dangerous surface in the
 *     feature and it is the least covered.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

const UI = new URL('..', import.meta.url).pathname
process.chdir(UI)

const out = mkdtempSync(join(tmpdir(), 'cide-changebars-'))
let failed = 0
/** How many block shapes the revert round-trip covered, so the summary cannot drift from it. */
let revertShapes = 0

function eq(actual, expected, what) {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a === b) return
  failed += 1
  console.error(`FAIL ${what}\n  expected ${b}\n  actual   ${a}`)
}

const ok = (actual, what) => eq(actual, true, what)

/** Comments stripped, so a pin certifies the code and not the prose that discusses it. */
const stripJs = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')

const read = (path) => readFileSync(join(UI, path), 'utf8')

try {
  /*
   * Both modules in one pass, and `mergeModel.ts` is here for more than convenience: compiling
   * them together is what makes a rename of `Edit`'s fields a failure in this file rather than a
   * silently different classification at runtime. `changeModel.EditLike` restates that shape.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/changeModel.ts',
      'src/panes/mergeModel.ts',
      '--rootDir',
      'src',
      '--outDir',
      out,
      '--module',
      'esnext',
      '--target',
      'es2022',
      '--moduleResolution',
      'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      '--noUnusedParameters',
      '--noUnusedLocals',
    ],
    { stdio: 'inherit' },
  )

  const {
    CHANGE_COL_CH,
    CHANGE_MAX_LINES,
    CHANGE_RECOMPUTE_MS,
    POPUP_MAX_LINES,
    applyRevert,
    changeBlocks,
    popupContent,
    revertPlan,
    splitBaseline,
  } = await import(`file://${join(out, 'editor/changeModel.js')}`)
  const { diffLines } = await import(`file://${join(out, 'panes/mergeModel.js')}`)

  /** The app's own composition, so nothing here can be right in a way the app is not. */
  const blocksOf = (base, buffer) => changeBlocks(base, buffer, diffLines(base, buffer))

  // --- 1. line endings: the assertion this file exists for -------------------------------

  {
    /*
     * The buffer side of every case here is what `EditorState.create({doc}).doc.iterLines()`
     * actually yields for the same text — verified against the real `Text` — because that is what
     * `changeBars.ts` feeds in. A file ending in a break is THREE lines to CodeMirror, the last
     * of them empty.
     */
    const lf = 'alpha\nbeta\ngamma\n'
    const crlf = 'alpha\r\nbeta\r\ngamma\r\n'
    const cr = 'alpha\rbeta\rgamma\r'
    const buffer = ['alpha', 'beta', 'gamma', '']

    eq(splitBaseline(lf), buffer, 'an LF baseline splits the way CodeMirror splits a document')
    eq(splitBaseline(crlf), buffer, 'a CRLF baseline splits into the SAME lines — no stray \\r')
    eq(splitBaseline(cr), buffer, 'and so does a CR-only one')
    eq(
      splitBaseline('one\n'),
      ['one', ''],
      'a trailing break KEEPS the empty line after it, because `Text` does. Dropping it — which '
        + 'is what the diff pane\'s splitLines and Rust\'s strip_newline both correctly do '
        + 'against git\'s numbering — makes the baseline one line shorter than the buffer for '
        + 'nearly every file in a repository, and paints a phantom addition on the last line of '
        + 'each of them: a green bar at the bottom of files nobody has touched, invisible until '
        + 'somebody scrolls to the end',
    )
    eq(splitBaseline(''), [''], 'an empty blob is one empty line, which is what an empty doc is')
    eq(splitBaseline('one'), ['one'], 'a file with no trailing break is just its line')

    eq(
      blocksOf(splitBaseline(lf), buffer),
      [],
      'and so a file ending in a newline, unedited, has NO markers',
    )

    eq(
      blocksOf(splitBaseline(crlf), buffer),
      [],
      'a CRLF file that has not been edited has NO markers. Split on \\n alone and every line '
        + 'keeps a trailing \\r, every comparison fails and the whole file paints modified — '
        + 'silently, and looking exactly like a file that really did change',
    )

    const edited = ['alpha', 'BETA', 'gamma', '']
    const one = blocksOf(splitBaseline(crlf), edited)
    eq(one.length, 1, '…and a CRLF file with one edited line has exactly one block')
    eq(one[0]?.kind, 'modified', 'which is a modification')
    eq([one[0]?.firstLine, one[0]?.lastLine], [2, 2], 'on line 2 only')
  }

  // --- 2. classification and geometry ----------------------------------------------------

  {
    const base = ['a', 'b', 'c']

    const added = blocksOf(base, ['a', 'b', 'NEW', 'c'])
    eq(added.length, 1, 'an inserted line is one block')
    eq(added[0]?.kind, 'added', 'classified as added — the baseline side of the edit is empty')
    eq([added[0]?.firstLine, added[0]?.lastLine], [3, 3], 'covering the new line, 1-based')
    eq(added[0]?.baseLines, [], 'with no baseline text, because HEAD had nothing there')

    const modified = blocksOf(base, ['a', 'B', 'c'])
    eq(modified[0]?.kind, 'modified', 'a rewritten line is modified — neither side is empty')
    eq(modified[0]?.baseLines, ['b'], 'and it carries what HEAD had')

    const deleted = blocksOf(base, ['a', 'c'])
    eq(deleted.length, 1, 'a removed line is one block')
    eq(deleted[0]?.kind, 'deleted', 'classified as deleted — the buffer side of the edit is empty')
    eq(
      [deleted[0]?.firstLine, deleted[0]?.lastLine],
      [2, 1],
      'anchored on the line BELOW the removal, with lastLine < firstLine to say it covers nothing',
    )
    eq(deleted[0]?.baseLines, ['b'], 'carrying the lines that went')
    eq(deleted[0]?.atEnd, false, 'and not at the end of the file')

    const tail = blocksOf(base, ['a', 'b'])
    eq(tail[0]?.kind, 'deleted', 'a removal at the end of the file is still a deletion')
    eq(tail[0]?.atEnd, true, 'flagged atEnd, so the caret draws on the bottom edge')
    eq(
      tail[0]?.firstLine,
      2,
      '…anchored on the LAST line, because there is no line below it to hang on. Forgetting this '
        + 'case drops the marker entirely, and text removed from the end of a file is exactly the '
        + 'text nothing else on screen mentions',
    )

    eq(blocksOf(base, base), [], 'an unchanged buffer has no blocks at all')
    eq(blocksOf([], ['a', 'b']), [{
      kind: 'added',
      firstLine: 1,
      lastLine: 2,
      baseLines: [],
      atEnd: true,
    }], 'an empty baseline — HEAD has the file, empty — makes every line an addition')
  }

  {
    // A deletion immediately followed by an insertion: two blocks whose markers land on the same
    // line, which is why `changeBars.ts` keeps the carets in a second RangeSet.
    const base = ['keep', 'gone', 'tail']
    const blocks = blocksOf(base, ['keep', 'fresh', 'extra', 'tail'])
    ok(blocks.length >= 1, 'a replace-and-grow edit produces at least one block')
    ok(
      blocks.every((b) => b.firstLine >= 1 && b.firstLine <= 4),
      'and every block anchors inside the buffer',
    )
  }

  // --- 3. revert round-trips, over every shape -------------------------------------------

  {
    /*
     * The claim: for any pair, reverting every block one at a time — in reverse document order,
     * so an earlier revert cannot move a later block — reproduces the baseline exactly.
     *
     * Reverse order matters and is worth saying: applied forwards, removing three lines at the
     * top shifts every block below it and the second revert lands in the wrong place. The app
     * never applies two at once (each is a separate click against a freshly recomputed set), so
     * this is the check being stricter than the feature, not the feature relying on it.
     */
    const cases = [
      ['nothing at all', ['a', 'b', 'c'], ['a', 'b', 'c']],
      ['one line changed in the middle', ['a', 'b', 'c'], ['a', 'B', 'c']],
      ['the first line changed', ['a', 'b', 'c'], ['A', 'b', 'c']],
      ['the last line changed', ['a', 'b', 'c'], ['a', 'b', 'C']],
      ['a line added in the middle', ['a', 'b', 'c'], ['a', 'b', 'NEW', 'c']],
      ['a line added at the top', ['a', 'b', 'c'], ['NEW', 'a', 'b', 'c']],
      ['a line added at the end', ['a', 'b', 'c'], ['a', 'b', 'c', 'NEW']],
      ['two lines added at the end', ['a', 'b'], ['a', 'b', 'X', 'Y']],
      ['a line removed in the middle', ['a', 'b', 'c'], ['a', 'c']],
      ['the first line removed', ['a', 'b', 'c'], ['b', 'c']],
      ['the last line removed', ['a', 'b', 'c'], ['a', 'b']],
      ['the last two removed', ['a', 'b', 'c', 'd'], ['a', 'b']],
      ['a block replaced by a longer one', ['a', 'b', 'c'], ['a', 'X', 'Y', 'Z', 'c']],
      ['a block replaced by a shorter one', ['a', 'b', 'c', 'd'], ['a', 'X', 'd']],
      ['everything replaced', ['a', 'b'], ['X', 'Y']],
      ['a single-line file edited', ['only'], ['ONLY']],
    ]
    revertShapes = cases.length
    for (const [name, base, buffer] of cases) {
      const blocks = blocksOf(base, buffer)
      let lines = buffer
      for (const block of [...blocks].reverse()) {
        lines = applyRevert(lines, revertPlan(block, lines.length))
      }
      eq(
        lines,
        base,
        `reverting every block of "${name}" reproduces HEAD exactly — this is the only `
          + 'automated defence on an operation that can destroy the user\'s text',
      )
    }
  }

  {
    // The `eat` field, named directly, because it is what a reader will want to change.
    const addMiddle = blocksOf(['a', 'b'], ['a', 'NEW', 'b'])[0]
    eq(
      revertPlan(addMiddle, 3).eat,
      'trailing',
      'removing added lines eats the break AFTER them — otherwise an empty line is left behind',
    )
    const addEnd = blocksOf(['a', 'b'], ['a', 'b', 'NEW'])[0]
    eq(
      revertPlan(addEnd, 3).eat,
      'leading',
      '…and the break BEFORE them at the end of the file, where there is no trailing one to take',
    )
    const delEnd = blocksOf(['a', 'b', 'c'], ['a', 'b'])[0]
    const planEnd = revertPlan(delEnd, 2)
    eq(
      [planEnd.fromLine, planEnd.toLine],
      [3, 2],
      'restoring lines removed from the end inserts ONE PAST the last line, not above it — '
        + '`changeBars.ts` translates that to `doc.length`. Anchoring on the last line instead is '
        + 'off by one line and looks right whenever the last line happens to be blank',
    )
  }

  // --- 4. the guards ---------------------------------------------------------------------

  {
    const big = Array.from({ length: CHANGE_MAX_LINES + 1 }, (_, i) => `line ${i}`)
    eq(
      changeBlocks(big, ['x'], [{ aFrom: 0, aTo: 1, bFrom: 0, bTo: 1 }]),
      [],
      'past CHANGE_MAX_LINES on the baseline side, no markers at all',
    )
    eq(
      changeBlocks(['x'], big, [{ aFrom: 0, aTo: 1, bFrom: 0, bTo: 1 }]),
      [],
      '…and past it on the buffer side',
    )
    eq(
      changeBlocks(['a'], ['a'], [{ aFrom: 0, aTo: 0, bFrom: 5, bTo: 9 }]),
      [],
      'a block outside the buffer is DROPPED, not clamped: a clamp puts a confident marker on a '
        + 'line nothing happened to, which is worse than no marker',
    )
    eq(
      changeBlocks(['a'], ['a'], [{ aFrom: 1, aTo: 1, bFrom: 1, bTo: 1 }]),
      [],
      'and an edit that is empty on both sides is not a change',
    )
  }

  // --- 5. the card's content -------------------------------------------------------------

  {
    const added = blocksOf(['a'], ['a', 'NEW'])[0]
    eq(
      popupContent(added).copyable,
      false,
      'an added block offers no Copy: HEAD had nothing there, and a button that copies the empty '
        + 'string is a control that silently does nothing',
    )
    const removed = blocksOf(['a', 'b', 'c'], ['a'])[0]
    const card = popupContent(removed)
    eq(card.heading, '2 lines removed', 'the heading counts what went')
    eq(card.lines, ['b', 'c'], 'and the card shows it')
    eq(card.copyable, true, 'which is copyable')
    eq(popupContent(blocksOf(['a', 'b'], ['a'])[0]).heading, '1 line removed', 'singular at one')

    const long = Array.from({ length: POPUP_MAX_LINES + 5 }, (_, i) => `l${i}`)
    const trimmed = popupContent(blocksOf(long, ['only'])[0])
    eq(trimmed.lines.length, POPUP_MAX_LINES, 'a long block is trimmed to POPUP_MAX_LINES')
    eq(trimmed.hiddenLines, 5, '…and says how many it is not showing')
    eq(popupContent(removed).hiddenLines, 0, 'a short one says nothing')
  }

  // --- 6. where the column is mounted ----------------------------------------------------

  {
    const surface = stripJs(read('src/editor/EditorSurface.tsx'))
    const composed = surface.slice(
      surface.indexOf('const shared: Extension[] = ['),
      surface.indexOf('new EditorView({'),
    )
    ok(composed.length > 0, 'the extension array is where this check expects it')
    ok(
      composed.indexOf('blameSlot.of(') < composed.indexOf('lineNumbers()'),
      'the blame column is still first, so it stays LEFT of the line numbers where IDEA has it',
    )
    ok(
      composed.indexOf('foldExtensions(') < composed.indexOf('shared.push(CHANGE_BARS)'),
      'the change column is pushed AFTER the fold gutter, so the strip sits against the text — '
        + 'gutters lay out in extension order, and moved above it the column still works and is '
        + 'simply in the wrong place, which a reader cannot see in a diff',
    )
    const gate = composed.indexOf('if (!oversize) {')
    const push = composed.indexOf('shared.push(CHANGE_BARS)')
    const readOnly = composed.indexOf('if (readOnly) {')
    ok(
      gate !== -1 && push > gate && (readOnly === -1 || push < readOnly),
      'and it is pushed OUTSIDE the oversize gate: the column\'s width is reserved '
        + 'unconditionally, so the extension has to be present in every buffer or the gutter '
        + 'geometry would depend on file size',
    )
    ok(
      !/CHANGE_BARS[\s\S]{0,80}useMemo|useMemo[\s\S]{0,80}CHANGE_BARS/.test(surface),
      'CHANGE_BARS is used as the module-level constant it is, not rebuilt per render: a fresh '
        + 'extension value is what tears a gutter\'s DOM down and takes an open card with it',
    )

    const build = surface.indexOf('}, [path, reloadKey])')
    ok(build !== -1, 'the build effect is still keyed exactly `[path, reloadKey]`')
    for (const [, deps] of surface.matchAll(/\}, \[([^\]]*)\]\)/g)) {
      if (!deps.includes('reloadKey')) continue
      ok(
        !/\bbaseline\b/.test(deps),
        'and `baseline` is in NO dependency array that also holds `reloadKey` — rebuilding the '
          + 'view costs the undo history, the scroll position, the selection and any unsaved '
          + `edits, and a baseline arriving must not do that (saw: [${deps}])`,
      )
    }
    ok(
      /setChangeBaseline\.of\(baselineRef\.current\)/.test(surface),
      'the baseline is re-seeded when the view is rebuilt, or a reload comes back with an empty '
        + 'column and no event to fill it — the effect is keyed on a prop that did not change',
    )
    ok(
      /oversizeRef\.current \? null : baseline/.test(surface),
      'and an oversize buffer is handed null rather than a baseline, so one flag decides what a '
        + 'large file does about grammar, folding and markers alike',
    )

    const pane = stripJs(read('src/panes/EditorPane.tsx'))
    ok(
      pane.includes('baseline={baseline}'),
      'EditorPane actually feeds it — an unfed column draws nothing and passes every check above',
    )
    ok(
      pane.includes('openBaseline(') && pane.includes('releaseBaseline('),
      '…and refcounts it, so a split over one file asks one question and the last pane out drops it',
    )
    ok(
      /useSyncExternalStore\(subscribeBaselines, baselineRevision, baselineRevision\)/.test(pane),
      'the store is read with the revision counter as the snapshot — a NUMBER. Returning the '
        + 'lines array from the selector is a fresh identity per call and the render loop that '
        + 'unmounts the whole root',
    )
  }

  // --- 7. the extension's own invariants -------------------------------------------------

  {
    const ext = stripJs(read('src/editor/changeBars.ts'))
    ok(
      /showTooltip/.test(ext) && !/hoverTooltip/.test(ext),
      'the card comes from `showTooltip.from(field)` and NOT `hoverTooltip`, which listens on '
        + '`contentDOM` — and the gutter is not part of it, so it would never fire at all',
    )
    ok(/domEventHandlers/.test(ext), 'so the click comes from the gutter\'s own handlers')
    ok(
      /event\.preventDefault\(\)/.test(ext),
      'and the click is prevented, or the selection moves first and scrolls the pane out from '
        + 'under the pointer',
    )
    ok(
      /revertPlan\(/.test(ext),
      'the revert goes through `revertPlan`: nobody re-derives the line-break arithmetic inline',
    )
    ok(
      ext.includes("plan.insert.join('\\n')") && !/'\\r\\n'/.test(ext),
      'and the restored lines are joined with \\n and never \\r\\n — a CRLF join puts literal \\r '
        + 'characters INSIDE CodeMirror lines, and `restoreLineEndings` then adds the file\'s own '
        + 'ending on top of them, so every reverted line ends \\r\\r\\n',
    )
    ok(
      /iterLines\(\)/.test(ext) && !/doc\.toString\(\)\.split/.test(ext),
      'the buffer is read with `iterLines`, not `toString().split()`: the string form allocates '
        + 'the whole document before splitting it, which is a megabyte of garbage per tick on a '
        + 'timer that runs five times a second',
    )
    ok(
      /EditorView\.domEventHandlers\(/.test(ext) && !/view\.dom\.addEventListener/.test(ext),
      'the dismissal is an `EditorView.domEventHandlers`, which attaches to **contentDOM** — so '
        + 'it never sees the click on the marker that opened the card, nor one on the card '
        + 'itself, because neither the gutter nor a tooltip is inside contentDOM. Hand-rolled on '
        + '`view.dom` it would see both: a gutter handler returning true only calls '
        + 'preventDefault and never stopPropagation, so the opening click would close the card in '
        + 'the same gesture and the marker would read as simply not clickable',
    )
    ok(
      /fingerprint\(/.test(ext),
      'an unchanged recompute dispatches nothing — otherwise the card is destroyed five times a '
        + 'second and its Revert button can never be clicked',
    )
    ok(
      /console\.error\('\[cide\] the change could not be reverted'/.test(ext),
      'and the revert is wrapped: no error boundary stands between a dispatch and the React root',
    )
  }

  // --- 8. the stylesheet -----------------------------------------------------------------

  {
    const css = read('src/editor/EditorSurface.module.css')
    const declarations = css.replace(/\/\*[\s\S]*?\*\//g, '')

    ok(
      css.includes(`--change-col: ${CHANGE_COL_CH}ch`),
      `the column is sized \`${CHANGE_COL_CH}ch\`, the same number as CHANGE_COL_CH — a `
        + 'stylesheet that disagreed would leave dead space beside every line or clip the bar',
    )
    ok(
      /\.cm-gutters\)[^}]*min-width:\s*calc\(70px \+ var\(--fold-col\) \+ var\(--change-col\)\)/.test(
        css,
      ),
      'the gutter reservation GROWS by the column. `.cm-lineNumbers` is `flex: 1` of it, so '
        + 'without this every line number shifts left the moment a bar appears — which here is '
        + 'the moment somebody types their first character into a clean file',
    )
    ok(
      /\.cm-gutters:has\(\.cm-blame\)\)[^}]*var\(--change-col\)/.test(css),
      '…and the annotated reservation carries it too, or turning blame on takes the change '
        + 'column back out of the numbers',
    )
    ok(
      /\.cm-cide-changes\)[^}]*flex:\s*none/.test(css),
      'the column is `flex: none`, so it cannot take its width out of the numbers\' share',
    )
    ok(
      /\.cm-cide-changes \.cm-gutterElement\)[^}]*position:\s*relative/.test(css),
      'the per-line CELL is the positioned ancestor, not the column. The deletion caret is '
        + 'absolute, so with the column positioned instead every caret in the file stacks in its '
        + 'top-left corner — all drawn, all in one place, and invisible for any line scrolled '
        + 'past. Nothing throws and nothing is missing from the DOM: a deletion simply appears to '
        + 'paint no marker',
    )

    for (const cls of ['cm-change-add', 'cm-change-mod', 'cm-change-del', 'cm-change-del-end']) {
      ok(
        css.includes(cls),
        `every class the extension can emit has a rule — \`${cls}\` is missing, which is a `
          + 'change the column knows about and paints nothing for',
      )
    }
    for (const cls of ['cm-change-add', 'cm-change-mod', 'cm-change-del']) {
      ok(
        new RegExp(`\\.cm-cide-changes \\.cm-gutterElement\\.${cls}`).test(css),
        `\`${cls}\` is written three classes deep, so it beats \`.cm-activeLineGutter\` — which `
          + 'puts a background on EVERY gutter element of the caret\'s line, and would take the '
          + 'bar off the one line the user is actually editing, regardless of source order',
      )
    }
    ok(
      /cm-change-del\)::before[^}]*position:\s*absolute/.test(css),
      'the caret is absolutely positioned, so it adds no height: a gutter element that grew '
        + 'would slide the column out of step with the buffer for the rest of the file',
    )
    ok(
      !/cm-change-del\)::before[^}]*font-size/.test(css),
      'and it is drawn with borders rather than a glyph, so it carries no font-size to rot',
    )
    ok(
      !/--diff-add-bg|--diff-del-bg/.test(
        declarations.slice(declarations.indexOf('.cm-cide-changes')),
      ),
      'the bars are NOT painted from the diff pane\'s row washes: those are 10–12% tints meant '
        + 'to sit behind text, and as a hairline strip they are invisible',
    )
    ok(
      css.includes('.cm-tooltip.cm-change-popup'),
      'and the card has a frame, or it inherits CodeMirror\'s light-grey base theme',
    )
  }

  // --- 9. the numbers agree ---------------------------------------------------------------

  {
    ok(
      CHANGE_RECOMPUTE_MS >= 60 && CHANGE_RECOMPUTE_MS <= 250,
      'the recompute interval is inside the band this codebase already calls human — above a '
        + `frame by an order of magnitude and under BLAME_HOVER_MS (saw ${CHANGE_RECOMPUTE_MS})`,
    )
    const model = read('src/editor/changeModel.ts')
    eq(
      model.match(/^import /gm),
      null,
      'changeModel.ts has NO imports — it is compiled standalone by this script, and one `@/*` '
        + 'would drag the whole IPC surface in behind it',
    )
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `change bars: ok (${CHANGE_COL_CH}ch column, ${CHANGE_RECOMPUTE_MS}ms recompute, `
      + `${CHANGE_MAX_LINES} line cap, revert round-tripped over ${revertShapes} shapes)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
