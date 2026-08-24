/**
 * Renders `GitDiffView` against a fixture and prints what came out.
 *
 * Driven by `ui/scripts/check-diff-render.mjs`. Not part of the app — nothing imports it, so
 * it is tree-shaken out of the real bundle — and it exists for the reason
 * `sidebar/GitPanel/smokeEntry.tsx` does: compiling is not painting, and the claim this pane
 * has to keep is about *markup*. `check-diff-selection.mjs` proves the model maps a selection
 * to the same positions in both directions; this proves the rows the component marks are that
 * same set, so the sentence "what is highlighted is what is sent" covers the DOM too.
 *
 * The digest is deliberately positional: `selected` is the `hunk:line` of every row the view
 * drew as selected, which is the thing a user looks at.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { GitDiffView, blameLookup } from './GitDiffPane'
import { blameRefusal, type BlameLookup } from './diffBlame'
import { toSelection, resolve, type Marks } from '@/sidebar/GitPanel/diffSelection'
import type { BlameFile, DiffSide, DiffView, FileDiff, LineOrigin, RevisionDiff } from '@/ipc/client'

function line(origin: LineOrigin, content: string, oldLineno: number | null, newLineno: number | null) {
  return { origin, content, oldLineno, newLineno, noNewline: false }
}

/**
 * The same three-hunk shape `check-diff-selection.mjs` uses, so the two agree by eye.
 *
 * `newText: null`, deliberately: every digest that predates the whole-file view renders
 * against this fixture, and a `null` text is exactly the wire an old backend — or a
 * too-large file — sends, so all of those digests keep pinning the hunks-only fallback.
 * The whole-file arm has fixtures of its own below.
 */
export const FIXTURE: FileDiff = {
  path: 'src/main.rs',
  oldPath: null,
  side: 'unstaged',
  status: 'modified',
  binary: false,
  oldMode: 33188,
  newMode: 33188,
  rev: 'cafef00dcafef00d',
  partialOk: true,
  oldText: null,
  newText: null,
  textsOmitted: false,
  hunks: [
    {
      index: 0,
      header: '@@ -1,2 +1,3 @@ fn main() {',
      oldStart: 1,
      oldLines: 2,
      newStart: 1,
      newLines: 3,
      lines: [
        line('context', 'fn main() {', 1, 1),
        line('addition', '    let x = 1;', null, 2),
        line('context', '}', 2, 3),
      ],
    },
    {
      index: 1,
      header: '@@ -10,3 +11,3 @@',
      oldStart: 10,
      oldLines: 3,
      newStart: 11,
      newLines: 3,
      lines: [
        line('context', 'let a = 0;', 10, 11),
        line('deletion', 'let b = 1;', 11, null),
        line('addition', 'let b = 2;', null, 12),
        line('context', 'let c = 3;', 12, 13),
      ],
    },
    {
      index: 2,
      header: '@@ -20,1 +21,4 @@',
      oldStart: 20,
      oldLines: 1,
      newStart: 21,
      newLines: 4,
      lines: [
        line('addition', 'one', null, 21),
        line('addition', 'two', null, 22),
        line('addition', 'three', null, 23),
        line('context', 'end', 20, 24),
      ],
    },
  ],
}

/**
 * An addition immediately *followed* by a deletion, which the three-hunk fixture never has.
 *
 * `diffRows.columnRows` treats `+` then `-` as two separate edits and refuses to group across
 * the boundary, because git emits `-` before `+` within one edit — so a `+` already closed the
 * edit it belonged to. Nothing in `FIXTURE` can tell that rule from its absence: it only ever
 * has `-` before `+`, where flushing and not flushing produce identical runs. This one fixture
 * is the difference between the rule being checked and merely being written down.
 */
