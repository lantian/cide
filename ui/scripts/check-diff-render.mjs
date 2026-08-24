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
 * # And, since M18, the blame column — model and markup in one script
 *
 * `src/panes/diffBlame.ts` is import-free precisely so the TypeScript in `node_modules` can
 * compile it standalone, which would ordinarily be its own `check:diff-blame`. It is folded in
 * here instead because this project's shape is one script per *surface*, and the two halves of
 * the column's only real claim live on either side of the fold: `blameFor` decides which commit
 * a row belongs to, and the markup decides whether that answer reaches a cell the reader can
 * see. Splitting them would leave neither script able to state the claim on its own.
 *
 * `editor/blameModel.ts` is compiled alongside — also import-free — for the joins that would
 * otherwise be two constants that merely happen to agree today.
 *
 * Every failure in this column is silent. A cell against the wrong line still carries a
 * plausible short oid and a real author, so there is no gap, no artefact and no error: just a
 * per-line falsehood in a column whose entire purpose is to be believed.
 *
 * Run: `pnpm --dir ui run check:diff-render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
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

  const ok = (actual, what) => eq(actual, true, what)

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

  // --- the same promise in the side-by-side layout -------------------------------------------

  /*
   * The loop at the top already checks `selected === sent` for these, which is the claim that
   * matters. What is left is what only the split layout can get wrong: it draws a context line
   * in both columns, so it has two chances to write a position and must take exactly one, and
   * it pairs deletions against additions, so it must not lose a change line off the bottom of a
   * ragged pair.
   */
  const splitNames = Object.keys(byName).filter((n) => n.startsWith('split'))
  eq(splitNames.length, 5, 'the split layout is exercised at all')

  /*
   * The pairing boundary. `PAIR_FIXTURE` is `context, +, -, context`: the `+` closes its
   * edit, so the `-` after it opens a new one and the two are two one-sided runs, never one
   * zipped row. The columns are sequential in the DOM now, so document order across the
   * whole markup stopped meaning file order — the order claims are per column instead, and
   * `positions` must be exactly their concatenation, left first.
   */
  eq(
    byName.pairUnified.positions,
    ['0:0', '0:1', '0:2', '0:3'],
    'unified draws a hunk in file order',
  )
  eq(
    byName.pairSplit.leftPositions,
    ['0:0', '0:2', '0:3'],
    "the left column keeps the old side's file order: context, the deletion, context",
  )
  eq(
    byName.pairSplit.rightPositions,
    ['0:1'],
    "the right column carries the addition alone — a '+' then a '-' is two edits, so the "
      + 'deletion has no line to be faced against',
  )
  eq(
    byName.pairSplit.insertLeft,
    ['add'],
    "and the left column marks where the addition belongs — the un-zipped pair reads as two "
      + 'one-sided runs, each with its thin line on the side that lacks it',
  )
  eq(byName.pairSplit.insertRight, ['del'], 'the deletion marks the right column the same way')

  for (const d of Object.values(byName)) {
    eq(
      [...new Set(d.positions)].length,
      d.positions.length,
      `${d.name}: no position is written twice — a context line is drawn in both columns and `
        + 'numbered in only one',
    )
    eq(
      d.positions,
      [...d.leftPositions, ...d.rightPositions].length === 0
        ? d.positions
        : [...d.leftPositions, ...d.rightPositions],
      `${d.name}: positions are the two columns' in DOM order, left first`,
    )
    for (const side of ['leftPositions', 'rightPositions']) {
      eq(
        d[side],
        [...d[side]].sort((a, b) => {
          const [ah, al] = a.split(':').map(Number)
          const [bh, bl] = b.split(':').map(Number)
          return ah - bh || al - bl
        }),
        `${d.name}: ${side} is in its side's file order`,
      )
    }
  }

  // Every position the unified layout names, the split layout names too, and no other. Paired
  // by hand rather than by string surgery so a renamed case fails loudly instead of silently
  // comparing something against itself.
  for (const [split, unified] of [
    ['splitEmpty', 'empty'],
    ['splitRagged', 'ragged'],
    ['splitWholeHunk', 'wholeHunk'],
    ['splitEverything', 'everything'],
    ['splitJunk', 'junk'],
  ]) {
    eq(
      [...byName[split].positions].sort(),
      [...byName[unified].positions].sort(),
      `${split}: names every position ${unified} names, and no other`,
    )
    eq(
      [...byName[split].selected].sort(),
      [...byName[unified].selected].sort(),
      `${split}: paints exactly what ${unified} paints`,
    )
    eq(
      byName[split].count,
      byName[unified].count,
      `${split}: the footer counts the same lines — the layout is a layout, not a filter`,
    )
  }

  // --- the read-only arm: a file as one commit left it (M18) ----------------------------
  //
  // One sentence, checked against real markup rather than against the type: **a diff of two
  // commits offers no control that writes.** The discriminated union guarantees the *handlers*
  // are absent, which is a different claim — whether the buttons are drawn is decided by JSX,
  // and a greyed Stage button over a commit from 2019 would still be a control with no meaning
  // there. This is the one surface in the app where a wrong click writes to the index.
  for (const name of ['revision', 'revisionSplit']) {
    eq(byName[name].applyLabel, null, `${name}: no Stage/Unstage/Commit button is drawn at all`)
    eq(byName[name].sideControl, false, `${name}: no side switcher — the pair IS the tab's identity`)
    eq(byName[name].lineBox, false, `${name}: no per-line tick box, because nothing can be staged`)
    eq(byName[name].hunkBoxes, [], `${name}: and no hunk tri-state box either`)
    eq(byName[name].selected, [], `${name}: nothing is painted as selected`)
    eq(
      byName[name].revisions,
      '9f8e7d6 → a1b2c3d',
      `${name}: the pair is written out — a diff of two commits with nothing saying which two ` +
        `is a diff the reader cannot check`,
    )
    eq(byName[name].rows > 0, true, `${name}: the diff itself still draws`)
  }
  eq(
    [...byName.revisionSplit.positions].sort(),
    [...byName.revision.positions].sort(),
    'the read-only split names every position the unified one does, and no other',
  )
  // The header survives a fetch that found nothing, so the pane is not a dead end.
  eq(byName.revisionGone.rows, 0, 'a revision with no diff draws no rows')
  eq(byName.revisionGone.applyLabel, null, 'and still offers nothing that writes')
  eq(
    byName.revisionGone.revisions,
    '9f8e7d6 → a1b2c3d',
    'and still says which pair it was asked about',
  )
  // The staging arm must be unaffected — this is the regression the union exists to prevent.
  eq(byName.ragged.applyLabel !== null, true, 'the staging arm still has its apply button')
  eq(byName.ragged.sideControl, true, 'and its side switcher')
  eq(byName.ragged.lineBox, true, 'and its per-line boxes')
  eq(byName.ragged.revisions, null, 'and names no revision pair')

  // --- the blame column, in the markup (M18) --------------------------------------------
  //
  // `BLAME_FIXTURE` in the smoke has the run table these expectations are read from, and its
  // comment says which run was chosen to sit against which diff row. The claim is one sentence:
  // **every cell names the commit that wrote the line on the new side, and no row that has no
  // such line gets a cell.**

  eq(
    byName.empty.blameCells,
    [],
    'with no `blame` prop the column is not in the markup at all — absent, not zero-width; a ' +
      'diff nobody asked to annotate must not pay 22 characters of margin',
  )
  eq(byName.empty.blameTrack, false, 'and the row grid reserves no track for it')
  eq(byName.empty.blamePressed, false, 'and the toggle reads off, which is the default')
  eq(byName.empty.blameDisabled, false, 'while still being usable on a side that can answer')

  eq(
    byName.blame.blameCells,
    ['aaa1111', 'aaa1111', 'aaa1111', 'bbb2222', '', '', 'bbb2222', 'ccc3333', 'ccc3333', 'ccc3333', 'ccc3333'],
    'every row of the diff carries the commit covering its NEW line number — a context row and ' +
      'an addition alike',
  )
  eq(
    byName.blame.blameCells.length,
    byName.blame.rows,
    'one cell per row, no more and no fewer: the column is a column',
  )
  eq(
    byName.blame.blameCells[4],
    '',
    "the deletion's cell is empty. It has no new line number, and falling back to the old one " +
      'would put a commit beside a line that commit did not write',
  )
  eq(
    byName.blame.blameCells[5],
    '',
    'and so is the unstaged addition — nothing has committed it. It is a different fact from ' +
      'the deletion, which is why the age carries it and the oid cannot',
  )
  eq(
    byName.blame.blameAges,
    ['0', '0', '0', '3', '', '-1', '3', '5', '5', '5', '5'],
    'the age bucket reaches the markup, one band per row: `0` is under a day, `3` is a month to ' +
      'a quarter, `5` is the oldest, `-1` is uncommitted and `` is a row with no attribution',
  )
  eq(byName.blame.blameTrack, true, 'and the row grid reserves the track it needs')
  eq(byName.blame.blamePressed, true, 'the toggle reads on')
  eq(
    byName.blame.selected,
    byName.empty.selected,
    'and the column changes nothing about what is painted as selected',
  )
  eq(byName.blame.positions, byName.empty.positions, 'nor about which positions are written')

  // The split layout. The right column is the new side, so the cells are all on it; the left
  // column simply has no blame track — the columns are independent grids since M25, so there
  // is no alignment for a missing track to break and no spacers to count.
  eq(
    byName.blameSplit.blameCells,
    ['aaa1111', 'aaa1111', 'aaa1111', 'bbb2222', '', 'bbb2222', 'ccc3333', 'ccc3333', 'ccc3333', 'ccc3333'],
    'split names every commit unified names — minus the deletion, which is on the LEFT half ' +
      'there and the left half is never annotated',
  )
  eq(
    byName.blameSplit.blameAges,
    ['0', '0', '0', '3', '-1', '3', '5', '5', '5', '5'],
    'and the same bands, in the same order',
  )
  eq(
    byName.blameSplit.blameCells.filter((c) => c !== '').sort(),
    byName.blame.blameCells.filter((c) => c !== '').sort(),
    'no attribution is gained or lost by switching layout — the layout is a layout, not a filter',
  )
  eq(
    [...new Set(byName.blameSplit.positions)].length,
    byName.blameSplit.positions.length,
    'and the column does not disturb the rule the split layout exists to keep: no position is ' +
      'written twice',
  )
  eq(
    [...byName.blameSplit.positions].sort(),
    [...byName.blame.positions].sort(),
    'nor which positions are written at all',
  )

  // On but unanswered: a fetch in flight, or a run set `collapseRuns` refused. Same markup.
  eq(
    byName.blamePending.blameCells,
    [],
    'a refused run set draws no column at all — `collapseRuns` rejects a set with a hole, and a ' +
      'repaired column would be plausible and one line out for everything below the hole',
  )
  eq(byName.blamePending.blameTrack, false, 'and reserves no track while there is nothing in it')
  eq(
    byName.blamePending.blamePressed,
    true,
    'while the toggle still reads on, because the user did ask: absent is off, `null` is ' +
      'on-with-no-answer, and one prop carries both',
  )

  // The staged side. Its new text is the index, and nothing in `cide_git::blame` can produce the
  // index — see `diffBlame.blameRefusal` for why blaming the working file instead would be wrong
  // by a line or two, silently, on exactly the files that have unstaged changes on top.
  eq(byName.blameStaged.blameDisabled, true, 'the staged side cannot be annotated')
  eq(byName.blameStaged.blameCells, [], 'and draws no column')
  ok(
    byName.blameStaged.blameTitle.includes('index'),
    'and says why in the title rather than being silently inert — the listed-and-inert state ' +
      'this codebase keeps re-shipping',
  )
  eq(
    byName.blame.blameDisabled,
    false,
    'while the unstaged side, which ends at the working tree, is offered it',
  )
  eq(byName.everything.blameDisabled, false, 'and so is the combined side, for the same reason')

  // The read-only arm shows it too, and blames `new_rev` rather than HEAD. What is checked here
  // is that the column survives an arm with no tick box at all — that row grid has one fewer
  // track, and a cell placed by order lands in the wrong one if the two disagree.
  eq(
    byName.revisionBlame.blameCells,
    byName.blame.blameCells,
    'a diff of two commits carries the same column, cell for cell',
  )
  eq(byName.revisionBlame.lineBox, false, 'with no tick box beside it')
  eq(byName.revisionBlame.blameTrack, true, 'and a grid that still reserves the column')
  eq(
    byName.revision.blameCells,
    [],
    'and it is off by default there as well, rather than on because the pane is read-only',
  )

  // --- the whole-file view (M25) ------------------------------------------------------------
  //
  // The texts arrived on the wire and the pane now draws the document, not just the patch. The
  // claim that must not move is the old one: whole-file mode adds NO wire positions and loses
  // none — a gap row is unselectable by construction, not by a disabled flag.

  eq(
    byName.wholeUnified.positions,
    byName.empty.positions,
    'the whole file names exactly the positions the hunks-only render names: gap rows carry none',
  )
  eq(byName.wholeUnified.gaps, ['7', '7'], 'both unchanged runs are drawn, 7 lines each')
  eq(byName.wholeUnified.folds, [], 'and nothing folds under the size threshold')
  eq(byName.wholeUnified.selected, ['1:2'], 'ticking works exactly as before')
  eq(
    byName.wholeUnified.hunkBoxes,
    ['none', 'some', 'none'],
    'the staging arm keeps a tri-state box per hunk — on the slim bar now, the sticky @@ header '
      + 'is gone',
  )
  eq(
    byName.wholeUnified.count,
    '1 of 6 lines selected · lines',
    'and the footer still counts only the change lines, however many context rows are on screen',
  )

  eq(
    [...byName.wholeSplit.positions].sort(),
    [...byName.empty.positions].sort(),
    'the split whole file names the same set',
  )
  eq(
    byName.wholeSplit.leftPositions,
    ['0:0', '0:2', '1:0', '1:1', '1:3', '2:3'],
    'left column: contexts and the deletion, in old-side file order',
  )
  eq(
    byName.wholeSplit.rightPositions,
    ['0:1', '1:2', '2:0', '2:1', '2:2'],
    'right column: the additions, in new-side file order',
  )
  eq(
    byName.wholeSplit.insertLeft,
    ['add', 'add'],
    "two pure insertions (hunk 0's one line, hunk 2's three) mark the left column; the "
      + 'deletion-then-addition in hunk 1 is a pair — both sides have content, no marker',
  )
  eq(byName.wholeSplit.insertRight, [], 'and nothing marks the right column here')
  eq(byName.wholeSplit.hunkBoxes, ['none', 'some', 'none'], 'one box per hunk, left column only')

  eq(
    byName.wholeRevision.positions,
    byName.empty.positions,
    'the read-only arm draws the same whole file',
  )
  eq(byName.wholeRevision.gaps, ['7', '7'], 'gaps included')
  eq(
    byName.wholeRevision.hunkBoxes,
    [],
    'with no hunk bars at all — nothing to stage, and the line numbers are the landmarks',
  )
  eq(
    [...byName.wholeRevisionSplit.positions].sort(),
    [...byName.wholeRevision.positions].sort(),
    'and its split names the same set',
  )
  eq(byName.wholeRevisionSplit.insertLeft, ['add', 'add'], 'with the same insertion markers')
  eq(byName.wholeRevisionSplit.hunkBoxes, [], 'and still no box anywhere')

  eq(
    byName.wholeBig.folds,
    ['5998'],
    'over the size threshold the one long gap folds behind an expander row saying how much it hides',
  )
  eq(byName.wholeBig.gaps, [], 'and is not also drawn open')
  eq(byName.wholeBig.rows, 2, 'so the DOM is the changes plus a fold, not 6,000 rows')
  eq(byName.wholeBigOpen.folds, [], 'one click later the fold is gone')
  eq(byName.wholeBigOpen.gaps, ['5998'], 'and the gap is open in its place')

  eq(
    byName.wholeMismatch.positions,
    byName.empty.positions,
    'a text that contradicts a hunk line falls back to the hunks-only render, indistinguishable '
      + 'from a wire with no texts — wrong unchanged lines between correct changed ones would be '
      + 'silent, so the whole file is refused instead',
  )
  eq(byName.wholeMismatch.gaps, [], 'no gap of it is drawn')
  eq(byName.wholeMismatch.folds, [], 'and nothing folds')
  eq(byName.wholeMismatch.count, byName.empty.count, 'and the footer agrees with the fallback')

  eq(
    byName.edgeSplit.insertLeft,
    ['add', 'add'],
    'an insertion at line 1 marks above the first row and one at end of file marks below the '
      + 'last — the boundary a marker keyed on "the following row" would not have',
  )
  eq(byName.edgeSplit.insertRight, ['del'], 'and the deletion marks the right column')
  eq(byName.edgeSplit.leftPositions, ['0:1', '0:2', '0:3'], 'old side: context, deletion, context')
  eq(byName.edgeSplit.rightPositions, ['0:0', '0:4'], 'new side: the two additions')

  eq(
    byName.wholeBlame.blameCells.length,
    byName.wholeBlame.rows + 14,
    'blame annotates gap rows too — one cell per drawn line, hunk rows and the 14 gap lines alike',
  )
  eq(byName.wholeBlame.blameTrack, true, 'and the grid reserves the track')
  eq(
    byName.wholeBlameSplit.blameCells.length,
    24,
    'in the split, every right-column line row has a cell — 24 of them — and the left column '
      + 'has none, it is a different document',
  )

  // --- the blame column, as a model ---------------------------------------------------------
  //
  // `diffBlame.ts` compiled on its own. What the render section above cannot reach: the
  // boundaries of each run, a line past the end of the blamed file, and the field names the
  // structural restatement has to keep in step with Rust.

  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/diffBlame.ts',
      'src/panes/diffRows.ts',
      'src/panes/diffSync.ts',
      'src/editor/blameModel.ts',
      '--rootDir', 'src',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      '--noUnusedParameters',
      '--noUnusedLocals',
    ],
    { stdio: 'inherit' },
  )

  const { blameFor, blameRefusal, UNCOMMITTED_BUCKET } = await import(
    `file://${resolve(out, 'panes/diffBlame.js')}`
  )
  const blameModel = await import(`file://${resolve(out, 'editor/blameModel.js')}`)

  /** The smoke's run table, restated as a lookup so the boundaries can be walked. */
  const lookup = {
    lines: 30,
    runs: [
      { start: 1, lines: 5, commit: 0 },
      { start: 6, lines: 6, commit: 1 },
      { start: 12, lines: 1, commit: null },
      { start: 13, lines: 8, commit: 1 },
      { start: 21, lines: 10, commit: 2 },
    ],
    commits: [
      { shortOid: 'aaa1111', author: 'Ada Lovelace' },
      { shortOid: 'bbb2222', author: 'Grace Hopper' },
      { shortOid: 'ccc3333', author: 'Alan Turing' },
    ],
    buckets: [0, 3, UNCOMMITTED_BUCKET, 3, 5],
  }
  const at = (origin, newLineno) => blameFor(lookup, { origin, newLineno })

  eq(
    UNCOMMITTED_BUCKET,
    blameModel.UNCOMMITTED_BUCKET,
    "`diffBlame`'s restated uncommitted bucket is `blameModel`'s. The stylesheet keys its " +
      '"yours, not in history" tint on that exact number, so a drift paints an uncommitted line ' +
      'as the newest committed one',
  )
  eq(
    blameModel.AGE_BUCKETS.length,
    5,
    'and there are still five boundaries, so `5` is the oldest band the markup can carry',
  )

  eq(
    at('context', 1)?.oid,
    'aaa1111',
    'a context row is annotated: its new line number is a line of the new file like any other',
  )
  eq(at('addition', 2)?.oid, 'aaa1111', 'and so is an addition')
  eq(
    at('deletion', null),
    null,
    'a deletion is not. It has no new line number, and the old one names a line in a different ' +
      'document — the whole reason the old side is refused',
  )
  eq(
    at('deletion', 4),
    null,
    'and it is refused BY ORIGIN, not by the accident of a null: a wire that numbered deletions ' +
      'on both sides would otherwise start attributing them quietly',
  )

  // Every run boundary, from both directions. An off-by-one here is the failure mode that looks
  // exactly like a working column.
  for (const [line, oid] of [
    [1, 'aaa1111'], [5, 'aaa1111'], [6, 'bbb2222'], [11, 'bbb2222'],
    [13, 'bbb2222'], [20, 'bbb2222'], [21, 'ccc3333'], [30, 'ccc3333'],
  ]) {
    eq(at('context', line)?.oid, oid, `line ${line} lands in the run that covers it`)
  }
  eq(at('context', 5)?.author, 'Ada Lovelace', 'and carries that commit’s author')
  eq(at('context', 1)?.bucket, 0, 'and its age band')
  eq(at('context', 21)?.bucket, 5, 'including the oldest one')

  const uncommitted = at('addition', 12)
  eq(uncommitted?.uncommitted, true, 'a line no commit owns reports itself uncommitted')
  eq(uncommitted?.oid, '', 'with no oid')
  eq(uncommitted?.author, '', 'and no author — there is nobody to name')
  eq(uncommitted?.bucket, UNCOMMITTED_BUCKET, 'and the tint that is not a step on the age ramp')

  eq(at('context', 31), null, 'a line past the end of the blamed file has no cell…')
  eq(at('context', 0), null, '…and neither does line 0, which does not exist on a 1-based wire')
  eq(
    blameFor(null, { origin: 'context', newLineno: 1 }),
    null,
    'and no lookup means no cell, rather than a throw inside a render',
  )

  eq(blameRefusal('unstaged'), null, 'the unstaged side ends at the working tree and is blamable')
  eq(blameRefusal('combined'), null, 'so does the combined side')
  ok(
    (blameRefusal('staged') ?? '').length > 0,
    'the staged side is not: its new text is the index, which `cide_git::blame` cannot produce',
  )

  // --- the whole-file row model, on its own (M25) --------------------------------------------
  //
  // `diffRows.ts` compiled standalone, like `diffBlame.ts` above. What the render section
  // cannot reach: the offset arithmetic at every hunk boundary, the zero-length coverage
  // convention, and each validation edge — every one of which fails silently in markup, as a
  // plausible file with the wrong unchanged lines in it.

  const rowsMod = await import(`file://${resolve(out, 'panes/diffRows.js')}`)
  const {
    splitLines,
    wholeFileSegments,
    presentSegments,
    hunkSegments,
    columnRows,
    GAP_COLLAPSE_MIN,
    WHOLE_FILE_COLLAPSE_ABOVE,
  } = rowsMod

  eq(splitLines('a\nb\n'), ['a', 'b'], 'a trailing newline is a terminator, not a phantom line')
  eq(splitLines('a\nb'), ['a', 'b'], 'and its absence is not a lost line')
  eq(splitLines('a\r\nb\r\n'), ['a', 'b'], 'CRLF strips one \\r per line — the strip_newline rule')
  eq(splitLines(''), [], 'an empty text has no lines')

  const mkLine = (origin, content, oldLineno, newLineno) => ({
    origin,
    content,
    oldLineno,
    newLineno,
    noNewline: false,
  })
  /** The smoke's WHOLE fixture, restated: 24 new lines, three hunks, two 7-line gaps. */
  const wholeHunks = [
    {
      index: 0, header: '@@ -1,2 +1,3 @@', oldStart: 1, oldLines: 2, newStart: 1, newLines: 3,
      lines: [
        mkLine('context', 'fn main() {', 1, 1),
        mkLine('addition', '    let x = 1;', null, 2),
        mkLine('context', '}', 2, 3),
      ],
    },
    {
      index: 1, header: '@@ -10,3 +11,3 @@', oldStart: 10, oldLines: 3, newStart: 11, newLines: 3,
      lines: [
        mkLine('context', 'let a = 0;', 10, 11),
        mkLine('deletion', 'let b = 1;', 11, null),
        mkLine('addition', 'let b = 2;', null, 12),
        mkLine('context', 'let c = 3;', 12, 13),
      ],
    },
    {
      index: 2, header: '@@ -20,1 +21,4 @@', oldStart: 20, oldLines: 1, newStart: 21, newLines: 4,
      lines: [
        mkLine('addition', 'one', null, 21),
        mkLine('addition', 'two', null, 22),
        mkLine('addition', 'three', null, 23),
        mkLine('context', 'end', 20, 24),
      ],
    },
  ]
  const wholeText =
    ['fn main() {', '    let x = 1;', '}', 'g4', 'g5', 'g6', 'g7', 'g8', 'g9', 'g10',
     'let a = 0;', 'let b = 2;', 'let c = 3;', 'g14', 'g15', 'g16', 'g17', 'g18', 'g19', 'g20',
     'one', 'two', 'three', 'end'].join('\n') + '\n'

  const segments = wholeFileSegments(wholeHunks, wholeText)
  eq(
    segments?.map((s) => s.kind),
    ['hunk', 'gap', 'hunk', 'gap', 'hunk'],
    'the file is hunks with the gaps between their coverages filled in',
  )
  eq(
    [segments?.[1]?.lines[0]?.oldLineno, segments?.[1]?.lines[0]?.newLineno],
    [3, 4],
    "the first gap starts numbered on BOTH sides — the old number is the new one plus hunk 0's "
      + 'declared offset, no second text needed',
  )
  eq(
    [segments?.[3]?.lines.at(-1)?.oldLineno, segments?.[3]?.lines.at(-1)?.newLineno],
    [19, 20],
    'and the second gap ends one line short of hunk 2 on both sides',
  )
  eq(
    segments?.[1]?.lines.length,
    7,
    'seven lines in the first gap — new 4 through 10',
  )

  eq(
    wholeFileSegments(wholeHunks, null),
    null,
    'no text, no whole file: the fallback is the hunks-only render, never a guess',
  )
  eq(
    wholeFileSegments(wholeHunks, wholeText.replace('let a = 0;', 'DIFFERENT')),
    null,
    'a context line the text contradicts refuses the whole file',
  )
  eq(
    wholeFileSegments(wholeHunks, wholeText.replace('    let x = 1;', 'WRONG')),
    null,
    'and so does an addition — every line that exists on the new side is checked',
  )
  eq(
    wholeFileSegments([wholeHunks[1], wholeHunks[0], wholeHunks[2]], wholeText),
    null,
    'hunks out of order are refused, not sorted — an out-of-order wire is a wire this module '
      + 'does not understand',
  )
  eq(
    wholeFileSegments(wholeHunks, 'fn main() {\n'),
    null,
    'a coverage past the end of the text is refused',
  )
  eq(
    wholeFileSegments(
      [{ ...wholeHunks[0], lines: wholeHunks[0].lines.map((l) =>
        l.content === '}' ? { ...l, content: '}\r' } : l) }],
      'fn main() {\r\n    let x = 1;\r\n}\r\n',
    ),
    null,
    'but hunk content never carries the \\r the text had — Rust already stripped it — so a hunk '
      + 'that does is a mismatch like any other',
  )
  eq(
    wholeFileSegments(
      wholeHunks.slice(0, 1),
      'fn main() {\r\n    let x = 1;\r\n}\r\n',
    )?.map((s) => s.kind),
    ['hunk'],
    'while a CRLF text against LF hunk content compares equal, one stripped \\r per line',
  )
  eq(
    wholeFileSegments(
      [{ index: 0, header: '@@ -1,2 +0,0 @@', oldStart: 1, oldLines: 2, newStart: 0, newLines: 0,
         lines: [mkLine('deletion', 'a', 1, null), mkLine('deletion', 'b', 2, null)] }],
      '',
    )?.map((s) => s.kind),
    ['hunk'],
    'a whole-file deletion — the zero-length coverage convention, git numbering the line BEFORE '
      + 'the position — reconstructs to just the hunk and no phantom gap',
  )

  // The folding policy.
  ok(WHOLE_FILE_COLLAPSE_ABOVE === 5_000, 'the threshold is the documented 5,000 lines')
  const gapOf = (n) => ({
    kind: 'gap',
    index: 0,
    lines: Array.from({ length: n }, (_, i) =>
      mkLine('context', `l${i + 1}`, i + 1, i + 1)),
  })
  eq(
    presentSegments([gapOf(500)], 4_000, new Set()).map((s) => s.kind),
    ['gap'],
    'under the threshold nothing folds, whatever the gap length — the whole file is the feature',
  )
  eq(
    presentSegments([gapOf(GAP_COLLAPSE_MIN)], 6_000, new Set()).map((s) => s.kind),
    ['gap'],
    'a gap at the minimum stays open even over the threshold: a fold that hides ten lines buys '
      + 'nothing',
  )
  const folded = presentSegments([gapOf(GAP_COLLAPSE_MIN + 1)], 6_000, new Set())
  eq(folded.map((s) => s.kind), ['fold'], 'one past the minimum folds')
  eq(folded[0]?.count, GAP_COLLAPSE_MIN + 1, 'and says how much it hides')
  eq(
    presentSegments([gapOf(GAP_COLLAPSE_MIN + 1)], 6_000, new Set([0])).map((s) => s.kind),
    ['gap'],
    'and an expanded index opens it again',
  )

  // The split columns and their run table.
  const cols = columnRows(segments, { bars: false })
  eq(
    cols.runs.map((r) => r.kind),
    ['shared', 'add', 'shared', 'shared', 'shared', 'pair', 'shared', 'shared', 'add', 'shared'],
    'the run table: context and gaps are shared, a lone insertion is `add`, a deletion followed '
      + 'by its addition is one `pair`',
  )
  for (const run of cols.runs) {
    if (run.kind !== 'shared') continue
    eq(
      run.leftTo - run.leftFrom,
      run.rightTo - run.rightFrom,
      'a shared run occupies both columns with equal row counts — the equality the scroll sync '
        + 'leans on',
    )
  }
  eq(
    cols.runs.filter((r) => r.kind === 'add').every((r) => r.leftFrom === r.leftTo),
    true,
    'an add run is empty on the left',
  )
  eq(
    cols.left.length + cols.right.length,
    cols.runs.reduce((n, r) => n + (r.leftTo - r.leftFrom) + (r.rightTo - r.rightFrom), 0),
    'every row is in exactly one run',
  )
  const barred = columnRows(hunkSegments(wholeHunks), { bars: true, collapsed: new Set([1]) })
  eq(
    barred.left.filter((r) => r.kind === 'bar').length,
    3,
    'the fallback draws a bar per hunk in each column',
  )
  eq(
    barred.left.some((r) => r.kind === 'line' && r.hunk === 1),
    false,
    'and a collapsed hunk keeps its bar and folds its lines away',
  )

  // --- the scroll mapping, on its own (M25) --------------------------------------------------

  const syncMod = await import(`file://${resolve(out, 'panes/diffSync.js')}`)
  const { rowSpans, mapScroll, insertMarkers } = syncMod

  eq(
    rowSpans(cols.runs).length,
    cols.runs.filter((r) => r.kind !== 'shared').length,
    'only disagreeing runs become spans — shared runs carry the offset and need no anchor',
  )
  eq(
    insertMarkers(cols.runs).map((m) => `${m.column}:${m.tone}`),
    ['left:add', 'left:add'],
    'markers land in the column that LACKS the run: both pure insertions mark the left, the '
      + 'pair marks nothing',
  )
  const delRuns = [{ kind: 'del', leftFrom: 0, leftTo: 2, rightFrom: 0, rightTo: 0 }]
  eq(
    insertMarkers(delRuns),
    [{ column: 'right', beforeRow: 0, tone: 'del' }],
    'and a lone deletion marks the right',
  )

  // A geometry with a pure insertion on the right: left pixels 0..300, right 0..500, the
  // insertion occupying right 100..300 while the left holds at 100.
  const geometry = {
    anchors: [
      { a: 0, b: 0 },
      { a: 100, b: 100 },
      { a: 100, b: 300 },
      { a: 300, b: 500 },
    ],
    aMax: 200,
    bMax: 400,
  }
  eq(mapScroll(geometry, 'a', 0), 0, 'exact at the start anchor')
  eq(mapScroll(geometry, 'b', 100), 100, 'exact at a boundary anchor')
  eq(
    mapScroll(geometry, 'b', 200),
    100,
    'inside the insertion the left column holds still at the marker — the IDEA behaviour',
  )
  eq(mapScroll(geometry, 'b', 300), 100, 'right up to its end')
  eq(mapScroll(geometry, 'b', 400), 200, 'and interpolates again past it')
  eq(mapScroll(geometry, 'a', 200), 400, 'the same segment from the left maps into the stretch')
  eq(mapScroll(geometry, 'a', 9_999), 400, 'over-scroll clamps to what the target can reach')
  eq(mapScroll(geometry, 'b', -5), 0, 'and under-scroll clamps to zero')
  {
    let last = -1
    for (let top = 0; top <= 500; top += 7) {
      const mapped = mapScroll(geometry, 'b', top)
      ok(mapped >= last, `monotonic at b=${top}: a mapping that goes backwards judders`)
      last = mapped
    }
  }
  eq(
    mapScroll({ anchors: [], aMax: 90, bMax: 90 }, 'a', 40),
    40,
    'no anchors — no changed runs — is the identity mapping, clamped',
  )

  // --- the joins ----------------------------------------------------------------------------
  //
  // The structural restatement is not a second source of truth only for as long as these hold.
  // Same slice `check-blame.mjs` takes, for the same reason: a field renamed in
  // `crates/cide-ipc/src/history.rs` shows up as `undefined` in a cell and nowhere else.

  const generated = readFileSync('src/ipc/generated.ts', 'utf8')
  const shape = (name) => {
    const start = generated.indexOf(`export type ${name} = `)
    if (start < 0) return ''
    const end = generated.indexOf('};', start)
    return generated.slice(start, end < 0 ? generated.length : end)
  }
  for (const [type, fields] of [
    ['BlameFile', ['lines', 'runs', 'commits']],
    ['BlameRun', ['start', 'lines', 'commit']],
    ['BlameCommit', ['shortOid', 'author']],
    ['DiffLineView', ['origin', 'oldLineno', 'newLineno']],
    // The whole-file texts, on both diff wires — `diffRows`' restatements mean nothing if
    // these leave.
    ['FileDiff', ['oldText', 'newText', 'textsOmitted', 'hunks', 'rev', 'partialOk']],
    ['RevisionDiff', ['oldText', 'newText', 'textsOmitted']],
    ['DiffHunkView', ['oldStart', 'oldLines', 'newStart', 'newLines']],
  ]) {
    const body = shape(type)
    ok(body !== '', `${type} exists in generated.ts`)
    for (const field of fields) {
      ok(
        new RegExp(`\\b${field}\\b`).test(body),
        `${type}.${field} is still on the wire, so \`diffBlame\`'s restatement of it still means ` +
          'something',
      )
    }
  }
  eq(
    /export type DiffSide = [^;]*;/.exec(generated)?.[0],
    'export type DiffSide = "staged" | "unstaged" | "combined";',
    "`blameRefusal` is total over the sides, and restates them; a fourth would arrive here " +
      'un-considered and default to blamable',
  )

  // The stylesheet. `check:theme` walks the tokens; what is pinned here is that a rule exists
  // for every band the model can emit, and that the column is the width the editor's is.
  const css = readFileSync('src/panes/GitDiffPane.module.css', 'utf8')
  eq(
    /--blame-col:\s*(\d+)ch/.exec(css)?.[1],
    `${blameModel.BLAME_LABEL_CHARS}`,
    "the diff's column is exactly as wide as the editor gutter's `BLAME_LABEL_CHARS`, so a file " +
      'open in both does not have two different left margins',
  )
  for (let band = 0; band <= blameModel.AGE_BUCKETS.length; band += 1) {
    ok(
      css.includes(`.blame[data-age='${band}']`),
      `the ramp has a rule for band ${band} — a missing one is an untinted row that reads as ` +
        '"the same age as its neighbours"',
    )
  }
  ok(
    css.includes(`.blame[data-age='${blameModel.UNCOMMITTED_BUCKET}']`),
    'and one for the uncommitted band, which is deliberately not a step on that ramp',
  )
  for (const rule of [
    ".lines[data-boxed='false'] .row",
    ".lines[data-blamed='true'] .row",
    ".lines[data-boxed='false'][data-blamed='true'] .row",
    ".column[data-boxed='false'] .half",
    ".column[data-blamed='true'] .half",
    ".column[data-boxed='false'][data-blamed='true'] .half",
  ]) {
    ok(
      css.includes(rule),
      `\`${rule}\` has its own column list. A grid places cells by ORDER, so a template with a ` +
        'track for a cell that was not rendered puts every later cell one column left',
    )
  }

  // The split layout's own pieces: the insertion marker, and the corpses that must stay buried.
  ok(
    /\.insertMark\s*\{[^}]*height:\s*0/.test(css),
    'the insertion marker takes NO layout height — a 2px-tall element would shift every row '
      + 'below it and with them every measured scroll anchor',
  )
  ok(
    /\.insertMark::before\s*\{[^}]*height:\s*2px/.test(css),
    'while its ::before paints the visible 2px line by overdraw',
  )
  ok(
    css.includes(".insertMark[data-tone='add']::before") && css.includes('var(--green)'),
    'the add tone is the theme green — a token declared in both palette blocks, not a literal',
  )
  ok(
    css.includes(".insertMark[data-tone='del']::before") && css.includes('var(--red)'),
    'and the del tone the theme red',
  )
  for (const corpse of ['.filler', '.blameGap', '.splitRow']) {
    ok(
      !css.includes(corpse),
      `\`${corpse}\` is gone: the split view aligns by scroll mapping now, and a resurrected ` +
        'filler cell is the empty-space layout the markers replaced',
    )
  }
  ok(
    /\.column\s*\{[^}]*position:\s*relative/.test(css),
    "the column is positioned, so each row's offsetTop is scroller-relative — remove this and " +
      'every measured anchor is off by the column’s own offset in the pane',
  )

  // --- both diff panes offer the layout toggle (M21) --------------------------------------------
  //
  // > *"when opening diff from git panel - i'm not able to set split view of the diff… Split
  // > button just grey and not clickable"*
  //
  // A source check, because the two panes are *wiring* rather than view: `GitDiffView` itself is
  // what this file renders, and it correctly draws the control disabled when handed no `onView`.
  // The defect was one caller never handing it one — and that caller's own comment claimed the
  // opposite, which is why the claim is now asserted instead of merely written down.

  const paneSrc = readFileSync('src/panes/GitDiffPane.tsx', 'utf8')
  eq(
    [...paneSrc.matchAll(/diffView\.writable \? \{ onView: setDiffView \}/g)].length,
    2,
    'both panes wire the layout toggle — the working-tree one and the read-only revision one. A '
      + 'read-only diff has nothing to stage, so side-by-side is if anything the commoner reason '
      + 'to open one',
  )
  eq(
    [...paneSrc.matchAll(/subscribeDiffView, getDiffView, getServerDiffView/g)].length,
    2,
    '…from the same store, so the mode is one editor setting rather than one remembered per '
      + 'pane kind',
  )

  // --- the split view's scroll sync, pinned in the pane (M25) -------------------------------
  //
  // The pure mapping is driven above; what only the pane holds is the wiring discipline, and
  // each of these is a bug with no error and no changed pixel in any snapshot if it goes.

  ok(
    /const echoes = useRef<Set<'left' \| 'right'>>/.test(paneSrc),
    'the echo guard is a Set of marks — MergePane.tsx documents at length why a time-released '
      + 'flag cannot guard a loop whose echo event arrives asynchronously',
  )
  ok(
    paneSrc.includes('echoSet.delete(fromName)'),
    'and the echoed scroll consumes its mark and stops — that consumption IS the loop guard',
  )
  ok(
    [...paneSrc.matchAll(/addEventListener\('scroll'/g)].length >= 2,
    'both columns are listened to, so wheel, keyboard and find-in-page all ride one path',
  )
  ok(
    /whenResizeSettles\(key, measure\)/.test(paneSrc),
    'anchors re-measure only when a size SETTLES — a per-event measure is the drag-stutter '
      + 'class check:resize exists for',
  )
  ok(
    !paneSrc.includes('gitDiffBlameGap'),
    'the blame spacer is gone with the filler cells; the left column simply has no blame track',
  )
  ok(
    /import[^;]*from '\.\/diffRows'/s.test(paneSrc) && /import[^;]*from '\.\/diffSync'/s.test(paneSrc),
    'the pane consumes the two pure modules this script drives, not private copies of them',
  )

  if (failed === 0) console.log('git diff view render: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
