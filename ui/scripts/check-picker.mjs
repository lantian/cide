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
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-picker-'))
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
      'src/overlays/score.ts',
      'src/overlays/format.ts',
      'src/overlays/listKeys.ts',
      'src/overlays/gotoLine.ts',
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
  const { groupDigits, matchCounter, basename, dirname, kindBadge } = require(
    join(out, 'overlays/format.js'),
  )
  const { listAction, PAGE_ROWS } = require(join(out, 'overlays/listKeys.js'))
  const { canGo, gotoNote, parseGoto } = require(join(out, 'overlays/gotoLine.js'))
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

  if (failed > 0) {
    console.error(`\ncheck-picker: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-picker: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