export const PAIR_FIXTURE: FileDiff = {
  ...FIXTURE,
  path: 'src/pair.rs',
  hunks: [
    {
      index: 0,
      header: '@@ -1,3 +1,3 @@',
      oldStart: 1,
      oldLines: 3,
      newStart: 1,
      newLines: 3,
      lines: [
        line('context', 'keep', 1, 1),
        line('addition', 'added first', null, 2),
        line('deletion', 'removed after', 2, null),
        line('context', 'tail', 3, 3),
      ],
    },
  ],
}

/* --- the whole-file fixtures (M25) -------------------------------------------------------- */

/**
 * `FIXTURE`'s hunks with the texts they came from: a 24-line new side whose gaps — new
 * 4–10 and 14–20 — are what the whole-file view has to fill in. The arithmetic matters more
 * than the words: `wholeFileSegments` validates every context and addition line against
 * this text and refuses the file on any disagreement, so a fixture whose numbers were off
 * by one would silently pin the *fallback* and prove nothing about the feature.
 */
const WHOLE_TEXT =
  [
    'fn main() {',
    '    let x = 1;',
    '}',
    'g4',
    'g5',
    'g6',
    'g7',
    'g8',
    'g9',
    'g10',
    'let a = 0;',
    'let b = 2;',
    'let c = 3;',
    'g14',
    'g15',
    'g16',
    'g17',
    'g18',
    'g19',
    'g20',
    'one',
    'two',
    'three',
    'end',
  ].join('\n') + '\n'

/** The old side, 20 lines. Not consumed by the row model; carried because a real wire has it. */
const WHOLE_OLD =
  [
    'fn main() {',
    '}',
    'g4',
    'g5',
    'g6',
    'g7',
    'g8',
    'g9',
    'g10',
    'let a = 0;',
    'let b = 1;',
    'let c = 3;',
    'g14',
    'g15',
    'g16',
    'g17',
    'g18',
    'g19',
    'g20',
    'end',
  ].join('\n') + '\n'

export const WHOLE: FileDiff = {
  ...FIXTURE,
  path: 'src/whole.rs',
  oldText: WHOLE_OLD,
  newText: WHOLE_TEXT,
}

/**
 * The same hunks with a context line the text contradicts — `let a = 0;` is `DIFFERENT` in
 * the text. The whole-file view must refuse this and render exactly what `FIXTURE` renders:
 * the check compares the two digests field for field.
 */
export const WHOLE_MISMATCH: FileDiff = {
  ...WHOLE,
  path: 'src/mismatch.rs',
  newText: WHOLE_TEXT.replace('let a = 0;', 'DIFFERENT'),
}

/**
 * A 6,000-line file — over `WHOLE_FILE_COLLAPSE_ABOVE` — with one change at the top, so the
 * single 5,998-line gap must fold behind an expander row. The `Open` variant renders the
 * same fixture with gap 0 expanded, which is the state one click produces.
 */
const BIG_LINES = Array.from({ length: 6_000 }, (_, i) => `l${i + 1}`)
export const WHOLE_BIG: FileDiff = {
  ...FIXTURE,
  path: 'src/big.rs',
  oldText: BIG_LINES.slice(1).join('\n') + '\n',
  newText: BIG_LINES.join('\n') + '\n',
  hunks: [
    {
      index: 0,
      header: '@@ -1,1 +1,2 @@',
      oldStart: 1,
      oldLines: 1,
      newStart: 1,
      newLines: 2,
      lines: [line('addition', 'l1', null, 1), line('context', 'l2', 1, 2)],
    },
  ],
}

/**
 * A block added at line 1, a deletion in the middle, a block added at end of file — the
 * three marker positions the split view has to get right: before the first row, between
 * rows, and *after the last row*, which is the boundary a marker keyed on "the following
 * row" would not have.
 */
export const EDGE: FileDiff = {
  ...FIXTURE,
  path: 'src/edge.rs',
  oldText: 'mid1\ngone\nmid2\n',
  newText: 'first\nmid1\nmid2\nlast\n',
  hunks: [
    {
      index: 0,
      header: '@@ -1,3 +1,4 @@',
      oldStart: 1,
      oldLines: 3,
      newStart: 1,
      newLines: 4,
      lines: [
        line('addition', 'first', null, 1),
        line('context', 'mid1', 1, 2),
        line('deletion', 'gone', 2, null),
        line('context', 'mid2', 3, 3),
        line('addition', 'last', null, 4),
      ],
    },
  ],
}

