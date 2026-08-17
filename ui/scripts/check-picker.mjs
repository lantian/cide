/**
 * The pure logic behind the overlays and the file tree: palette ranking, list navigation,
 * the counter's digit grouping, the row-window chunk arithmetic, and the plan that decides
 * which projects get walked.
 *
 * The ranking half exists because `overlays/score.ts` is a *port* of
 * `cide_core::commands::score`, and a port with no test is a copy that drifts. The
 * assertions below are the same ones `commands.rs` makes about its own tiers, so a change on
 * either side that breaks the agreement fails on this side too.
 *
 * The chunk arithmetic half exists because off-by-one errors there are invisible: a tree that
 * asks for one chunk too few shows a band of blank rows only at particular scroll offsets.
 *
 * Same shape as `check-status-format.mjs` and `check-key-gate.mjs` — there is no JS test
 * runner in this project, and these are pure functions the TypeScript in `node_modules` can
 * compile on its own.
 *
 * Run: `pnpm --dir ui run check:picker`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-picker-'))
let failed = 0

const ok = (cond, what) => {
  if (!cond) {
    failed += 1
    console.error(`FAIL ${what}`)
  }
}

const uiFile = (rel) => readFileSync(fileURLToPath(new URL(`../${rel}`, import.meta.url)), 'utf8')

/**
 * Remove comments before grepping.
 *
 * Load-bearing: `FilePicker.tsx` and `store.ts` explain the library scope by name at length,
 * so a grep over raw source matches the *explanation* of a feature that has been deleted —
 * which is precisely how a gate stays green over a dead control. See `check-claude-env.mjs`,
 * which makes the same argument and paid for it.
 */
