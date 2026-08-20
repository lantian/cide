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
   * The pairing boundary, in document order. `PAIR_FIXTURE` is `context, +, -, context`: the
   * `+` closes its edit, so the `-` after it opens a new one and the two must NOT be zipped
   * onto one row. Zipped, the addition and the deletion swap places in the document, which the
   * sorted comparisons below are blind to by construction.
   */
  eq(
    byName.pairUnified.positions,
    ['0:0', '0:1', '0:2', '0:3'],
    'unified draws a hunk in file order',
  )
  eq(
    byName.pairSplit.positions,
    ['0:0', '0:1', '0:2', '0:3'],
    "split keeps file order too: a '+' followed by a '-' is two edits, so the deletion goes on "
      + 'its own row below rather than being faced against the addition above it',
  )

  for (const name of splitNames) {
    const d = byName[name]
    eq(
      [...new Set(d.positions)].length,
      d.positions.length,
      `${name}: no position is written twice — a context line is drawn in both columns and `
        + 'numbered in only one',
    )
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
  eq(byName.empty.blameGaps, 0, 'and no spacer either')
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
  eq(byName.blame.blameGaps, 0, 'unified draws no spacers — there is only one column of code')
  eq(byName.blame.blamePressed, true, 'the toggle reads on')
  eq(
    byName.blame.selected,
    byName.empty.selected,
    'and the column changes nothing about what is painted as selected',
  )
  eq(byName.blame.positions, byName.empty.positions, 'nor about which positions are written')

  // The split layout. The right half is the new side, so the cells are all on it; the left half
  // is the old side and gets a spacer, which is what keeps the two halves' grids in step.
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
    byName.blameSplit.blameGaps,
    6,
    'one spacer per drawn old-side cell, so the two halves keep the same column list. A missing ' +
      'one slides the old code out of line with the new for every row below it',
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
    ".lines[data-boxed='false'] .half",
    ".lines[data-blamed='true'] .half",
    ".lines[data-boxed='false'][data-blamed='true'] .half",
  ]) {
    ok(
      css.includes(rule),
      `\`${rule}\` has its own column list. A grid places cells by ORDER, so a template with a ` +
        'track for a cell that was not rendered puts every later cell one column left',
    )
  }

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

  if (failed === 0) console.log('git diff view render: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