/* --- the blame column's fixture (M18) ----------------------------------------------------- */

const DAY = 86_400
/** 2026-08-19 12:00:00Z. Fixed, so the age buckets do not depend on the minute CI ran. */
const NOW = Date.UTC(2026, 7, 19, 12, 0, 0) / 1000

/**
 * A blame of `FIXTURE`'s **new** side, as `git_blame` would answer it.
 *
 * A real `BlameFile` rather than a hand-written `BlameLookup`, and for the reason
 * [`REVISION_FIXTURE`] states about its own shape: a fixture in the frontend's own vocabulary
 * renders perfectly and proves nothing about what the backend sends. It goes through
 * `blameLookup` — the same function the pane calls — so `collapseRuns`' cover check and the age
 * ramp are exercised here rather than assumed.
 *
 * The runs are chosen to put one of each kind of answer against a diff row:
 *
 *   * `1–5` a commit an hour old (bucket 0), covering hunk 0's whole span (new 1, 2, 3);
 *   * `6–11` a commit forty days old (bucket 3), covering hunk 1's first context line;
 *   * `12` **uncommitted**, covering hunk 1's addition — which is what a working-tree blame says
 *     about a line that is not staged and not committed;
 *   * `13–20` the forty-day commit again, covering hunk 1's trailing context;
 *   * `21–30` a commit over a year old (bucket 5), covering all of hunk 2 — *including its three
 *     additions*, which is the shape a blame at `new_rev` produces: on a historical diff the
 *     added lines are committed, by the commit the tab is showing.
 */
const BLAME_FIXTURE: BlameFile = {
  path: FIXTURE.path,
  head: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678',
  lines: 30,
  runs: [
    { start: 1, lines: 5, commit: 0 },
    { start: 6, lines: 6, commit: 1 },
    { start: 12, lines: 1, commit: null },
    { start: 13, lines: 8, commit: 1 },
    { start: 21, lines: 10, commit: 2 },
  ],
  commits: [
    {
      oid: 'aaa1111aaa1111aaa1111aaa1111aaa1111aaa11',
      shortOid: 'aaa1111',
      summary: 'Teach main about x',
      author: 'Ada Lovelace',
      authorEmail: 'ada@example.org',
      authored: BigInt(NOW - 3_600),
      origPath: null,
      boundary: false,
    },
    {
      oid: 'bbb2222bbb2222bbb2222bbb2222bbb2222bbb22',
      shortOid: 'bbb2222',
      summary: 'Rename the counters',
      author: 'Grace Hopper',
      authorEmail: 'grace@example.org',
      authored: BigInt(NOW - 40 * DAY),
      origPath: null,
      boundary: false,
    },
    {
      oid: 'ccc3333ccc3333ccc3333ccc3333ccc3333ccc33',
      shortOid: 'ccc3333',
      summary: 'Initial import',
      author: 'Alan Turing',
      authorEmail: 'alan@example.org',
      authored: BigInt(NOW - 400 * DAY),
      origPath: null,
      boundary: false,
    },
  ],
  source: 'working',
  dirty: true,
  follow: 'renames',
  downgraded: null,
}

const BLAME: BlameLookup | null = blameLookup(BLAME_FIXTURE, NOW)

/**
 * The same blame with a **hole** at line 6, which `collapseRuns` must refuse outright.
 *
 * `blameLookup` answers `null` for it, so the pane is handed the same value it holds while a
 * fetch is in flight and draws no column at all. That is the whole point: a run set with a hole
 * paints a column that is silently one line off for everything below it, with every row still
 * carrying a plausible oid and nothing on screen to notice.
 */
const BLAME_BROKEN: BlameLookup | null = blameLookup(
  { ...BLAME_FIXTURE, runs: [{ start: 1, lines: 4, commit: 0 }, { start: 6, lines: 25, commit: 1 }] },
  NOW,
)