const strip = (src) => src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

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
      'src/overlays/score.ts',
      'src/overlays/format.ts',
      'src/overlays/listKeys.ts',
      'src/overlays/gotoLineModel.ts',
      // M14. The Find usages popup's four indistinguishable-looking empty states, its filter
      // predicate and its file count. Import-free for exactly this reason.
      'src/overlays/usagesModel.ts',
      'src/sidebar/rowWindow.ts',
      'src/store/fileIndex.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      '--lib', 'es2023',
      // These five modules need no DOM, but `@types/react-dom` is auto-included from
      // `node_modules/@types` and does not compile without it. Matches the project tsconfig.
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const { searchCommands } = require(join(out, 'overlays/score.js'))
  const { groupDigits, matchCounter, basename, dirname, kindBadge, pickerEmptyState, pickerEmptyText } =
    require(join(out, 'overlays/format.js'))
  const { listAction, PAGE_ROWS } = require(join(out, 'overlays/listKeys.js'))
  const { canGo, gotoNote, parseGoto } = require(join(out, 'overlays/gotoLineModel.js'))
  const usages = require(join(out, 'overlays/usagesModel.js'))
  const { chunkOf, chunkRequest, chunksFor, chunksToEvict } = require(
    join(out, 'sidebar/rowWindow.js'),
  )
  const { planFileIndex, isNoIndex } = require(join(out, 'store/fileIndex.js'))

  /* ----------------------------------------------------------------- palette ranking */

  // Six of the seed commands the mock's palette lists, in registry order.
  //
  // Every row carries a `keywords` array, empty where the registry's is: the generated
  // `Command` always has the field, so a fixture without it would exercise a shape the
  // palette never sees — which is the mistake `check-commands.mjs` now guards against for
  // `ProjectRoot`, made here.
  const TABLE = [
    { id: 'pane.split.right', title: 'Split pane right', group: 'Window', keywords: [] },
    { id: 'pane.split.down', title: 'Split pane down', group: 'Window', keywords: [] },
    { id: 'claude.split.newSession', title: 'Split: new Claude session', group: 'Claude', keywords: [] },
    { id: 'pane.promoteToTab', title: 'Promote pane to full tab', group: 'Window', keywords: [] },
    { id: 'pane.detachToWindow', title: 'Detach pane into window', group: 'Window', keywords: [] },
    { id: 'terminal.splitBelow', title: 'Split terminal below', group: 'Terminal', keywords: [] },
    { id: 'theme.toggle', title: 'Toggle light/dark theme', group: 'View', keywords: [] },
    // Not a seed command. It is here so `new` has a mid-word competitor — without one, the
    // word-start assertion below is satisfied by any implementation that matches at all.
    { id: 'session.renew', title: 'Renew session', group: 'Claude', keywords: [] },
  ]
  const ids = (query) => searchCommands(TABLE, query).map((c) => c.id)

  eq(ids('').length, TABLE.length, 'an empty query lists the whole table')
  eq(searchCommands(TABLE, '') === TABLE, false, 'an empty query returns a copy, not the table')
  eq(ids('   '), ids(''), 'a whitespace query is an empty query')

  // A title prefix must outrank everything else — the criterion `commands.rs` names.
  eq(ids('split')[0], 'pane.split.right', 'a title prefix ranks first')
  // `Split: new Claude session` also starts with `split`, so both prefix hits come before
  // `Split terminal below`... which also starts with it. Table order breaks the tie.
  eq(
    ids('split'),
    [
      'pane.split.right',
      'pane.split.down',
      'claude.split.newSession',
      'terminal.splitBelow',
    ],
    'equal scores keep table order',
  )

  // A word-start match beats a mid-word substring. `Split: new Claude session` matches `new`
  // at a word start (tier 4); `Renew session` matches it inside `Renew` (tier 3).
  // The tail is the subsequence tier — `new` reads out of `Split pa*ne* do*w*n` too — so only
  // the first two positions are the claim being made.
  eq(
    ids('new').slice(0, 2),
    ['claude.split.newSession', 'session.renew'],
    'a word start after a colon outranks a mid-word substring',
  )

  // Earlier offsets rank first within a tier: `pane` starts at 6 in both `Split pane …`
  // titles, at 7 in `Detach pane into window` and at 8 in `Promote pane to full tab`.
  eq(
    ids('pane'),
    ['pane.split.right', 'pane.split.down', 'pane.detachToWindow', 'pane.promoteToTab'],
    'offset orders within a tier',
  )

  // An id-only match still matches, below every title tier.
  eq(ids('promotetotab'), ['pane.promoteToTab'], 'the id is searched')

  // Subsequence is the last resort. `spr` finds `Split pane right`.
  eq(ids('spr')[0], 'pane.split.right', 'subsequence matching')
  eq(ids('zzz'), [], 'no match is empty')

  eq(ids('SPLIT'), ids('split'), 'matching is case-insensitive')

  /* -------------------------------------------- the two tiers below the title/id ones */

  /*
   * The phrase from the report that started this round, and the two tiers it needed.
   *
   * `create new branch` matched **nothing at all**. Every tier above matches the needle
   * *whole* against one string, and no title contains the word `create` — the needle is not a
   * prefix of `New branch…`, not one of its words, not a substring, not in the id, and (being
   * longer than the title) not even a subsequence of it. The palette answered *No matching
   * commands* about a command that exists, which is indistinguishable from the command not
   * existing, which is the whole failure class this round is about.
   *
   * A separate table from `TABLE` above, deliberately: `New branch…` is a *prefix* match for
   * `new`, so folding it into that one would have quietly moved the assertion about word-start
   * beating mid-word onto a different pair of rows.
   */
  const GIT = [
    { id: 'git.branch.switch', title: 'Switch branch…', group: 'Git', keywords: [] },
    { id: 'git.pull', title: 'Pull (fast-forward only)', group: 'Git', keywords: ['update'] },
    {
      id: 'git.branch.new',
      title: 'New branch…',
      group: 'Git',
      keywords: ['create', 'make', 'checkout'],
    },
  ]
  const gitIds = (query) => searchCommands(GIT, query).map((c) => c.id)

  eq(
    gitIds('create new branch'),
    ['git.branch.new'],
    'the reported phrase finds the command it names — every word of it starts a word of the ' +
      'title, a keyword or the id',
  )
  eq(
    gitIds('create'),
    ['git.branch.new'],
    'a keyword matches on its own, and only for the command that declares it',
  )
  eq(
    gitIds('update'),
    ['git.pull'],
    'a keyword the title does not contain at all — `Pull (fast-forward only)` is what the app ' +
      'calls it and `update` is what a user calls it',
  )
  eq(
    gitIds('eat'),
    [],
    'a keyword matches at a word boundary, not anywhere inside itself — `create` must not ' +
      'answer to `eat`, or the tier stops discriminating',
  )
  eq(
    gitIds('branch create'),
    ['git.branch.new'],
    'the words may be typed in any order: this tier is a set, not a sequence',
  )
  eq(
    gitIds('switch to branch'),
    [],
    'and every word has to land — `to` is in nothing here, so a row that matched two words ' +
      'out of three is not offered',
  )

  /* ------------------------------------------------------------------------ formatting */

  eq(groupDigits(0), '0', 'zero')
  eq(groupDigits(999), '999', 'below the first group')
  eq(groupDigits(1000), '1,000', 'the first group')
  eq(groupDigits(2418), '2,418', "the mock's own figure")
  eq(groupDigits(100000), '100,000', 'six digits')
  eq(groupDigits(1234567), '1,234,567', 'two groups')
  eq(matchCounter(6, 2418), '6 of 2,418', "the mock's counter, verbatim")

  eq(basename('/a/b/c.rs'), 'c.rs', 'basename')
  eq(basename('c.rs'), 'c.rs', 'basename of a bare name')
  eq(dirname('/a/b/c.rs'), '/a/b', 'dirname')
  eq(dirname('c.rs'), '', 'dirname of a bare name')

  eq(kindBadge('lib.rs'), { label: 'RS', tone: 'accent' }, 'rust badge')
  eq(kindBadge('Cargo.lock'), { label: 'LOCK', tone: 'faint' }, 'lock badge')
  eq(kindBadge('App.module.css'), { label: 'CSS', tone: 'purple' }, 'the last extension wins')
  // A leading dot is a hidden file, not an extension.
  eq(kindBadge('.gitignore'), { label: '·', tone: 'faint' }, 'a dotfile gets the neutral mark')
  eq(kindBadge('/x/y/Makefile'), { label: '·', tone: 'faint' }, 'no extension')

  /* ------------------------------------------------------------------ list navigation */

  const key = (name, mods = {}) => ({
    key: name,
    shiftKey: mods.shift === true,
    altKey: mods.alt === true,
    ctrlKey: mods.ctrl === true,
    metaKey: mods.meta === true,
  })

  eq(listAction(key('ArrowDown'), 5, 0), { kind: 'select', index: 1 }, 'down moves')
  eq(listAction(key('ArrowDown'), 5, 4), { kind: 'select', index: 0 }, 'down wraps')
  eq(listAction(key('ArrowUp'), 5, 0), { kind: 'select', index: 4 }, 'up wraps')
  eq(listAction(key('PageDown'), 100, 0), { kind: 'select', index: PAGE_ROWS }, 'page down')
  eq(listAction(key('PageUp'), 100, 0), { kind: 'select', index: 100 - PAGE_ROWS }, 'page up wraps')
  eq(listAction(key('Home'), 100, 40), { kind: 'select', index: 0 }, 'home')
  eq(listAction(key('End'), 100, 40), { kind: 'select', index: 99 }, 'end')
  eq(listAction(key('ArrowDown'), 0, 0), { kind: 'select', index: 0 }, 'movement in an empty list')
  eq(listAction(key('Escape'), 5, 0), { kind: 'dismiss' }, 'escape dismisses')

  eq(listAction(key('Enter'), 5, 0), { kind: 'accept', modifier: 'plain' }, '⏎')
  eq(listAction(key('Enter', { shift: true }), 5, 0), { kind: 'accept', modifier: 'shift' }, '⇧⏎')
  eq(listAction(key('Enter', { alt: true }), 5, 0), { kind: 'accept', modifier: 'alt' }, '⌥⏎')
  eq(listAction(key('Enter', { ctrl: true }), 5, 0), { kind: 'accept', modifier: 'ctrl' }, '⌃⏎')
  // The mock draws ⌘⏎; on Linux that binds to Ctrl, so both must reach the same action.
  eq(listAction(key('Enter', { meta: true }), 5, 0), { kind: 'accept', modifier: 'ctrl' }, '⌘⏎ = ⌃⏎')
  eq(listAction(key('Enter'), 0, 0), { kind: 'none' }, '⏎ on an empty list is not ours')
  eq(listAction(key('a'), 5, 0), { kind: 'none' }, 'an ordinary character is not ours')

  /* --------------------------------------------------------------- row-window chunking */

  eq(chunkOf(0), 0, 'row 0 is in chunk 0')
  eq(chunkOf(199), 0, 'the last row of chunk 0')
  eq(chunkOf(200), 1, 'the first row of chunk 1')

  eq(chunksFor(0, 0), [], 'an unmeasured range asks for nothing')
  eq(chunksFor(5, 5), [], 'an empty range asks for nothing')
  eq(chunksFor(10, 5), [], 'an inverted range asks for nothing')
  eq(chunksFor(0, 1), [0], 'one row is one chunk')
  eq(chunksFor(0, 200), [0], 'exactly one chunk')
  eq(chunksFor(0, 201), [0, 1], 'one row past the boundary adds a chunk')
  eq(chunksFor(199, 201), [0, 1], 'a range straddling a boundary')
  eq(chunksFor(450, 810), [2, 3, 4], 'a range spanning three chunks')

  eq(chunkRequest(0, 100000), { offset: 0, len: 200 }, 'a full chunk')
  eq(chunkRequest(3, 650), { offset: 600, len: 50 }, 'the last chunk is clamped to the count')
  eq(chunkRequest(4, 650), { offset: 800, len: 0 }, 'a chunk past the end asks for nothing')

  eq(chunksToEvict([1, 2, 3], new Set(), 10), [], 'nothing to evict below the cap')
  eq(chunksToEvict([1, 2, 3, 4], new Set(), 2), [1, 2], 'the two oldest go')
  eq(
    chunksToEvict([1, 2, 3, 4], new Set([1, 2]), 2),
    [3, 4],
    'visible chunks are never evicted, however old',
  )
  eq(
    chunksToEvict([1, 2], new Set([1, 2]), 1),
    [],
    'the cap yields rather than evict what is on screen',
  )

  /* ------------------------------------------------------------------- the index plan */

  // The shape `store/workspace.ts` passes in, and the map it carries between snapshots.
  const target = (project, ...roots) => ({ project, roots })
  const known = (...entries) => new Map(entries)
  const plan = (map, open) => {
    const answer = planFileIndex(map, open)
    return { index: answer.index, close: answer.close, known: [...answer.known] }
  }

  eq(
    plan(known(), [target('p1', '/a')]),
    { index: ['p1'], close: [], known: [['p1', '/a']] },
    'a project nobody has indexed is indexed',
  )
  eq(
    plan(known(['p1', '/a']), [target('p1', '/a')]),
    { index: [], close: [], known: [['p1', '/a']] },
    'the same project over the same roots is not walked again',
  )
  // The whole point of the snapshot-driven design: `applySnapshot` runs on every mutation in
  // any window, so a plan that re-indexed here would walk the repository on every keystroke
  // that marks a buffer dirty.
  eq(
    plan(known(['p1', '/a']), [target('p1', '/a')]).index,
    [],
    'a snapshot that changed nothing about the roots asks for no walk',
  )
  eq(
    plan(known(['p1', '/a']), [target('p1', '/a', '/b')]),
    { index: ['p1'], close: [], known: [['p1', '/a\u0000/b']] },
    'a project that gained a root is re-indexed',
  )
  eq(
    plan(known(['p1', '/a\u0000/b']), [target('p1', '/b', '/a')]).index,
    ['p1'],
    'root order is part of the identity, as it is in Rust',
  )
  eq(
    plan(known(['p1', '/a']), []),
    { index: [], close: ['p1'], known: [] },
    'a project that left the workspace is closed',
  )
  eq(
    plan(known(['p1', '/a']), [target('p2', '/b')]),
    { index: ['p2'], close: ['p1'], known: [['p2', '/b']] },
    'one project replacing another is both halves at once',
  )
  eq(
    plan(known(), [target('p1'), target('p2', '/b')]),
    { index: ['p2'], close: [], known: [['p2', '/b']] },
    'a project with no roots is skipped, not indexed',
  )
  eq(
    plan(known(['p1', '/a']), [target('p1')]),
    { index: [], close: [], known: [['p1', '/a']] },
    'a rootless project that was indexed before is left alone, not closed',
  )
  eq(
    plan(known(), [target('p1', '/a'), target('p2', '/b')]).index,
    ['p1', 'p2'],
    'workspace order is kept',
  )
  // Two roots whose concatenation is ambiguous under a naive separator. `/a` + `/bc` and
  // `/a/b` + `/c` both read as `/a/bc` if the join is `''`, and as `/a /bc` under a space —
  // which is a legal character in a path.
  eq(
    plan(known(), [target('p1', '/a', '/bc')]).known[0][1] ===
      plan(known(), [target('p2', '/a/b', '/c')]).known[0][1],
    false,
    'the roots digest cannot collide',
  )

  /* ------------------------------------------------------------------- go to line input */

  /*
   * What Ctrl+G's box accepts, and — the half that matters — what it refuses.
   *
   * The jump itself cannot crash whatever arrives: `editor/revealRequest.ts::revealRange` clamps
   * the line and the column, and `check-editor.mjs` already drives that clamp with a hit past the
   * end of a shortened file, line 0 and a negative line. What the clamp *cannot* do is refuse,
   * and its `whole()` turns a non-finite number into `1` — so a mistyped `l20` would clamp
   * silently to the top of the file. A caret that lands somewhere the user did not name is worse
   * than a disabled button, so every refusal below is the point of the module rather than
   * decoration on it.
   */
  {
    eq(parseGoto('120'), { kind: 'ok', at: { line: 120, column: 1 } }, 'a bare line number')
    eq(parseGoto('120:8'), { kind: 'ok', at: { line: 120, column: 8 } }, 'line and column')
    eq(parseGoto('  120  '), { kind: 'ok', at: { line: 120, column: 1 } }, 'surrounding space is trimmed')
    eq(parseGoto('0'), { kind: 'ok', at: { line: 0, column: 1 } }, 'line 0 is accepted — revealRange clamps it to 1')
    eq(parseGoto('').kind, 'empty', 'an empty field is not an error, it is how the box opens')
    eq(parseGoto('   ').kind, 'empty', 'and neither is whitespace')

    // Every one of these used to be a jump to line 1 if it reached `revealRange`.
    for (const bad of ['abc', 'l20', '12:', ':8', '1.5', '1,200', '12 8', '0x10', '1e3']) {
      eq(parseGoto(bad).kind, 'invalid', `\`${bad}\` is refused rather than clamped to line 1`)
    }
    eq(parseGoto('12:0').kind, 'invalid', 'column 0 is refused — a 0-based paste is an off-by-one worth reporting')
    eq(parseGoto('0:1'), { kind: 'ok', at: { line: 0, column: 1 } }, 'but line 0 still is not, and the asymmetry is deliberate')

    // Relative and percentage jumps are CodeMirror's `gotoLine` syntax and IDEA's Ctrl+G has
    // neither. Refused with their own sentence, because the user is not typing nonsense.
    for (const relative of ['+5', '-5', '50%']) {
      const parsed = parseGoto(relative)
      eq(parsed.kind, 'invalid', `\`${relative}\` is refused`)
      eq(
        /no \+n, -n or n%/.test(parsed.reason),
        true,
        `\`${relative}\` gets the relative-jump sentence, not "type a line number"`,
      )
    }

    eq(canGo(parseGoto('7')), true, 'Enter and the button share one predicate')
    eq(canGo(parseGoto('')), false, 'an empty field submits nothing')
    eq(canGo(parseGoto('nope')), false, 'and neither does an unparseable one')

    // The note is the only warning an out-of-range jump gets, because the clamp is silent.
    eq(gotoNote(parseGoto(''), 100), null, 'an empty field says nothing')
    eq(gotoNote(parseGoto('40'), 100), null, 'and neither does a line that exists')
    eq(gotoNote(parseGoto('100'), 100), null, 'the last line is in range')
    eq(
      gotoNote(parseGoto('101'), 100),
      'Past the end — this file has 100 lines',
      'one line past the end is reported, with the number that corrects the user',
    )
    eq(gotoNote(parseGoto('2'), 1), 'Past the end — this file has 1 line', 'and the singular is singular')
    eq(gotoNote(parseGoto('0'), 100), 'Before the first line — goes to line 1', 'line 0 says where it will land')
    eq(gotoNote(parseGoto('abc'), 100), 'Type a line number, or line:column', 'a refusal carries its own reason')
  }

  eq(isNoIndex({ kind: 'noIndex' }), true, 'the tagged NoIndex rejection is recognised')
  eq(isNoIndex({ kind: 'io', detail: {} }), false, 'a different FsError is not NoIndex')
  eq(isNoIndex('no file index for this project'), false, 'the prose alone is not the tag')
  eq(isNoIndex(null), false, 'null is not NoIndex')
  eq(isNoIndex(undefined), false, 'undefined is not NoIndex')

  /* ------------------------------------------------------------------------------------ */
  /* Find usages: the popup's arithmetic and its sentences. (M14)                          */
  /* ------------------------------------------------------------------------------------ */

  /*
   * Two things live here and both are invisible on a screenshot.
   *
   * The **filter** is a substring test and not `score.ts`'s fuzzy ranking, deliberately: that
   * scorer is a port of a command-title matcher, and run over source lines it produces confident
   * nonsense, because almost any subsequence of characters occurs somewhere in forty lines of code.
   *
   * The **statuses** are four different facts rendered as one grey line, and getting two of them
   * the wrong way round reads fine in review and misleads in use — in particular "still searching"
   * and "found nothing", where a popup that appears empty and then fills claims *no usages* for as
   * long as a cold rust-analyzer takes.
   */
  const ok = (condition, what) => {
    if (!condition) {
      failed += 1
      console.error(`FAIL ${what}`)
    }
  }

  eq(usages.subject(null), 'the symbol', 'a caret on punctuation still gets a sentence')
  eq(usages.subject('parse'), '‘parse’', 'and a named one gets its name')
  eq(usages.usagesLabel('parse'), 'Usages of ‘parse’', 'which heads the dialog')

  const row = (rel, text) => ({ rel, text })
  ok(usages.matchesUsage(row('src/a.rs', 'let x = parse()'), 'a.rs'), 'the path is searched')
  ok(usages.matchesUsage(row('src/a.rs', 'let x = parse()'), 'LET'), 'and the line, case-blind')
  ok(
    !usages.matchesUsage(row('src/a.rs', 'let x = parse()'), 'rs let'),
    'but not across the two — a query straddling them must not match by accident',
  )
  ok(usages.matchesUsage(row('src/a.rs', 'x'), '  '), 'an empty query keeps everything')
  eq(
    usages.filterUsages([row('a.rs', 'one'), row('b.rs', 'two')], 'b').length,
    1,
    'and the filter is the predicate applied, not a second implementation of it',
  )

  /*
   * Counted by consecutive run, the same way `groupHits` groups. A count derived from a Set of
   * paths would disagree with the number of headings drawn the moment a path appeared twice, and
   * "12 usages in 3 files" over four headings is the kind of wrong only noticed by someone who
   * already distrusts it.
   */
  eq(
    usages.fileCount([{ path: '/w/a' }, { path: '/w/a' }, { path: '/w/b' }, { path: '/w/a' }]),
    3,
    'a path that comes back later is a third group, because that is how it is drawn',
  )
  eq(usages.fileCount([]), 0, 'and nothing is no files')

  const status = (over) =>
    usages.usagesStatus({
      searching: false,
      detail: null,
      failed: null,
      total: 3,
      shown: 3,
      truncated: false,
      name: 'parse',
      query: '',
      ...over,
    })
  eq(status({}), null, 'rows draw instead of a status')
  // Read defensively from here on. A mutation that makes one of these branches return `null`
  // would otherwise die with a TypeError inside the check rather than printing which claim broke,
  // and an unreadable failure is a failure nobody acts on.
  const say = (over) => status(over) ?? '(no status at all)'
  ok(
    say({ searching: true }).startsWith('Finding usages of ‘parse’…'),
    'a running search SAYS it is running — a popup that appears empty and fills reads as "no ' +
      'usages" for as long as the search takes',
  )
  ok(
    say({ searching: true, detail: 'Indexing' }).includes('Indexing'),
    'and carries the server’s own word for what it is doing, so a 20-second wait is explained',
  )
  ok(
    say({ total: 0, shown: 0 }).includes('No usages of ‘parse’'),
    'used nowhere says so',
  )
  ok(
    say({ shown: 0, query: 'zzz' }).includes('zzz'),
    'while a filter that hides every row blames the filter — saying "no usages" there would be ' +
      'the app disowning a state the user just created',
  )
  eq(
    status({ failed: 'gopls is not running for this project.' }),
    'gopls is not running for this project.',
    'and the server’s own sentence outranks all of them, verbatim',
  )
  ok(say({ truncated: true }).includes('cap'), 'a capped list says so rather than lying')
  // The two that must never be confused: `null` from the protocol means "no symbol here", `[]`
  // means "used nowhere". Rendering the first as the second tells a user who pressed ⌥F7 on a
  // keyword that their function is unused.
  ok(
    /declaration/.test(usages.noSymbolSentence()) &&
      !/No usages/.test(usages.noSymbolSentence()),
    'no declaration and no usages are two different sentences — the protocol distinguishes them ' +
      '(`null` versus `[]`) and collapsing them tells a user who pressed ⌥F7 on a keyword that ' +
      'their function is unused',
  )
  ok(
    usages.noUsagesSentence(null).includes('the symbol'),
    'and both survive not knowing the identifier’s name',
  )

  /* ------------------------------------------------- M16: the picker's library scope */

  /*
   * Four empty states, two of which look identical and mean opposite things.
   *
   * The rule came out of a ternary in `FilePicker.tsx`'s JSX when the library scope made it
   * four-way. Two of the four are empty answers — "still filling, wait" and "finished, and
   * there is nothing" — and a third is an empty answer with a cause the user can act on. That
   * is the shape this project ships inverted, and it is unreachable by any check while it
   * lives inside a render.
   */
  const empty = (over) =>
    pickerEmptyState({
      running: false,
      awaitingIndex: false,
      query: 'lib',
      libraries: false,
      total: 12,
      ...over,
    })

  eq(empty({ running: true }), 'indexing', 'a filling matcher says so before anything else')
  eq(
    empty({ awaitingIndex: true }),
    'indexing',
    'and so does a project whose walk has not started — the two are different facts and the ' +
      'same answer, because what the user needs to know is "wait", not which index it is',
  )
  eq(
    empty({ running: true, query: '' }),
    'indexing',
    'a walk outranks the empty query: `Type to search` over a repository that is mid-walk ' +
      'tells the user to do the thing they already did',
  )
  eq(empty({ query: '' }), 'typeToSearch', 'and a settled empty query invites one')
  eq(empty({}), 'noMatches', 'a query that found nothing found nothing')
  eq(
    empty({ libraries: true, total: 0 }),
    'noLibraries',
    'but with the scope ON and a candidate set of zero, the cause is that nothing resolved — ' +
      '"No matches" there blames the query for an empty index, which is the answer ' +
      '`libraries.rs` refuses to give about its own group',
  )
  eq(
    empty({ libraries: false, total: 0 }),
    'noMatches',
    'and with the scope OFF a zero total is an unindexed project, not a library problem — ' +
      'saying otherwise would put a sentence about dependencies in front of somebody who ' +
      'never asked about them',
  )
  eq(
    empty({ libraries: true, total: 0, query: '' }),
    'typeToSearch',
    'an empty query has not searched for anything yet and has no business reporting on the ' +
      'library scope',
  )
  eq(
    empty({ libraries: true, total: 800 }),
    'noMatches',
    'a project with files of its own and no dependencies gets "No matches", which is true: ' +
      'nothing-resolved is only worth saying when there is nothing else to say',
  )

  // Every state has words, and they are all different. A `switch` that fell through would
  // return `undefined` and render an empty box, which reads as a picker that has hung.
  const STATES = ['indexing', 'typeToSearch', 'noLibraries', 'noMatches']
  const sentences = STATES.map((s) => pickerEmptyText(s))
  eq(
    sentences.filter((t) => typeof t === 'string' && t.length > 0).length,
    STATES.length,
    'every empty state has a sentence',
  )
  eq(new Set(sentences).size, STATES.length, 'and no two of them are the same words')

  /* ------------------------------------------------------------- the wiring, in source */
  //
  // Everything above is satisfied by a rule nothing calls and a scope nothing can reach.
  // These read comment-stripped source, because both files explain the feature by name at
  // length and a grep over raw source would match the explanation of a deleted one.

  const picker = strip(uiFile('src/overlays/FilePicker.tsx'))

  ok(
    /pickerEmptyState\(/.test(picker) && /pickerEmptyText\(/.test(picker),
    'FilePicker asks the checked rule rather than re-deciding in its JSX',
  )
  ok(
    /pickerApi\.query\([^)]*libraries/.test(picker),
    'and it passes the scope to `picker.query`. Without this the toggle is a flag nothing ' +
      'reads: it lights up, and the answer never changes',
  )
  ok(
    /pickerApi\.indexLibraries\(/.test(picker),
    'and it starts the walk when the scope goes on. Rust answers from an empty library ' +
      'matcher otherwise — a scope that is switched on and returns nothing, for ever',
  )
  {
    // The dep array. Flipping the scope changes the answer to the *same* query, so without
    // `libraries` in it ⌥L does nothing visible until the user types another character — a
    // control that appears not to work, which is worse than no control at all.
    const poll = picker.slice(picker.indexOf('const poll = ('))
    const deps = poll.match(/\}, \[([^\]]*)\]\)/)
    ok(deps != null, "the poll effect's dependency array is readable")
    ok(
      /\blibraries\b/.test(deps?.[1] ?? ''),
      'and it names `libraries`, so flipping the scope re-asks the current query rather than ' +
        'waiting for the next keystroke',
    )
  }
  ok(
    /aria-pressed/.test(picker),
    'the mouse affordance is an `aria-pressed` button, matching `SearchPanel`’s precedent — ' +
      'nothing in the overlay language uses a checkbox',
  )
  ok(
    /onMouseDown/.test(picker) && !/<button[\s\S]{0,400}?onClick=\{\(\) => onToggle/.test(picker),
    'and it commits on `mousedown` with `preventDefault`, not on `click`: a click blurs the ' +
      'input between the two events and ModalShell’s selectionchange listener repaints the ' +
      'caret, after which typing stops working',
  )
  ok(
    /⌥L/.test(picker),
    'and the same element prints the chord, so one control is both the documentation and the ' +
      'mouse target',
  )
  ok(
    /hit\.source/.test(picker),
    'a library row draws its provenance. Two rows reading `RS lib.rs src/lib.rs` is the ' +
      'failure the whole scope has to avoid, and the package name is the only thing that ' +
      'separates them',
  )

  const overlayCss = uiFile('src/overlays/Overlay.module.css')
  ok(
    /\.source \{[^}]*flex: none/.test(overlayCss),
    'and the provenance chip does not shrink. `.path` is `flex: 1` and ellipsises first, ' +
      'deliberately: a path is reconstructible from a name and a package, and a package name ' +
      'cut in half is not',
  )

  /*
   * Two Rust decisions with no behavioural gate, asserted at source level because the
   * alternatives cost more than they are worth — one is a wall-clock measurement over the
   * user's own cargo registry, the other is a watcher-burst path with no test harness. Both
   * were found by mutation: the code compiled, every test passed, and the feature was 20×
   * slower / silently stale.
   */
  const repoFile = (rel) =>
    readFileSync(fileURLToPath(new URL(`../../${rel}`, import.meta.url)), 'utf8')
  const stripRust = (src) => strip(src.split(/#\[cfg\(test\)\]/)[0])

  {
    const files = stripRust(repoFile('crates/cide-app/src/files.rs'))
    const walk = files.slice(files.indexOf('Index::walk_roots('))
    ok(
      /threads:\s*1/.test(walk.slice(0, 400)),
      'the library walk asks for ONE thread per package. `BuildOptions::default()` is ' +
        '`threads: 0`, which lets `ignore` spin one walker per core — right for a handful of ' +
        'project roots and measured at 1.24 s against 130 ms for 593 package directories on ' +
        'this machine, all of it in spawning and joining 32 threads per directory. Nothing ' +
        'else in the suite can see a 20× slowdown',
    )
    ok(
      /libraries_walked/.test(files) && /forget_libraries/.test(files),
      'and the walk is guarded and reversible — the flags behind "at most once per project" ' +
        'and "the lockfile moved"',
    )
  }

  ok(
    /fs\.forget_libraries\(\)/.test(stripRust(repoFile('crates/cide-app/src/cmd/fs.rs'))),
    'a stale lockfile drops the library candidates. `cargo update` moves versions and ' +
      '`cargo remove` deletes a directory the matcher still offers, so without this Ctrl+P ' +
      'keeps opening files that no longer exist until the app is relaunched. Its only caller ' +
      'is the watcher-burst path, which has no test harness',
  )

  const store = strip(uiFile('src/overlays/store.ts'))
  ok(
    /libraries: false/.test(store),
    'the scope is OFF at every launch — the request’s own words. A `true` here would make ' +
      '"disabled by default" false for everybody after the first flip',
  )
  ok(
    /export function filePickerOpen/.test(store),
    'and `filePickerOpen` is derived here rather than as a comparison at the call site: ' +
      '`overlayOpen` is true for any of nine overlays, and ⌥L scoped to that would be ' +
      'swallowed in a terminal pane whenever a menu happened to be up',
  )

  const dispatch = strip(uiFile('src/keys/dispatch.ts'))
  ok(
    /case 'picker\.libraries':/.test(dispatch),
    "the command is dispatched. `check:commands` requires every id to be handled here or to " +
      'carry an `unavailable` reason — listed-and-silently-inert is the state that check ' +
      'makes unrepresentable',
  )
  ok(
    /toggleLibraries\(\)/.test(dispatch),
    'and it flips the flag rather than opening something',
  )

  if (failed > 0) {
    console.error(`\ncheck-picker: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-picker: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
