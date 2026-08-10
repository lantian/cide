/**
 * The pure logic behind the search panel: the grouping, the byte-offset highlight, the
 * indentation trim, the state machine and the readout.
 *
 * These are the parts where being wrong is invisible. A highlight computed with
 * `String.slice` over a byte offset lands one character early on any line with an accent in
 * it — which reads as a font quirk, not a bug. A grouping that keyed on a map instead of on
 * consecutive runs reorders files as results arrive, which reads as jitter. A state machine
 * that collapsed "searching" into "no results" tells the user their query found nothing for
 * the second before it finds something.
 *
 * Same shape as `check-picker.mjs` — there is no JS test runner in this project, and
 * `SearchModel.ts` is import-free precisely so the TypeScript in `node_modules` can compile
 * it on its own.
 *
 * Run: `pnpm --dir ui run check:search`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-search-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    failed += 1
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
  }
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/SearchModel.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      '--lib', 'es2023',
      // `@types/react-dom` is auto-included from `node_modules/@types` and does not compile
      // without the DOM lib. Matches `check-picker.mjs` and the project tsconfig.
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const {
    EMPTY_QUERY,
    groupHits,
    panelState,
    sameQuery,
    spliceHits,
    splitHighlight,
    summarize,
    trimIndent,
  } = require(join(out, 'sidebar/SearchModel.js'))

  /* ------------------------------------------------------------------------- grouping */

  const hit = (path, line, text = 'needle', start = 0, end = 6) => ({
    path: `/root/${path}`,
    rel: path,
    line,
    text,
    start,
    end,
  })
  const none = new Set()

  eq(groupHits([], none), [], 'no hits is no rows')

  const two = [hit('a.rs', 1), hit('a.rs', 9), hit('b.rs', 3)]
  eq(
    groupHits(two, none).map((r) => (r.kind === 'file' ? `${r.rel}(${r.hits})` : `${r.hit.line}`)),
    ['a.rs(2)', '1', '9', 'b.rs(1)', '3'],
    'a heading per file, then its hits, with the count on the heading',
  )
  eq(
    groupHits(two, none)[0],
    { kind: 'file', path: '/root/a.rs', rel: 'a.rs', hits: 2, collapsed: false },
    'the heading carries the absolute path as its identity and the relative one to draw',
  )
  eq(
    groupHits(two, none)[1],
    { kind: 'hit', index: 0, hit: two[0] },
    'a hit row carries its index in the flat list, so selecting it needs no search',
  )

  // Collapsing keeps the heading and its count, and removes only the lines.
  eq(
    groupHits(two, new Set(['/root/a.rs'])).map((r) => (r.kind === 'file' ? r.rel : r.hit.line)),
    ['a.rs', 'b.rs', 3],
    'a collapsed group keeps its heading',
  )
  eq(groupHits(two, new Set(['/root/a.rs']))[0].hits, 2, 'and still counts its hidden hits')

  // The property the run-based grouping exists for: order is the walk's, never rearranged.
  const interleaved = [hit('a.rs', 1), hit('b.rs', 1), hit('a.rs', 2)]
  eq(
    groupHits(interleaved, none)
      .filter((r) => r.kind === 'file')
      .map((r) => `${r.rel}(${r.hits})`),
    ['a.rs(1)', 'b.rs(1)', 'a.rs(1)'],
    'a file that reappears is a second group, not a hit moved up into the first',
  )

  /* ------------------------------------------------------------- byte-offset highlight */

  eq(
    splitHighlight('let needle = 1', 4, 10),
    { before: 'let ', match: 'needle', after: ' = 1' },
    'ascii: byte offsets are string indices',
  )
  // The whole reason this function exists. `'🙂 needle'.slice(5, 11)` is `eedle ` — the emoji
  // is four bytes and two UTF-16 units, so a naive slice lands two characters late.
  eq(
    splitHighlight('🙂 needle', 5, 11),
    { before: '🙂 ', match: 'needle', after: '' },
    'an emoji before the match: four bytes, two UTF-16 units',
  )
  eq('🙂 needle'.slice(5, 11) === 'needle', false, 'and String.slice really does get it wrong')
  eq(
    splitHighlight('héllo needle', 7, 13),
    { before: 'héllo ', match: 'needle', after: '' },
    'a two-byte character before the match',
  )
  eq(
    splitHighlight('needle', 0, 6),
    { before: '', match: 'needle', after: '' },
    'a match filling the line',
  )
  // Offsets arrive over IPC. A decode of an out-of-range subarray inside a render unmounts
  // the panel, so they are clamped rather than trusted.
  eq(
    splitHighlight('short', 40, 90),
    { before: 'short', match: '', after: '' },
    'offsets past the end are clamped',
  )
  eq(
    splitHighlight('short', 3, 1),
    { before: 'sho', match: '', after: 'rt' },
    'an inverted range highlights nothing rather than producing a negative slice',
  )
  eq(
    splitHighlight('short', -5, 2),
    { before: '', match: 'sh', after: 'ort' },
    'a negative offset is clamped to the start',
  )
  eq(
    splitHighlight('short', Number.NaN, 2),
    { before: '', match: 'sh', after: 'ort' },
    'a non-finite offset does not produce undefined slices',
  )

  /* ------------------------------------------------------------------ indentation trim */

  eq(
    trimIndent({ ...hit('a.rs', 1), text: '        let needle = 1', start: 12, end: 18 }),
    { path: '/root/a.rs', rel: 'a.rs', line: 1, text: 'let needle = 1', start: 4, end: 10 },
    'the offsets move with the stripped indentation',
  )
  eq(
    splitHighlight(
      trimIndent({ ...hit('a.rs', 1), text: '\t\tlet needle', start: 6, end: 12 }).text,
      trimIndent({ ...hit('a.rs', 1), text: '\t\tlet needle', start: 6, end: 12 }).start,
      trimIndent({ ...hit('a.rs', 1), text: '\t\tlet needle', start: 6, end: 12 }).end,
    ),
    { before: 'let ', match: 'needle', after: '' },
    'trimming tabs keeps the highlight on the match',
  )
  const flush = { ...hit('a.rs', 1), text: 'needle', start: 0, end: 6 }
  eq(trimIndent(flush) === flush, true, 'an unindented line is returned unchanged')
  eq(
    trimIndent({ ...hit('a.rs', 1), text: '  a  b', start: 5, end: 6 }).text,
    'a  b',
    'only leading whitespace goes',
  )

  eq(
    trimIndent({ ...hit('a.rs', 1), text: '    let x', start: 0, end: 4 }),
    { path: '/root/a.rs', rel: 'a.rs', line: 1, text: '    let x', start: 0, end: 4 },
    'a match ON the indentation is not trimmed away — trimming it would erase the highlight',
  )
  eq(
    splitHighlight(
      trimIndent({ ...hit('a.rs', 1), text: '\tlet x', start: 0, end: 1 }).text,
      trimIndent({ ...hit('a.rs', 1), text: '\tlet x', start: 0, end: 1 }).start,
      trimIndent({ ...hit('a.rs', 1), text: '\tlet x', start: 0, end: 1 }).end,
    ),
    { before: '', match: '\t', after: 'let x' },
    'searching for a tab still points at the tab',
  )

  /* ------------------------------------------------------------------- appending pages */

  // The ordinary poll: the page lands at the end of what is held.
  eq(spliceHits([1, 2, 3], 3, [4, 5]), [1, 2, 3, 4, 5], 'a page at the end is appended')
  eq(spliceHits([], 0, [1]), [1], 'the first page of a new search')
  eq(spliceHits([1, 2], 2, []), [1, 2], 'an empty page changes nothing')
  // The case the panel used to get stuck on for ever: a second window restarted the walk, so
  // the backend list is shorter than the local one and every later poll asks past its end.
  eq(
    spliceHits([1, 2, 3, 4, 5], 0, [9]),
    [9],
    'a job restarted under the panel resynchronises rather than dropping the frame',
  )
  eq(
    spliceHits([1, 2, 3, 4, 5], 2, [9]),
    [1, 2, 9],
    'and it truncates to the answered offset, so no gap can open',
  )
  // Offsets arrive over IPC; nothing here may produce a sparse array or a negative slice.
  eq(spliceHits([1, 2], 99, [3]), [1, 2, 3], 'an offset past the end cannot leave a hole')
  eq(spliceHits([1, 2], -4, [3]), [3], 'a negative offset is clamped to the start')

  /* --------------------------------------------------------------------- panel states */

  const state = (over) =>
    panelState({ pattern: 'needle', running: false, total: 0, error: null, ...over })

  eq(state({ pattern: '' }), 'empty', 'no query at all')
  eq(state({ running: true }), 'searching', 'a walk with nothing found yet')
  eq(state({}), 'noResults', 'a finished walk with nothing in it')
  eq(state({ total: 3 }), 'results', 'hits')
  eq(state({ running: true, total: 3 }), 'results', 'hits, still walking')
  // The error outranks `running`: a pattern that did not compile started no walk.
  eq(state({ running: true, error: 'unclosed group' }), 'error', 'a bad pattern is not progress')
  eq(state({ pattern: '', error: 'stale' }), 'empty', 'an emptied box beats a stale error')
  eq(state({ error: '' }), 'noResults', 'an empty error string is not an error')

  /* -------------------------------------------------------------------------- readout */

  eq(summarize(0, 0, false), 'no results', 'nothing found')
  eq(summarize(1, 1, false), '1 result in 1 file', 'both singular')
  eq(summarize(2, 1, false), '2 results in 1 file', 'plural hits in one file')
  eq(summarize(12, 3, false), '12 results in 3 files', 'the ordinary case')
  eq(summarize(5000, 42, true), '5000+ results in 42 files', 'a truncated search reports floors')
  const grouped = (n) => n.toLocaleString('en-US')
  eq(
    summarize(5000, 42, true, grouped),
    '5,000+ results in 42 files',
    'the formatter is the caller’s — the panel passes groupDigits',
  )

  /* --------------------------------------------------------------- query identity */

  eq(sameQuery(EMPTY_QUERY, { ...EMPTY_QUERY }), true, 'a copy is the same query')
  eq(
    sameQuery({ ...EMPTY_QUERY, pattern: 'a' }, { ...EMPTY_QUERY, pattern: 'b' }),
    false,
    'a different pattern',
  )
  // Every toggle is part of the identity, or a frame would be painted under the wrong one.
  for (const [key, value] of [
    ['caseSensitive', true],
    ['wholeWord', true],
    ['mode', 'regex'],
  ]) {
    eq(
      sameQuery({ ...EMPTY_QUERY }, { ...EMPTY_QUERY, [key]: value }),
      false,
      `${key} is part of the query identity`,
    )
  }
  eq(EMPTY_QUERY.pattern, '', 'the panel opens with an empty box')
  eq(EMPTY_QUERY.mode, 'literal', 'and in literal mode, not regex')

  if (failed > 0) {
    console.error(`\ncheck-search: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-search: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