export interface DiffDigest {
  name: string
  /** `hunk:line` of every row drawn as selected, in document order. */
  selected: string[]
  /** The same, as the wire `Selection` resolves it — these two must be equal. */
  sent: string[]
  /** `data-state` of each hunk's tri-state box, in order. */
  hunkBoxes: string[]
  rows: number
  count: string | null
  applyLabel: string | null
  applyDisabled: boolean
  note: string | null
  /** The `from → to` the read-only header names, or `null` in the staging arm. */
  revisions: string | null
  /** Whether the side switcher is drawn. Must be false for a diff of two commits. */
  sideControl: boolean
  /** Whether any per-line tick box is drawn. Must be false for a diff of two commits. */
  lineBox: boolean
  /**
   * Every `hunk:line` the view wrote, selected or not, in document order.
   *
   * The split layout draws a context line *twice* — once per column — and only one of the two
   * carries the position, so this is what pins that rule: a repeat here means the same mark is
   * in the DOM twice and "the rows drawn as selected" has stopped being a set of positions.
   */
  positions: string[]
  /**
   * The positions split per column — the left (old) column's in its file order, then the
   * right's. Both empty in the unified layout. Document order across the whole markup stopped
   * meaning file order when the columns became sequential in the DOM, so the order claims
   * live here now.
   */
  leftPositions: string[]
  rightPositions: string[]

  /* --- the whole-file view (M25) --- */

  /** `data-count` of every unchanged-gap wrapper, in document order. */
  gaps: string[]
  /** `data-count` of every fold (expander) row, in document order. */
  folds: string[]
  /** `data-tone` of every insertion marker, per column. Empty outside the split layout. */
  insertLeft: string[]
  insertRight: string[]

  /* --- the blame column (M18) --- */

  /**
   * `data-blame` of every blame cell, in document order. `''` where the row has no attribution
   * — a deletion, or a line the blame does not reach.
   *
   * An empty array means the column is not in the markup **at all**, which is a stronger claim
   * than a zero-width one: an unannotated diff must not pay 22 characters of margin for a column
   * that is not there.
   */
  blameCells: string[]
  /** `data-age` of every blame cell, in the same order. `'-1'` is uncommitted, `''` is absent. */
  blameAges: string[]
  /** Whether the row grid reserved a track for the column. */
  blameTrack: boolean
  blamePressed: boolean
  blameDisabled: boolean
  blameTitle: string
}

/** Every blame cell's `data-blame` and `data-age`, in document order. */
function blameOf(html: string): { cells: string[]; ages: string[] } {
  const found = [
    ...html.matchAll(/data-audit="gitDiffBlame" data-blame="([^"]*)" data-age="([^"]*)"/g),
  ]
  return {
    cells: found.map((m) => m[1] ?? ''),
    ages: found.map((m) => m[2] ?? ''),
  }
}

/** The positional row regex, over one slice of the markup. */
function rowsOf(html: string): Array<RegExpMatchArray> {
  return [...html.matchAll(/data-audit="gitDiffRow" data-at="([\d:]+)" data-selected="(\w+)"/g)]
}

/**
 * The whole-file and split facts, from one render. The column boundary is the right
 * column's `data-side="new"` — everything before it is the header plus the left column,
 * and no row or marker exists outside a column in the split layout.
 */
function layoutOf(html: string): {
  leftPositions: string[]
  rightPositions: string[]
  gaps: string[]
  folds: string[]
  insertLeft: string[]
  insertRight: string[]
} {
  const boundary = html.indexOf('data-side="new"')
  const left = boundary === -1 ? '' : html.slice(0, boundary)
  const right = boundary === -1 ? '' : html.slice(boundary)
  const tones = (part: string): string[] =>
    [...part.matchAll(/data-audit="gitDiffInsertMark" data-tone="(\w+)"/g)].map((m) => m[1] ?? '')
  return {
    leftPositions: rowsOf(left).map((m) => m[1] ?? ''),
    rightPositions: rowsOf(right).map((m) => m[1] ?? ''),
    gaps: [...html.matchAll(/data-audit="gitDiffGap" data-count="(\d+)"/g)].map((m) => m[1] ?? ''),
    folds: [...html.matchAll(/data-audit="gitDiffExpand" data-count="(\d+)"/g)].map(
      (m) => m[1] ?? '',
    ),
    insertLeft: tones(left),
    insertRight: tones(right),
  }
}

