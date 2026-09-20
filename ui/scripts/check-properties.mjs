/**
 * Checks `src/chrome/filePropertiesModel.ts` — every sentence and every format on the file
 * properties card. (M70)
 *
 * The card is almost entirely *claims about a file*, which is a shape where being wrong looks
 * exactly like being right: a blank row, a plausible date, a green tick. None of it throws and
 * none of it appears in a render digest, so the things worth pinning are the ones where a wrong
 * answer is indistinguishable from a correct one on screen.
 *
 * Six of those, and each has a section below:
 *
 *  1. **An absent fact gets a sentence.** Every optional Rust sends can be missing for a reason
 *     the reader can act on. A blank next to a label reads as a broken card.
 *  2. **A bounded count never looks exact.** `at least` appears if and only if the walk was
 *     truncated. An exact-looking lower bound is worse than no number in a panel whose only job
 *     is being believed.
 *  3. **An unknown git status is not a clean one.** `TreeStatusMap` is capped, and absence from
 *     a *truncated* map means nothing at all. A green *Clean* on a modified file is the one
 *     wrong answer here that costs the user work.
 *  4. **Never committed and too-much-history are different answers.** Collapsing them tells
 *     somebody their file is untracked because their repository is large.
 *  5. **`more` is not `length >= limit`.** The off-by-one draws a *there is more* footnote onto
 *     a file whose entire history is already on screen.
 *  6. **Singular and plural.** `1 files` in a dialog survives for years because nobody owns it.
 *
 * Same shape as `check-fs-clipboard.mjs` and `check-close-confirm.mjs` — there is no JS test
 * runner in this project, and this is a pure module the TypeScript in `node_modules` can compile
 * on its own. It is not import-free: it imports `../panes/imageKinds`, which *is*, so `tsc`
 * follows the relative path and compiles both. That reuse is the point — `formatBytes` had
 * already been written three times in this tree and two of the copies had drifted apart (one
 * rounds a KiB, the other floors it), so a fourth was not on the table.
 *
 * Run: `pnpm --dir ui run check:properties`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-properties-'))

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/filePropertiesModel.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // CommonJS + node10 for `check-fs-clipboard.mjs`'s reason: this module has a relative
      // import, the app compiles under `bundler` resolution where an extensionless specifier is
      // normal, and ESM output would keep it extensionless for a node that refuses to load it.
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      // Both flags the app's own tsconfig sets. `exactOptionalPropertyTypes` is the one that
      // matters here: it is what makes `modeString?: string | undefined` refuse a `null`, which
      // is exactly the wire shape the Rust side's `skip_serializing_if` pairing guarantees
      // against.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
      '--lib', 'es2023',
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const m = require(join(out, 'chrome', 'filePropertiesModel.js'))

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

  /** A minimal file, every optional absent — the shape a stat of a bare filesystem gives. */
  const file = (over = {}) => ({
    path: '/home/u/work/cide/ui/src/editor/changeModel.ts',
    name: 'changeModel.ts',
    kind: 'file',
    len: 7412,
    readonly: false,
    ...over,
  })

  // ---- 1. an absent fact gets a sentence, never a blank ------------------------------------

  eq(m.formatTime(undefined), m.NOT_RECORDED, 'a missing time says so')
  eq(m.formatTime(Number.NaN), m.NOT_RECORDED, 'a NaN time says so')
  eq(m.formatOwner(undefined), null, 'a missing owner is null, for the row to be dropped')
  eq(m.formatSize(-1), m.NOT_RECORDED, 'a negative size is not rendered as a number')
  eq(m.formatSize(Number.POSITIVE_INFINITY), m.NOT_RECORDED, 'an infinite size says so')

  // The producing side guarantees exactly one of `text`/`textSkipped` for a file, so the row is
  // never blank — `cide_core::properties`'s own test pins the other half of this.
  ok(
    m.textLine(file({ textSkipped: 'Binary — no lines to count.' })) ===
      'Binary — no lines to count.',
    'a file with no countable text says why',
  )
  ok(
    m.textLine(file({ text: { lines: 214, ending: 'lf', utf8: true } })) ===
      '214 lines · LF · UTF-8',
    'a text file states its three facts',
  )
  // A directory has no lines and needs no apology for not having them.
  eq(m.textLine({ ...file(), kind: 'dir' }), null, 'a folder gets no text row at all')

  // Non-UTF-8 is the one case `document::read` refuses outright, so this row has to explain
  // rather than label — it is the only place in cide that can.
  ok(
    m.textLine(file({ text: { lines: 4, ending: 'lf', utf8: false } })).includes('will not open'),
    'an invalid-UTF-8 file explains why it will not open',
  )

  // A symlink is announced before anything else, because every other row is about the link.
  eq(m.kindLabel(file({ kind: 'symlink' })), 'Symlink', 'a symlink says so')
  eq(
    m.kindLabel(file({ kind: 'symlink', symlinkBroken: true })),
    'Broken symlink',
    'a dangling symlink says so, since its other rows are about a target that is not there',
  )
  eq(m.kindLabel(file({ kind: 'other' })), 'Special file', 'a fifo is named, not called a file')
  eq(m.kindLabel(file({ kind: 'dir' })), 'Folder', 'a directory is a folder')

  // ---- the size row ------------------------------------------------------------------------

  eq(m.formatSize(7412), '7,412 B (7 KiB)', 'exact first, human in brackets')
  eq(m.formatSize(512), '512 B', 'under a KiB the bracket would repeat the number')
  eq(m.formatSize(0), '0 B', 'an empty file is 0 B and not "Not recorded"')
  ok(m.formatSize(5 * 1024 * 1024).includes('5.0 MiB'), 'megabytes keep one decimal')

  // ---- 2. a bounded count never looks exact ------------------------------------------------

  eq(
    m.entriesLine({ files: 42, dirs: 7, bytes: 0, truncated: false }),
    '42 files, 7 folders',
    'an exact walk states its counts plainly',
  )
  ok(
    m.entriesLine({ files: 50000, dirs: 3001, bytes: 0, truncated: true }).startsWith('at least '),
    'a truncated walk hedges — an exact-looking lower bound is the failure DirSummary names',
  )
  ok(
    m.dirSizeLine({ files: 0, dirs: 0, bytes: 3500000, truncated: true }).startsWith('at least '),
    'and the size is hedged by the same flag',
  )
  eq(
    m.entriesLine(null),
    null,
    'a walk still running is null, never "0 files" — which is a real answer for an empty folder',
  )
  eq(m.dirSizeLine(null), null, 'same for the size row')
  eq(m.formatCount(4096, 'file', true), 'at least 4,096 files', 'formatCount hedges and separates')
  eq(m.formatCount(4096, 'file', false), '4,096 files', 'and does not hedge when it need not')

  // ---- 3. an unknown git status is not a clean one -----------------------------------------

  ok(
    m.statusLabel(null, false).startsWith('Clean'),
    'absence from a complete map genuinely means clean — that is the map’s encoding',
  )
  eq(
    m.statusLabel(null, true),
    m.STATUS_UNKNOWN,
    'absence from a TRUNCATED map means nothing at all; a green tick here is the costly lie',
  )
  eq(
    m.statusLabel('clean', true),
    m.STATUS_UNKNOWN,
    'an explicit clean from a truncated map is no better evidence than an absent one',
  )
  eq(m.statusLabel('modified', false), 'Modified', 'a known status is named')
  eq(m.statusLabel('ignored', false), 'Ignored by git', 'ignored says who is ignoring it')
  // A status a newer build reports renders as itself rather than as a blank — `cide_docker`'s
  // "carried as a String and never an enum" rule, which exists so a new daemon state is visible.
  eq(m.statusLabel('somethingNew', false), 'somethingNew', 'an unknown status renders as itself')

  // ---- 4. never committed is not the same as too much history ------------------------------

  const git = (over = {}) => ({
    relPath: 'ui/src/editor/changeModel.ts',
    firstTruncated: false,
    recent: [],
    more: false,
    ...over,
  })
  const commit = (over = {}) => ({
    oid: 'a'.repeat(40),
    shortOid: 'aaaaaaa',
    summary: 'a commit',
    author: 'Ivan Vorontsov',
    authored: 1700000000,
    ...over,
  })

  eq(
    m.trackedSince(git({ firstCommit: undefined, firstTruncated: false })),
    'Never committed',
    'no first commit and nothing truncated means the file has no history',
  )
  ok(
    m.trackedSince(git({ firstCommit: undefined, firstTruncated: true })).includes('More history'),
    'no first commit because the walk gave up is a DIFFERENT answer — collapsing them tells a '
      + 'user their file is untracked because their repository is large',
  )
  ok(
    m.trackedSince(git({ firstCommit: commit() })).includes('2023'),
    'a found first commit renders its date',
  )
  eq(m.trackedSince(null), null, 'no repository means no row')

  // ---- 5. `more` is the extra row, never the length ----------------------------------------

  const ten = Array.from({ length: 10 }, (_, i) => commit({ summary: `c${i}` }))
  eq(
    m.moreLine(git({ recent: ten, more: false }), 10),
    null,
    'ten commits with nothing beyond them draws no footnote — this is the off-by-one that would '
      + 'otherwise claim there is more on a file whose whole history is on screen',
  )
  ok(
    m.moreLine(git({ recent: ten, more: true }), 10) !== null,
    'ten commits with an eleventh behind them does draw one',
  )
  eq(m.moreLine(null, 0), null, 'no repository, no footnote')

  // ---- the buttons -------------------------------------------------------------------------

  ok(!m.canOpenHistory(null), 'no repository offers no history button')
  ok(
    !m.canOpenHistory(git({ recent: [] })),
    'a path with no history offers no button — the tab would open onto the same sentence the '
      + 'card already shows',
  )
  ok(m.canOpenHistory(git({ recent: [commit()] })), 'a path with history offers the button')
  // ---- 6. singular and plural ---------------------------------------------------------------

  eq(m.formatCount(1, 'file', false), '1 file', 'one file, not "1 files"')
  eq(m.formatCount(0, 'file', false), '0 files', 'zero is plural')
  eq(
    m.entriesLine({ files: 1, dirs: 1, bytes: 0, truncated: false }),
    '1 file, 1 folder',
    'both nouns agree in number',
  )
  eq(
    m.textLine(file({ text: { lines: 1, ending: 'lf', utf8: true } })),
    '1 line · LF · UTF-8',
    'one line, not "1 lines"',
  )

  // ---- the line-ending vocabulary ----------------------------------------------------------

  eq(m.endingLabel('lf'), 'LF', 'the status bar’s word')
  eq(m.endingLabel('crlf'), 'CRLF', 'the status bar’s word')
  eq(m.endingLabel('mixed'), 'Mixed', 'a half-converted file is visibly mixed')
  ok(
    m.endingLabel('none') !== 'LF',
    'a file with no break has no ending; claiming LF is a guess on the one row people read to '
      + 'find out whether a tool mangled a checkout',
  )

  // ---- 3 (cont). the disk/buffer disagreement ----------------------------------------------

  eq(
    m.sizeNote(true),
    'on disk',
    'a dirty buffer makes the card label its numbers — without it the card silently contradicts '
      + 'the status bar six inches away and neither says which is stale',
  )
  eq(m.sizeNote(false), null, 'and says nothing when there is nothing to disambiguate')

  // ---- the path row ------------------------------------------------------------------------

  const ROOTS = ['/home/u/work/cide']
  eq(
    m.displayPath('/home/u/work/cide/ui/src/main.tsx', ROOTS),
    'ui/src/main.tsx',
    'inside a root, the path the reader recognises',
  )
  eq(
    m.displayPath('/tmp/scratch.txt', ROOTS),
    '/tmp/scratch.txt',
    'outside every root the absolute path is itself the information',
  )
  eq(
    m.displayPath('/home/u/work/cide/a.txt', ['/home/u', '/home/u/work/cide']),
    'a.txt',
    'the longest matching root wins, so a nested project does not show its parent’s prefix',
  )
  eq(m.displayPath('/home/u/work/cide/a.txt', []), '/home/u/work/cide/a.txt', 'no roots, no change')

  // ---- the label ---------------------------------------------------------------------------

  ok(m.cardLabel(file()).includes('changeModel.ts'), 'the dialog names the file it is about')

  if (failed > 0) {
    console.error(`\nfilePropertiesModel.ts: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('filePropertiesModel.ts: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
