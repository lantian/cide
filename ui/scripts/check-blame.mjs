/*
 * `TZ` first, before anything can construct a `Date`.
 *
 * `blameModel.ts` formats in **local** time on purpose — the question a blame gutter answers is
 * "when did I write this", and a commit made at 23:30 that shows yesterday's date is wrong in the
 * one way the reader can detect. That makes its output machine-dependent, which is exactly what a
 * check must not be, so this process pins itself to UTC and asserts against UTC. Node re-reads
 * `process.env.TZ` on the next `Date` it builds, so this line has to be the first statement in the
 * file and above the imports it shares a module with.
 */
process.env.TZ = 'UTC'

/**
 * Checks the blame gutter's import-free model — `editor/blameModel.ts` — and pins it against the
 * three files it has to agree with: the generated wire types, the editor surface that hosts the
 * column, and the stylesheet that paints it.
 *
 * # Why this exists
 *
 * Every failure this feature has is silent. A run set with a hole paints a gutter that is one line
 * off for everything below it, and **every line still carries a label**, so there is no gap, no
 * artefact and no error — just a per-line falsehood in the one surface whose entire purpose is to
 * be believed. A renamed field on the Rust side gives `undefined` in a cell. A missing tint class
 * gives an untinted band that reads as "these lines are the same age". None of it throws, none of
 * it is visible in a screenshot, and there is no JS test runner in this project.
 *
 * Same shape as `check-sidebar.mjs`: `blameModel.ts` is import-free precisely so the TypeScript in
 * `node_modules` can compile it on its own and this file can import the output.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that the column appears. There is no DOM and no CodeMirror in this process. That a
 *     `GutterMarker` without `toDOM` contributes only its `elementClass` was established by
 *     reading `@codemirror/view`'s `GutterElement.setMarkers`, and is written down in `blame.ts`'s
 *     header; that the gutter is laid out left of the numbers rests on extension order, which is
 *     source-asserted below and not measured.
 *   - that the hover card is positioned correctly, or that `flushSync` is enough to measure it.
 *     Both need a live view.
 *   - the round trip through Rust. `crates/cide-git/src/blame.rs` owns the runs and asserts
 *     `check_runs` on every route; this file asserts what the *frontend* does when that promise is
 *     broken anyway.
 *   - the age *colours*. `check:theme` walks every token a stylesheet references; what is pinned
 *     here is that a rule exists for every bucket the model can emit.
 *
 * Run: `pnpm --dir ui run check:blame`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-blame-'))
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

/** Comments stripped, so a pin certifies the code and not the prose that discusses it. */
const stripJs = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/blameModel.ts',
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

  const {
    AGE_BUCKETS,
    BLAME_HOVER_MS,
    BLAME_LABEL_CHARS,
    UNCOMMITTED_BUCKET,
    UNCOMMITTED_LABEL,
    ageBucket,
    blameLabel,
    collapseRuns,
    popupLines,
  } = await import(`file://${join(out, 'blameModel.js')}`)

  const DAY = 86_400
  /** 2026-08-19 12:00:00Z. Midday so nothing here sits near a date boundary. */
  const NOW = Date.UTC(2026, 7, 19, 12, 0, 0) / 1000

  /** A commit table entry, with only the fields that matter to the assertion overridden. */
  const commit = (over = {}) => ({
    oid: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678',
    shortOid: 'a1b2c3d',
    summary: 'Fix the parser',
    author: 'Ada Lovelace',
    authorEmail: 'ada@example.org',
    authored: BigInt(NOW - 40 * DAY),
    origPath: null,
    boundary: false,
    ...over,
  })

  const file = (over = {}) => ({
    path: 'src/main.rs',
    lines: 0,
    runs: [],
    commits: [],
    downgraded: null,
    ...over,
  })

  // --- 1. the run invariant is enforced, not assumed -------------------------------------
  //
  // Each of these is *rejected*, not repaired. A repair — extend the previous run over the hole,
  // clamp the overshoot — produces a plausible column that is wrong in a way nothing downstream
  // can notice: a gutter one line off below a hole still puts a label on every line.

  const three = [commit()]

  eq(
    collapseRuns(
      file({
        lines: 6,
        // 1..2, then 4..6 — line 3 belongs to nobody.
        runs: [{ start: 1, lines: 2, commit: 0 }, { start: 4, lines: 3, commit: 0 }],
        commits: three,
      }),
      NOW,
    ).length,
    0,
    'a run set with a HOLE draws no column at all — the alternative is a gutter that is one line ' +
      'off for everything below the hole, with a label on every line and nothing to notice',
  )

  eq(
    collapseRuns(
      file({
        lines: 4,
        runs: [{ start: 2, lines: 3, commit: 0 }],
        commits: three,
      }),
      NOW,
    ).length,
    0,
    'a run set that does not reach line 1 is rejected — line numbers are 1-based on this wire, ' +
      'and a set starting at 2 is an off-by-one that would silently shift the whole column',
  )

  eq(
    collapseRuns(
      file({
        lines: 4,
        runs: [{ start: 1, lines: 9, commit: 0 }],
        commits: three,
      }),
      NOW,
    ).length,
    0,
    'a run set that OVERSHOOTS `lines` is rejected rather than clipped',
  )

  eq(
    collapseRuns(
      file({ lines: 3, runs: [{ start: 1, lines: 3, commit: 7 }], commits: three }),
      NOW,
    ).length,
    0,
    'a run whose `commit` indexes past the table is rejected — that index is the only thing ' +
      'standing between the wire and `undefined.author`',
  )

  eq(
    collapseRuns(file({ lines: 3, runs: [{ start: 1, lines: 0, commit: 0 }], commits: three }), NOW)
      .length,
    0,
    'a zero-length run is rejected; `BlameRun.lines` is documented as never zero',
  )

  eq(collapseRuns(file(), NOW).length, 0, 'an empty file has no column, which is also `[]`')

  // --- 2. the collapsing rule -------------------------------------------------------------

  const older = commit({ oid: 'b'.repeat(40), shortOid: 'bbbbbbb', author: 'Grace Hopper' })
  const cover = file({
    lines: 6,
    runs: [
      // Length 1 at the very start...
      { start: 1, lines: 1, commit: 0 },
      { start: 2, lines: 4, commit: 1 },
      // ...and length 1 at the very end. Both are the shapes an off-by-one loses first.
      { start: 6, lines: 1, commit: 0 },
    ],
    commits: [commit(), older],
  })
  const markers = collapseRuns(cover, NOW)

  eq(markers.length, 6, 'a valid cover gives exactly one marker per line')
  eq(
    markers.map((m) => m.line),
    [1, 2, 3, 4, 5, 6],
    'and the markers are the lines, in order, 1-based',
  )
  eq(
    markers.map((m) => m.label !== ''),
    [true, true, false, false, false, true],
    'ONLY a run\'s first line carries a label — IDEA\'s collapsing, and what makes a ' +
      '22-character column legible instead of the same name forty times',
  )
  eq(
    markers.map((m) => m.bucket),
    [3, 3, 3, 3, 3, 3],
    'but EVERY line carries the bucket, so the tint reads as a block while the text does not ' +
      'repeat',
  )
  eq(
    markers.map((m) => m.oid.slice(0, 1)),
    ['a', 'b', 'b', 'b', 'b', 'a'],
    'and every line carries its own oid, so a click anywhere in a run opens the right commit',
  )
  eq(
    new Set(markers.slice(1, 5).map((m) => m.title)).size,
    1,
    'every line of one run shares one card — built once per run, not once per line',
  )
  eq(
    markers[1].title,
    popupLines(older, null).join('\n'),
    'and `BlameMarker.title` IS `popupLines(...).join("\\n")` — the hover card and the marker ' +
      'cannot be two renderings of one commit that disagree',
  )

  // --- 3. the age ramp --------------------------------------------------------------------

  eq(AGE_BUCKETS.length, 5, 'five boundaries, so six buckets: 0..=AGE_BUCKETS.length')
  eq(
    [...AGE_BUCKETS].sort((a, b) => a - b),
    [...AGE_BUCKETS],
    'and they ascend, which is what makes the linear scan in `ageBucket` correct',
  )

  {
    // Monotone: as `authored` moves forward in time the bucket never rises.
    const walk = []
    for (let ago = 0; ago <= 800 * DAY; ago += DAY / 4) walk.push(ageBucket(NOW - ago, NOW))
    const rising = walk.filter((b, i) => i > 0 && b < walk[i - 1])
    eq(rising, [], '`ageBucket` is monotone in `authored` over 800 days sampled every six hours')
  }

  eq(ageBucket(NOW - 3600, NOW), 0, 'an hour old is the newest bucket')
  eq(ageBucket(NOW + 9 * DAY, NOW), 0, 'and so is a FUTURE timestamp — clock skew and rebases ' +
    'produce them routinely, and the alternative is painting the newest line as the oldest')
  eq(ageBucket(NOW - 10 * 365 * DAY, NOW), AGE_BUCKETS.length, 'ten years saturates at the top')
  eq(
    ageBucket(NOW - 20 * 365 * DAY, NOW),
    ageBucket(NOW - 10 * 365 * DAY, NOW),
    'and twenty years is the SAME bucket as ten — the scale is exponential, so nothing above a ' +
      'year is distinguished, which is the honest answer',
  )
  ok(
    ageBucket(NOW - 20 * DAY, NOW) !== ageBucket(NOW - 385 * DAY, NOW),
    'while two commits a year apart land in DIFFERENT buckets — the whole point of an ' +
      'exponential ramp is that a linear one puts a two-year-old file in one band and says nothing',
  )

  // --- 4. the label ------------------------------------------------------------------------

  eq(BLAME_LABEL_CHARS, 22, 'the column is 22 characters wide')

  {
    const long = blameLabel('Bartholomew Featherstonehaugh', NOW - 40 * DAY, NOW)
    eq(long.length, BLAME_LABEL_CHARS, 'an over-long author clips to exactly BLAME_LABEL_CHARS')
    ok(long.includes('…'), 'and says so with an ellipsis')
    ok(
      long.endsWith('2026-07-10'),
      'the DATE survives the clip and the name gives way: a truncated date is a different and ' +
        `plausible-looking day, which is unrecoverable (got ${JSON.stringify(long)})`,
    )
    eq(
      blameLabel('Ada Byron', NOW - 40 * DAY, NOW),
      'Ada Byron 2026-07-10',
      'a name that fits in the eleven characters the date leaves is not touched',
    )
    ok(
      blameLabel('Ada Byron', NOW - 40 * DAY, NOW).length <= BLAME_LABEL_CHARS,
      'and never exceeds the column it is drawn in',
    )
    eq(
      blameLabel('Ada Lovelace', NOW - 3600, NOW),
      'Ada Lovelace 11:00',
      'a commit less than a day old shows the TIME instead of the date — which is why `now` is a ' +
        'parameter of this function and not only of `ageBucket`',
    )
  }

  // --- 5. uncommitted ------------------------------------------------------------------------

  {
    const dirty = collapseRuns(
      file({
        lines: 3,
        runs: [{ start: 1, lines: 1, commit: 0 }, { start: 2, lines: 2, commit: null }],
        commits: [commit()],
        downgraded: null,
      }),
      NOW,
    )
    eq(
      dirty.slice(1).map((m) => m.bucket),
      [UNCOMMITTED_BUCKET, UNCOMMITTED_BUCKET],
      'an uncommitted run gets UNCOMMITTED_BUCKET, which is not a value the age ramp can produce',
    )
    ok(UNCOMMITTED_BUCKET < 0, 'and it is negative, so it can never collide with a bucket index')
    eq(
      dirty.slice(1).map((m) => m.oid),
      ['', ''],
      'and no oid, which is what makes it not a click target',
    )
    eq(dirty[1].label, UNCOMMITTED_LABEL, 'the run\'s first line says so')
    eq(dirty[2].label, '', 'and the rest collapse, exactly like a committed run')
  }

  // --- 6. the card --------------------------------------------------------------------------

  eq(
    popupLines(commit(), null),
    ['a1b2c3d Fix the parser', 'Ada Lovelace <ada@example.org>', '2026-07-10 12:00'],
    'the card is heading, author, exact date — and NOTHING else when there is nothing else to say',
  )
  eq(
    popupLines(commit({ origPath: 'src/parser.rs' }), null).length,
    4,
    'a followed rename adds one row',
  )
  ok(
    popupLines(commit({ origPath: 'src/parser.rs' }), null).some((l) => l.includes('src/parser.rs')),
    'naming the path the file had then',
  )
  ok(
    !popupLines(commit(), null).some((l) => l.startsWith('↳')),
    'and the `↳ was …` row appears ONLY when the path differs — `BlameCommit.origPath` is `None` ' +
      'when it equals `BlameFile.path`, so Rust has already made the comparison and re-deriving ' +
      'it here would be a second, worse answer',
  )
  ok(
    popupLines(commit({ boundary: true }), null).some((l) => /boundary/i.test(l)),
    'a boundary commit says it is the walk\'s limit rather than presenting itself as authorship',
  )
  eq(
    popupLines(null, null)[0],
    UNCOMMITTED_LABEL,
    'an uncommitted line gets a card of its own, headed with the same word the cell uses',
  )
  eq(
    popupLines(commit(), 'Copy detection timed out; fell back to rename-follow.').at(-1),
    'Copy detection timed out; fell back to rename-follow.',
    '`BlameFile.downgraded` rides on every card, last — a weaker answer presented as the ' +
      'requested one is exactly what that field exists to prevent',
  )
  eq(popupLines(commit(), '').length, 3, 'an empty note adds no row')

  eq(BLAME_HOVER_MS >= 150 && BLAME_HOVER_MS <= 600, true, 'the hover delay is in the human band')

  // --- 7. the wire shape, pinned against the generated types --------------------------------
  //
  // `blameModel.ts` restates these structurally (it may not import them and stay
  // standalone-compilable), so a rename in `crates/cide-ipc/src/history.rs` would typecheck on
  // both sides and produce `undefined` in a gutter cell. This is the join that catches it.

  const generated = readFileSync('src/ipc/generated.ts', 'utf8')
  const shape = (name) => {
    const at = generated.indexOf(`export type ${name} = `)
    if (at < 0) return ''
    const end = generated.indexOf('};', at)
    return generated.slice(at, end < 0 ? generated.length : end)
  }
  for (const [type, fields] of [
    ['BlameFile', ['path', 'head', 'lines', 'runs', 'commits', 'source', 'dirty', 'follow', 'downgraded']],
    ['BlameRun', ['start', 'lines', 'commit']],
    [
      'BlameCommit',
      ['oid', 'shortOid', 'summary', 'author', 'authorEmail', 'authored', 'origPath', 'boundary'],
    ],
  ]) {
    const body = shape(type)
    ok(body !== '', `${type} exists in generated.ts`)
    for (const field of fields) {
      ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still on the wire`)
    }
  }
  ok(
    /authored: bigint/.test(shape('BlameCommit')),
    'and `authored` is still a `bigint` — ts-rs renders `i64` that way, which is why the model ' +
      'narrows with `Number()` in exactly one place; the day it becomes a `number` that ' +
      'narrowing is dead code rather than a bug, but the reader should be told',
  )

  // --- 8. the editor surface, source-asserted ------------------------------------------------
  //
  // Every bug in this section would be a *placement* rather than a wrong function, and a module's
  // own tests cannot see one. Comments are stripped first: `EditorSurface.tsx` discusses all four
  // of these at length, and a whole-file grep would certify the prose.

  const surface = stripJs(readFileSync('src/editor/EditorSurface.tsx', 'utf8'))

  ok(
    (surface.match(/new Compartment\(\)/g) ?? []).length >= 3,
    'the surface has a THIRD compartment — language, lint and now blame — so the column can be ' +
      'switched on without rebuilding the view',
  )
  ok(
    /const shared: Extension\[\] = \[\s*blameSlot\.of\(/.test(surface),
    '`blameSlot.of(` is the FIRST entry of `shared`. CodeMirror lays gutters out in extension ' +
      'order, so this line is what puts the annotation column left of the line numbers where ' +
      'IDEA puts it; moved down one it still works and is simply in the wrong place',
  )
  ok(
    surface.indexOf('blameSlot.of(') < surface.indexOf('lineNumbers()'),
    'and it is before `lineNumbers()`, stated separately so a reordering is caught even if the ' +
      'array gains a first member',
  )
  ok(
    /\}, \[path, reloadKey\]\)/.test(surface),
    'the build effect is still keyed on exactly `[path, reloadKey]`',
  )
  eq(
    [...surface.matchAll(/\}, \[([^\]]*)\]\)/g)]
      .map((m) => m[1])
      .filter((deps) => /\bblame\b|\bblameOn\b/.test(deps) && /reloadKey/.test(deps)),
    [],
    'and NEITHER `blame` NOR `blameOn` is in it. Rebuilding the `EditorView` drops the undo ' +
      'history, the scroll position, the selection and any unsaved edits — turning a column on ' +
      'must not cost the user their work, and a refresh of it certainly must not',
  )
  ok(
    /setBlame\.of\(blameOn \? \(blame \?\? \[\]\) : null\)/.test(surface),
    'the effect CLEARS the field when `blameOn` is false. Reconfiguring the compartment away ' +
      'removes the gutter and leaves the field standing, so a hover card would outlive the ' +
      'column it hung off — the same trap the lint effect records, one worse because a tooltip ' +
      'takes the pointer',
  )
  ok(
    /slot\.reconfigure\(blameOn \? blameExt : \[\]\)/.test(surface),
    'and it reconfigures with a MEMOISED extension: a fresh `blameExtension(…)` per render mints ' +
      'a fresh `StateField`, which throws away the markers and the `touched` set on every paint',
  )

  // --- 9. the extension and the card ---------------------------------------------------------

  const ext = stripJs(readFileSync('src/editor/blame.ts', 'utf8'))
  ok(/showTooltip/.test(ext), 'the popup is a `showTooltip`')
  ok(
    !/hoverTooltip/.test(ext),
    'and NOT a `hoverTooltip`: that installs its listeners on `contentDOM`, and the gutter is not ' +
      'part of `contentDOM` — it would never see the pointer at all',
  )
  ok(
    /domEventHandlers/.test(ext),
    'so the hover comes from the gutter\'s own `domEventHandlers`',
  )
  ok(/initialSpacer/.test(ext), 'and the column has a spacer, so it does not jitter while scrolling')
  ok(
    /touched/.test(ext) && /isUserEvent\('undo'\)/.test(ext),
    'a `touched` set exists and undo is handled explicitly — an undo is itself a `docChanged` ' +
      'transaction, so the plain path would mark the very lines the undo restored and the column ' +
      'could never come back',
  )

  // --- 10. the stylesheet ---------------------------------------------------------------------

  const css = readFileSync('src/editor/EditorSurface.module.css', 'utf8')
  ok(
    css.includes(`${BLAME_LABEL_CHARS}ch`),
    `the column is sized \`${BLAME_LABEL_CHARS}ch\`, the same number as BLAME_LABEL_CHARS — a ` +
      'stylesheet that disagreed would clip the date off every label or leave dead space beside ' +
      'every line',
  )
  const missing = []
  for (let b = 0; b <= AGE_BUCKETS.length; b += 1) {
    if (!css.includes(`.cm-blame-age-${b}`)) missing.push(b)
  }
  eq(
    missing,
    [],
    'every bucket the model can emit has a tint rule. A boundary added to AGE_BUCKETS without a ' +
      'rule here is an untinted band, which reads as "these lines are all the same age"',
  )
  ok(
    !css.includes(`.cm-blame-age-${AGE_BUCKETS.length + 1}`),
    'and there is no rule for a bucket the model cannot produce',
  )
  ok(css.includes('.cm-blame-uncommitted'), 'uncommitted lines have their own tint, off the ramp')
  ok(
    css.includes('.cm-blame-touched'),
    'and so does a line this session has edited — drawn uncommitted whatever marker it carries, ' +
      'because an edited line whose anchor survived would otherwise keep the previous author\'s ' +
      'name over text they never wrote',
  )
  ok(
    /cm-gutters:has\(\.cm-blame\)/.test(css),
    'the gutter reservation GROWS by the column rather than sharing it: `.cm-lineNumbers` is ' +
      '`flex: 1` of that reservation, so without this every line number shifts left the moment ' +
      'the column appears — the bug the 70px comment already records for the lint gutter',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `blame: ok (${AGE_BUCKETS.length + 1} age buckets, ${BLAME_LABEL_CHARS}-char column, ` +
      `${markers.length}-line cover collapsed to ${markers.filter((m) => m.label !== '').length} labels)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