/** The toggle's three visible facts. */
function blameToggleOf(html: string): { pressed: boolean; disabled: boolean; title: string } {
  const found = /<button([^>]*)data-audit="gitDiffBlameToggle"([^>]*)>/.exec(html)
  const before = found?.[1] ?? ''
  return {
    pressed: /aria-pressed="true"/.test(before),
    disabled: (found?.[2] ?? '').includes('disabled'),
    title: /title="([^"]*)"/.exec(before)?.[1] ?? '',
  }
}

/**
 * The same hunks as [`FIXTURE`], as a `RevisionDiff`.
 *
 * A real `RevisionDiff` and not a cast of the `FileDiff`, deliberately. The read-only arm narrows
 * `diff` to this type, and the whole reason the fixtures in this project are written in the
 * generated shapes is the git panel's "renders perfectly, empty against every real repository"
 * failure — a cast would make this render and prove nothing about the shape the backend sends.
 * Sharing the hunks is what keeps the two arms comparable row for row.
 */
const REVISION_FIXTURE: RevisionDiff = {
  path: FIXTURE.path,
  oldPath: FIXTURE.oldPath,
  new: { kind: 'commit', oid: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678' },
  old: { kind: 'commit', oid: '9f8e7d6c5b4a39281706f5e4d3c2b1a098765432' },
  newOid: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678',
  oldOid: '9f8e7d6c5b4a39281706f5e4d3c2b1a098765432',
  status: FIXTURE.status,
  binary: FIXTURE.binary,
  oldMode: FIXTURE.oldMode,
  newMode: FIXTURE.newMode,
  hunks: FIXTURE.hunks,
  oldText: null,
  newText: null,
  textsOmitted: false,
}

/** The read-only arm with the texts, so the whole-file view is proved on frozen history too. */
const REVISION_WHOLE: RevisionDiff = {
  ...REVISION_FIXTURE,
  oldText: WHOLE_OLD,
  newText: WHOLE_TEXT,
}

function digest(
  name: string,
  marks: Marks,
  over: {
    diff: FileDiff | null
    side: DiffSide
    note?: string
    reason?: string
    held?: number
    /** Defaults to the unified layout, which is what `GitDiffView` defaults to. */
    view?: DiffView
    /**
     * Present ⇒ the column is on; `null` ⇒ on with nothing to draw yet. Absent ⇒ off.
     *
     * Spelled with `in` rather than with a sentinel because the *absence* of the prop is one of
     * the three states the view distinguishes, and that is exactly what the fixture has to be
     * able to reproduce.
     */
    blame?: BlameLookup | null
    /** Gaps opened in a folded whole-file view. */
    expanded?: ReadonlySet<number>
  },
): DiffDigest {
  const html = renderToStaticMarkup(
    <GitDiffView
      diff={over.diff}
      path="src/main.rs"
      side={over.side}
      marks={marks}
      collapsed={new Set()}
      busy={false}
      note={over.note ?? null}
      reason={over.reason ?? null}
      held={over.held ?? null}
      {...(over.view === undefined ? {} : { view: over.view })}
      {...('blame' in over ? { blame: over.blame } : {})}
      // The wiring's own rule, reproduced rather than restated: `GitDiff` withholds the handler
      // for exactly the sides `blameRefusal` names, and the view disables the toggle for them
      // again on its own. A fixture that always passed a handler would prove only the second
      // half.
      {...(blameRefusal(over.side) === null ? { onBlame: () => {} } : {})}
      {...(over.expanded === undefined ? {} : { expandedGaps: over.expanded })}
      onExpandGap={() => {}}
      onSide={() => {}}
      onMarks={() => {}}
      onCollapse={() => {}}
      onApply={() => {}}
      onDropHeld={() => {}}
    />,
  )
  const blame = blameOf(html)
  const toggle = blameToggleOf(html)
  const rows = rowsOf(html)
  const apply = /data-audit="gitDiffApply"([^>]*)>([^<]*)</.exec(html)
  return {
    name,
    selected: rows.flatMap((m) => (m[2] === 'true' ? [m[1] ?? ''] : [])),
    sent: over.diff === null ? [] : [...resolve(over.diff, toSelection(marks, over.diff))].sort(),
    hunkBoxes: [...html.matchAll(/data-audit="gitDiffHunkBox" data-state="(\w+)"/g)].map(
      (m) => m[1] ?? '',
    ),
    rows: rows.length,
    count: /data-audit="gitDiffCount"[^>]*>([\s\S]*?)<\/span>/.exec(html)?.[1]?.replace(/<[^>]*>/g, '') ?? null,
    applyLabel: apply?.[2] ?? null,
    applyDisabled: apply?.[1]?.includes('disabled') ?? false,
    note: /data-audit="gitDiffNote"[^>]*>([^<]*)</.exec(html)?.[1] ?? null,
    revisions:
      /data-audit="gitDiffRevisions"[^>]*>([\s\S]*?)<\/span>/
        .exec(html)?.[1]
        ?.replace(/<[^>]*>/g, '')
        .trim() ?? null,
    sideControl: html.includes('data-audit="gitDiffSide"'),
    lineBox: html.includes('data-audit="gitDiffLineBox"'),
    positions: rows.map((m) => m[1] ?? ''),
    ...layoutOf(html),
    blameCells: blame.cells,
    blameAges: blame.ages,
    blameTrack: html.includes('data-blamed="true"'),
    blamePressed: toggle.pressed,
    blameDisabled: toggle.disabled,
    blameTitle: toggle.title,
  }
}

/**
 * The read-only arm: a file as one commit left it, against another revision.
 *
 * A second render function rather than a flag on [`digest`], because the two arms of
 * `GitDiffViewProps` are a **discriminated union** — the staging arm's `onApply`, `onMarks`,
 * `onSide` and `side` do not exist here, and that is the whole point of the union rather than a
 * boolean plus optional handlers. A shared builder would have to pass them and then hope they
 * were ignored, which is exactly the shape the union was chosen to make unrepresentable.
 *
 * What this pins is one sentence: **a diff of two commits offers no control that writes.** The
 * Stage/Unstage/Commit button, the side switcher and the per-line tick boxes are all absent, and
 * they are absent from the markup rather than merely disabled — a greyed Stage button over a
 * commit from 2019 would still be a control that has no meaning there.
 */
function readOnlyDigest(
  name: string,
  diff: RevisionDiff | null,
  view?: DiffView,
  over: { blame?: BlameLookup | null } = {},
): DiffDigest {
  const html = renderToStaticMarkup(
    <GitDiffView
      readOnly
      revisions={{ from: '9f8e7d6', to: 'a1b2c3d' }}
      diff={diff}
      path="src/main.rs"
      collapsed={new Set()}
      busy={false}
      note={null}
      reason={null}
      {...(view === undefined ? {} : { view })}
      {...('blame' in over ? { blame: over.blame } : {})}
      // Always offered on this arm. `RevisionDiffPane` blames at `new_rev`, so there is no side
      // here that cannot be answered — see its comment for why the read-only arm shows the column
      // at all.
      onBlame={() => {}}
      onCollapse={() => {}}
      onExpandGap={() => {}}
    />,
  )
  const blame = blameOf(html)
  const toggle = blameToggleOf(html)
  const rows = rowsOf(html)
  const apply = /data-audit="gitDiffApply"([^>]*)>([^<]*)</.exec(html)
  return {
    name,
    selected: rows.flatMap((m) => (m[2] === 'true' ? [m[1] ?? ''] : [])),
    sent: [],
    hunkBoxes: [...html.matchAll(/data-audit="gitDiffHunkBox" data-state="(\w+)"/g)].map(
      (m) => m[1] ?? '',
    ),
    rows: rows.length,
    count: null,
    applyLabel: apply?.[2] ?? null,
    applyDisabled: apply?.[1]?.includes('disabled') ?? false,
    note: /data-audit="gitDiffNote"[^>]*>([^<]*)</.exec(html)?.[1] ?? null,
    revisions:
      /data-audit="gitDiffRevisions"[^>]*>([\s\S]*?)<\/span>/
        .exec(html)?.[1]
        ?.replace(/<[^>]*>/g, '')
        .trim() ?? null,
    sideControl: html.includes('data-audit="gitDiffSide"'),
    lineBox: html.includes('data-audit="gitDiffLineBox"'),
    positions: rows.map((m) => m[1] ?? ''),
    ...layoutOf(html),
    blameCells: blame.cells,
    blameAges: blame.ages,
    blameTrack: html.includes('data-blamed="true"'),
    blamePressed: toggle.pressed,
    blameDisabled: toggle.disabled,
    blameTitle: toggle.title,
  }
}

const digests: DiffDigest[] = [
  digest('empty', new Set(), { diff: FIXTURE, side: 'unstaged' }),
  digest('oneLine', new Set(['1:2']), { diff: FIXTURE, side: 'unstaged' }),
  digest('wholeHunk', new Set(['2:0', '2:1', '2:2']), { diff: FIXTURE, side: 'unstaged' }),
  digest('ragged', new Set(['0:1', '2:0', '2:2']), { diff: FIXTURE, side: 'staged' }),
  digest('everything', new Set(['0:1', '1:1', '1:2', '2:0', '2:1', '2:2']), {
    diff: FIXTURE,
    side: 'combined',
  }),
  // A context mark and a mark past the end of the diff: neither may light a row.
  digest('junk', new Set(['0:0', '9:9']), { diff: FIXTURE, side: 'unstaged' }),
  digest('binary', new Set(['0:1']), {
    diff: { ...FIXTURE, partialOk: false },
    side: 'unstaged',
  }),
  digest('gone', new Set(), { diff: null, side: 'staged', reason: 'src/main.rs has no changes' }),

  /*
   * The same claims again in the side-by-side layout.
   *
   * Without these the split view is drawn by code no check can fail: every case above renders
   * with `view` unset, which is `'unified'`, so `columnRows` and the `cell()` calls that place
   * its rows were reachable from the app and from nothing that runs in CI. The pane exists to
   * keep one promise — what is highlighted is what is staged — and that promise has to be
   * proved in both layouts or the layout switch is the place it silently stops holding.
   */
  digest('splitEmpty', new Set(), { diff: FIXTURE, side: 'unstaged', view: 'split' }),
  digest('splitRagged', new Set(['0:1', '2:0', '2:2']), {
    diff: FIXTURE,
    side: 'staged',
    view: 'split',
  }),
  digest('splitWholeHunk', new Set(['2:0', '2:1', '2:2']), {
    diff: FIXTURE,
    side: 'unstaged',
    view: 'split',
  }),
  digest('splitEverything', new Set(['0:1', '1:1', '1:2', '2:0', '2:1', '2:2']), {
    diff: FIXTURE,
    side: 'combined',
    view: 'split',
  }),
  digest('splitJunk', new Set(['0:0', '9:9']), { diff: FIXTURE, side: 'unstaged', view: 'split' }),

  /*
   * The pairing boundary, in both layouts so the two can be compared.
   *
   * `positions` is in *document order* here and that is the point: the sorted comparisons above
   * cannot see a regrouping, only a gain or loss. Facing `added first` against `removed after`
   * — one edit's `+` against the next edit's `-` — reorders the columns without changing the
   * set, and reading a diff that claims one line became another when it did not is exactly the
   * wrong answer this pane must not give.
   */
  digest('pairUnified', new Set(['0:1']), { diff: PAIR_FIXTURE, side: 'unstaged' }),
  digest('pairSplit', new Set(['0:1']), { diff: PAIR_FIXTURE, side: 'unstaged', view: 'split' }),
  /*
   * The read-only arm: a file as one commit left it.
   *
   * The claim is one sentence — **a diff of two commits offers no control that writes** — and it
   * is checked against real markup rather than by reading the union, because the union only
   * guarantees the *handlers* are absent. Whether the buttons are drawn is a separate question,
   * and a greyed Stage button over a commit from 2019 would still be a control with no meaning
   * there. Both layouts, because the layout switch is exactly where a rule silently stops
   * holding.
   */
  readOnlyDigest('revision', REVISION_FIXTURE),
  readOnlyDigest('revisionSplit', REVISION_FIXTURE, 'split'),
  readOnlyDigest('revisionGone', null),

  /*
   * The blame column. (M18)
   *
   * `blame` is the same fixture in both layouts and in both arms, so the four cases can be
   * compared against each other rather than each against a list of strings: the split layout must
   * name the same commits the unified one does *minus the deletion*, and the read-only arm must
   * name exactly what the staging arm does, because a column that changed its answer with the
   * layout or with the presence of a tick box would be the kind of wrong nobody sees.
   */
  digest('blame', new Set(), { diff: FIXTURE, side: 'unstaged', blame: BLAME }),
  digest('blameSplit', new Set(), {
    diff: FIXTURE,
    side: 'unstaged',
    view: 'split',
    blame: BLAME,
  }),
  /* On with nothing to draw: the fetch is in flight, or its run set was refused. */
  digest('blamePending', new Set(), { diff: FIXTURE, side: 'unstaged', blame: BLAME_BROKEN }),
  /*
   * The staged side, whose new text is the index — see `diffBlame.blameRefusal`.
   *
   * No `blame` prop, because that is the true state and not a simplification: the wiring never
   * fetches for this side, so there is nothing to hand the view. What is asserted is the pair —
   * a disabled toggle carrying the reason, and no column.
   */
  digest('blameStaged', new Set(), { diff: FIXTURE, side: 'staged' }),
  readOnlyDigest('revisionBlame', REVISION_FIXTURE, undefined, { blame: BLAME }),

  /*
   * The whole-file view. (M25)
   *
   * The selection claims are the same claims as ever — `wholeUnified` must carry exactly the
   * positions `empty` does, because gap rows have none — plus the new facts: the gaps drawn,
   * the folds above the size threshold, the insertion markers per column, and the mismatch
   * fixture falling back to a render indistinguishable from the hunks-only one.
   */
  digest('wholeUnified', new Set(['1:2']), { diff: WHOLE, side: 'unstaged' }),
  digest('wholeSplit', new Set(['1:2']), { diff: WHOLE, side: 'unstaged', view: 'split' }),
  readOnlyDigest('wholeRevision', REVISION_WHOLE),
  readOnlyDigest('wholeRevisionSplit', REVISION_WHOLE, 'split'),
  digest('wholeBig', new Set(), { diff: WHOLE_BIG, side: 'unstaged' }),
  digest('wholeBigOpen', new Set(), {
    diff: WHOLE_BIG,
    side: 'unstaged',
    expanded: new Set([0]),
  }),
  digest('wholeMismatch', new Set(), { diff: WHOLE_MISMATCH, side: 'unstaged' }),
  digest('edgeSplit', new Set(), { diff: EDGE, side: 'unstaged', view: 'split' }),
  digest('wholeBlame', new Set(), { diff: WHOLE, side: 'unstaged', blame: BLAME }),
  digest('wholeBlameSplit', new Set(), {
    diff: WHOLE,
    side: 'unstaged',
    view: 'split',
    blame: BLAME,
  }),
]

console.log(JSON.stringify(digests))
