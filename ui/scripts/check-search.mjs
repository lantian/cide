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
 * it on its own. `rowPaths.ts` and `treeFocus.ts` are compiled beside it for the same reason:
 * the first is the containment rule `scopeLabel` reuses rather than reimplements, and the
 * second is the "does the file tree hold the caret" claim that decides whether ⌃⇧F narrows
 * the search to a folder.
 *
 * Run: `pnpm --dir ui run check:search`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-search-'))
let failed = 0

const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8')

const ok = (cond, what) => {
  if (cond) return
  failed += 1
  console.error(`FAIL ${what}`)
}

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
      // The click rules, pinned at the foot of this file. They live in a module of their own
      // so a check script can hold them; see `clickSemantics.ts`.
      'src/sidebar/clickSemantics.ts',
      // `scopeLabel` imports `rootOf` from here — the longest-match, segment-aware containment
      // rule this repository has already got wrong once. Compiled in the same invocation so a
      // relative sibling import resolves under a bare `tsc` with no `paths`.
      'src/sidebar/rowPaths.ts',
      // The focus claim ⌃⇧F consults. Import-free so it can be driven here.
      'src/sidebar/treeFocus.ts',
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
    hitPosition,
    panelState,
    problemField,
    problemLabel,
    rowKey,
    sameQuery,
    scopeDirOf,
    scopeLabel,
    spliceHits,
    splitHighlight,
    summarize,
    trimIndent,
  } = require(join(out, 'sidebar/SearchModel.js'))
  const { searchClick, gestureOf } = require(join(out, 'sidebar/clickSemantics.js'))
  const { claimTreeFocus, releaseTreeFocus, treeFocused, __resetTreeFocus } = require(
    join(out, 'sidebar/treeFocus.js'),
  )

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
  // The error outranks `running`: an input that did not parse started no walk.
  const problem = (kind, detail = 'nope') => ({ kind, detail })
  eq(
    state({ running: true, error: problem('pattern', 'unclosed group') }),
    'error',
    'a bad pattern is not progress',
  )
  eq(state({ pattern: '', error: problem('scope') }), 'empty', 'an emptied box beats a stale error')
  eq(state({ error: problem('scope') }), 'error', 'a folder that names nothing is an answer')
  eq(state({ error: problem('include') }), 'error', 'and so is a glob that did not parse')

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
  /*
   * Every field of `SearchQuery` is part of the identity, or a frame is painted under the
   * wrong one. The toggles are the mild version of that; the scope is the sharp one — an
   * unscoped frame drawn under a scoped box is one folder's results under another folder's
   * heading, with nothing on screen to say which.
   *
   * Driven a field at a time rather than asserted as a list, because the failure is a
   * *forgotten* field and a list would have to be remembered too.
   */
  for (const [key, value] of [
    ['caseSensitive', true],
    ['wholeWord', true],
    ['mode', 'regex'],
    ['scope', 'crates/cide-git'],
    ['include', '*.ts'],
  ]) {
    eq(
      sameQuery({ ...EMPTY_QUERY }, { ...EMPTY_QUERY, [key]: value }),
      false,
      `${key} is part of the query identity`,
    )
  }
  // And the other direction: every key of `EMPTY_QUERY` is one `sameQuery` actually reads. A
  // field added to the DTO and to `EMPTY_QUERY` but not to the comparison is exactly the bug
  // the loop above cannot see, because nothing there names the field either.
  for (const key of Object.keys(EMPTY_QUERY)) {
    const other = key === 'mode' ? 'regex' : typeof EMPTY_QUERY[key] === 'boolean' ? true : 'x'
    eq(
      sameQuery({ ...EMPTY_QUERY }, { ...EMPTY_QUERY, [key]: other }),
      false,
      `sameQuery reads every field of EMPTY_QUERY, including ${key}`,
    )
  }
  eq(EMPTY_QUERY.pattern, '', 'the panel opens with an empty box')
  eq(EMPTY_QUERY.mode, 'literal', 'and in literal mode, not regex')
  eq(EMPTY_QUERY.scope, '', 'and unscoped — the whole project, as it always was')
  eq(EMPTY_QUERY.include, '', 'and over every file')

  /* ------------------------------------------------------- naming a folder to search in */

  const ROOT = { path: '/home/u/cide', label: 'cide' }
  const OTHER = { path: '/home/u/ui', label: 'ui' }

  eq(
    scopeLabel('/home/u/cide/crates/cide-git', [ROOT]),
    'crates/cide-git',
    'a single-root project names a folder relative to its root — the spelling `rel` uses',
  )
  eq(
    scopeLabel('/home/u/cide/crates/cide-git', [ROOT, OTHER]),
    'cide/crates/cide-git',
    'and a multi-root one prefixes the label, so the box and the hit headings agree',
  )
  eq(scopeLabel('/home/u/cide', [ROOT]), 'cide', 'the root itself is named by its label')
  eq(
    scopeLabel('/home/u/cide', [ROOT, OTHER]),
    'cide',
    'in either case — an empty box would read as no scope at all',
  )
  // Longest match, which is `rootOf`'s rule and the reason this imports it rather than
  // slicing at the first root that matches.
  eq(
    scopeLabel('/home/u/cide/ui/src', [ROOT, { path: '/home/u/cide/ui', label: 'ui' }]),
    'ui/src',
    'the innermost root wins when two of them nest',
  )
  eq(
    scopeLabel('/tmp/elsewhere/src', [ROOT]),
    '/tmp/elsewhere/src',
    'a path under no root keeps its absolute spelling: emptying the box would silently widen '
      + 'the search back to the whole project, and the backend can say what is wrong with it',
  )
  // `/home/u/cide-old` is not inside `/home/u/cide`; a bare `startsWith` says it is and would
  // slice the string at the wrong offset. `rowPaths.ts`'s header is the account of that.
  eq(
    scopeLabel('/home/u/cide-old/src', [ROOT]),
    '/home/u/cide-old/src',
    'containment is segment-aware',
  )

  eq(scopeDirOf('/a/b/c', true), '/a/b/c', 'a directory is its own scope')
  eq(scopeDirOf('/a/b/c.rs', false), '/a/b', 'and a file means the folder that holds it')
  eq(scopeDirOf('/c.rs', false), '/c.rs', 'a file at the filesystem root has no parent to name')

  /* --------------------------------------------------- which box a problem is about */

  eq(problemLabel('pattern'), 'Bad pattern', 'the heading the notice draws')
  eq(problemLabel('scope'), 'No such folder', 'a mistyped folder is not a mistyped regex')
  eq(problemLabel('include'), 'Bad file pattern', 'nor is a mistyped glob')
  eq(problemLabel('whatever'), 'Search failed', 'and an unknown kind still says something')
  eq(problemField('scope'), 'scope', 'the box to outline')
  eq(
    problemField('whatever'),
    null,
    'an unknown kind outlines nothing: an aria-invalid on an arbitrary input would be a lie',
  )

  /* ------------------------------------------------ does the file tree hold the caret */

  /*
   * The fact ⌃⇧F consults to decide whether it narrows the search to the selected folder.
   * A claim that is never released is the failure worth pinning: it would mean the chord kept
   * scoping to whatever the tree had selected long after the caret went to a terminal.
   */
  __resetTreeFocus()
  eq(treeFocused(), false, 'nothing holds the caret before anything has focused')
  claimTreeFocus()
  eq(treeFocused(), true, 'the tree took it')
  claimTreeFocus()
  eq(treeFocused(), true, 'and a second focus inside the tree is not a second claim')
  releaseTreeFocus()
  eq(treeFocused(), false, 'a blur that really left the tree gives it up')
  releaseTreeFocus()
  eq(treeFocused(), false, 'releasing twice is safe — unmount runs after a blur')

  /* ------------------------------------------------------ where a hit actually is */

  /*
   * > *"search result click doesn't point me to found place (should open file and select the
   * > line)"*
   *
   * The file half already worked. The *place* half is this conversion, and it is invisible
   * when it is wrong: byte offsets and editor columns agree for ASCII and diverge for
   * everything else, so a caret placed from the raw offsets lands two characters past the
   * match on exactly the lines a user is least likely to blame the search panel for.
   */
  const at = (text, start, end) => hitPosition({ path: '/p', rel: 'p', line: 7, text, start, end })

  eq(at('let needle = 1', 4, 10), { line: 7, column: 5, endColumn: 11 },
    'an ASCII line: 1-based columns, one past the byte offsets')
  eq(at('needle', 0, 6).column, 1, 'a match at the start of a line is column 1, not column 0')
  eq(
    // `✓` is 3 bytes and 1 UTF-16 unit, so `needle` starts at byte 7 and at column 6.
    at('// ✓ needle here', 7, 13),
    { line: 7, column: 6, endColumn: 12 },
    'a 3-byte glyph before the match shifts the byte offsets by 2 more than the columns — '
      + 'reading the offsets as columns would put the caret two characters late',
  )
  eq(
    at('🙂 needle', 5, 11),
    { line: 7, column: 4, endColumn: 10 },
    'an astral character is 4 bytes and 2 UTF-16 units, which is the case a naive `char` '
      + 'count gets wrong in the other direction',
  )
  eq(
    at('needle', 99, 200),
    { line: 7, column: 7, endColumn: 7 },
    'offsets past the end of the line are clamped rather than trusted: they arrived over IPC, '
      + 'and a decode of an out-of-range subarray throws inside a render',
  )
  eq(
    hitPosition({ path: '/p', rel: 'p', line: 3, text: '\t\tneedle', start: 2, end: 8 }).column,
    3,
    'the position is read from the UNTRIMMED line — `trimIndent` is a display decision for a '
      + '252px panel, and the file on disk still has the indentation',
  )

  /* ------------------------------------------------------------ selection identity */

  eq(rowKey({ kind: 'file', path: '/a/b.rs', rel: 'b.rs', hits: 2, collapsed: false }), 'f:/a/b.rs',
    'a heading is identified by its path')
  eq(rowKey({ kind: 'hit', index: 12, hit: hit('a.rs', 3) }), 'h:12',
    'a hit by its index into the flat list, not by path:line — one line can hold two matches, '
      + 'and they are two rows the user can move between')

  /* ------------------------------------------------------------------- click rules */

  const click = (gesture, kind) => searchClick({ gesture, kind })
  const act = (select, toggle, open) => ({ select, toggle, open })

  eq(
    click('single', 'hit'),
    act(true, false, true),
    'ONE click on a result opens it — unlike the two trees, and deliberately: a hit is a '
      + 'request to go somewhere, and there is nothing else a click on one could mean',
  )
  eq(
    click('double', 'hit'),
    act(false, false, false),
    'the second half of a double-click navigates nowhere: re-opening would be harmless today '
      + 'and "one gesture, one navigation" is what keeps it harmless later',
  )
  eq(click('single', 'file'), act(true, true, false), 'a heading folds its group')
  eq(click('double', 'file'), act(false, false, false), 'and does not fold it straight back')
  eq(gestureOf(2), 'double', 'told apart by `detail`, with no timer to make a click feel late')

  /* ------------------------------------------------- the parts a type cannot state */

  /*
   * `problemLabel` takes a bare `string`, because `SearchModel.ts` is import-free and cannot
   * see the generated `SearchProblem` union. `SearchStore.ts` holds the union in a `Record`
   * so a new Rust variant is a `tsc --noEmit` failure there — but nothing in the type system
   * connects that list to the `case` arms here, and a kind with no arm draws *Search failed*
   * over a sentence nobody wrote copy for. This is that connection.
   */
  const store = read('../src/sidebar/SearchStore.ts')
  const model = read('../src/sidebar/SearchModel.ts')
  const kinds = (store.match(/PROBLEM_KINDS[^{]*\{([^}]*)\}/s)?.[1] ?? '')
    .split(',')
    .map((line) => line.trim().split(':')[0].trim())
    .filter((name) => /^[a-z][a-zA-Z]*$/.test(name))
  ok(kinds.length >= 3, `PROBLEM_KINDS parsed out of SearchStore.ts (got ${kinds.join()})`)
  for (const kind of kinds) {
    ok(
      model.includes(`case '${kind}':`),
      `problemLabel has a case for the ${kind} problem kind`,
    )
    ok(
      problemLabel(kind) !== 'Search failed',
      `and the ${kind} kind draws its own heading rather than the fallback`,
    )
  }

  /*
   * The panel's own wiring, greped rather than rendered.
   *
   * Each of these is a way for the feature to compile, type-check and do nothing visible:
   * a hardcoded heading that survives the typed error, a clear button drawn as the character
   * `✕` (which `check:ui-icons` bans and which would slip through as ordinary JSX text), and
   * two boxes that exist in the model with no input bound to them.
   */
  const panel = read('../src/sidebar/SearchPanel.tsx')
  ok(panel.includes('problemLabel(error.kind)'), 'the notice heading comes from the problem kind')
  ok(!/>\s*Bad pattern\s*</.test(panel), 'and is not a hardcoded string beside it')
  ok(panel.includes('audit="searchScope"'), 'the folder box exists')
  ok(panel.includes('audit="searchInclude"'), 'the file-pattern box exists')
  ok(panel.includes('data-audit={audit}'), 'and both reach the DOM as audit hooks')
  // The mark, not the character — `check:ui-icons` is what bans the character everywhere, and
  // this is the positive half: the button must actually draw something.
  ok(/<Icon name="x"/.test(panel), 'the clear button is an icon mark')
  // The disclosure must not be able to hide a filter that is in force — a panel searching one
  // folder while looking exactly like it is searching all of them is the whole hazard here.
  ok(
    /narrowInUse\s*=\s*query\.scope[^\n]*query\.include/.test(panel)
      && /showNarrow\s*=\s*moreOpen\s*\|\|\s*narrowInUse/.test(panel),
    'the narrowing boxes are shown whenever either of them holds something',
  )

  if (failed > 0) {
    console.error(`\ncheck-search: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-search: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
