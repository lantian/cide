/**
 * Checks the Sessions tab's pure core — `src/sidebar/AgentsPanel/sessionsModel.ts` (M134) and
 * `sessionsTree.ts` (M199) — and pins its vocabularies against `cide_ipc::sessions`.
 *
 * `check-agents.mjs`' shape and reason: no JS test runner, and the modules are import-free (the
 * tree imports only types from the model) so the TypeScript in `node_modules` compiles them alone
 * and node imports the result. `model.ts` is compiled beside them because the status table is
 * pinned against its run-phase glyphs.
 *
 * # What it makes unrepresentable
 *
 * * **A kind or activity from Rust the tab cannot draw.** `SESSION_KINDS` and `SESSION_ACTIVITIES`
 *   must be exactly Rust's variants, camelCased, and every one has words and a vendored glyph.
 * * **A run that reads differently on the two tabs.** Each activity with a run-phase counterpart
 *   has that phase's glyph and tone.
 * * **A session in two places, or in none.** Grouped rows are in their group (live or ended),
 *   loose live rows at the root, everything else in Closed — and a membership naming a group that
 *   is gone reads as no group.
 * * **A drop that makes a cycle, or claims to move what it does not.** A group into its own
 *   subtree is refused with words; a no-op says so; a move carries the edit Rust is sent.
 * * **A waiting count that counts one question twice.**
 * * **A search that hides what it should find.** Every word must match somewhere; a transcript
 *   hit only ever adds rows; a hit inside a folded group opens it.
 *
 * Run: `pnpm --dir ui run check:sessions`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-sessions-'))

let failed = 0
const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (cond !== true) fail(what)
}

/** Line comments out, block comments out: the Rust doc comments name variants in prose. */
const stripComments = (source) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

/** A Rust enum's unit variants, camelCased. */
const variantsOf = (rust, name) => {
  const start = rust.indexOf(`pub enum ${name}`)
  ok(start >= 0, `found \`pub enum ${name}\` in cide-ipc/src/sessions.rs`)
  const body = rust.slice(rust.indexOf('{', start) + 1, rust.indexOf('\n}', start))
  return [...body.matchAll(/^[ \t]+([A-Z][A-Za-z0-9]*)[ \t]*,/gm)].map(
    (v) => v[1].charAt(0).toLowerCase() + v[1].slice(1),
  )
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/AgentsPanel/sessionsModel.ts',
      'src/sidebar/AgentsPanel/sessionsTree.ts',
      'src/sidebar/AgentsPanel/model.ts',
      '--rootDir', 'src',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  const dir = join(out, 'sidebar', 'AgentsPanel')
  const m = await import(`file://${join(dir, 'sessionsModel.js')}`)
  const t = await import(`file://${join(dir, 'sessionsTree.js')}`)
  const runModel = await import(`file://${join(dir, 'model.js')}`)

  // --- the vocabularies, against Rust ---------------------------------------------------------
  const rust = stripComments(readFileSync(resolve(UI, '../crates/cide-ipc/src/sessions.rs'), 'utf8'))
  const kinds = variantsOf(rust, 'SessionKind')
  ok(kinds.length >= 9, `read at least nine SessionKind variants (read ${kinds.length})`)
  eq([...m.SESSION_KINDS].sort(), [...kinds].sort(), 'SESSION_KINDS is exactly Rust’s SessionKind')
  for (const kind of m.SESSION_KINDS) {
    const label = m.kindLabel(kind)
    ok(typeof label === 'string' && label !== '' && label !== kind, `${kind} has a label of its own`)
  }
  eq(m.kindLabel('constructor'), 'constructor', 'a prototype key reads as itself')
  eq(m.kindLabel('someNewKind'), 'someNewKind', 'an unknown kind reads as itself')
  eq(m.harnessLabel('codex'), 'Codex', 'harnesses are labelled')
  eq(m.harnessLabel('toString'), 'toString', 'a prototype key is not a harness label')

  const activities = variantsOf(rust, 'SessionActivity')
  ok(activities.length >= 10, `read at least ten SessionActivity variants (read ${activities.length})`)
  eq(
    [...m.SESSION_ACTIVITIES].sort(),
    [...activities].sort(),
    'SESSION_ACTIVITIES is exactly Rust’s SessionActivity',
  )
  const iconSource = readFileSync(resolve(UI, 'src/icons/iconPaths.ts'), 'utf8')
  for (const status of [...m.SESSION_ACTIVITIES, 'question']) {
    const look = m.statusLook(status)
    ok(look.label !== 'Unknown' && look.label !== '', `${status} has words`)
    ok(iconSource.includes(`'${look.glyph}':`), `${status}'s glyph ${look.glyph} is vendored`)
  }
  eq(m.statusLook('constructor').label, 'Unknown', 'a prototype key is not a status')
  // The same glyph and tone as the run phase each corresponds to — one look for a run, both tabs.
  const pairs = {
    starting: 'starting',
    working: 'running',
    idle: 'idle',
    permission: 'awaitingPermission',
    paused: 'paused',
    blocked: 'blocked',
    failed: 'failed',
    finished: 'finished',
    stopped: 'cancelled',
  }
  for (const [status, phase] of Object.entries(pairs)) {
    eq(
      [m.statusLook(status).glyph, m.statusLook(status).tone],
      [runModel.phaseGlyph(phase), runModel.phaseTone(phase)],
      `${status} looks like the run phase ${phase}`,
    )
  }
  eq(
    ['awaitingInput', 'question', 'permission', 'working', 'idle', 'blocked'].map(m.isWaiting),
    [true, true, true, false, false, false],
    'only what the user owes counts as waiting — not a blocked run, which waits on a fix',
  )

  // --- names and the search --------------------------------------------------------------------
  const base = {
    name: null, prompt: null, task: null, taskTitle: null, agent: null, branch: null, cwd: '/p',
    startedMs: 0, lastSeenMs: 0, live: false, liveSession: null, run: null, pane: null,
    searchable: true, activity: 'finished', group: null, label: null,
  }
  const row = (o) => ({ ...base, kind: 'tab', harness: 'claude', title: 'Tab', ...o })
  const rows = [
    row({ id: 'a', kind: 'console', title: 'proj : claude', prompt: 'Add a Sessions tab' }),
    row({ id: 'b', kind: 'subagent', title: 'Coder', agent: 'Coder', task: 't-14', taskTitle: 'Split the renderer' }),
    row({ id: 'c', harness: 'codex', name: 'gate retry' }),
  ]
  eq(m.displayName(rows[0]), 'Add a Sessions tab', 'a generic "project : harness" title gives way to the first prompt')
  eq(m.displayName(rows[2]), 'gate retry', 'a /rename name wins over the title')
  eq(m.displayName({ ...rows[2], label: 'mine' }), 'mine', 'cide’s label wins over the /rename name')
  eq(m.displayName(row({ id: 'z', title: 'my proj : codex' })), 'my proj : codex', 'with nothing better, the generic title')
  ok(m.hasOnlyGenericName(row({ id: 'z', title: 'p : codex' })), '…which is flagged for a dimmer drawing')
  eq(m.displayName(row({ id: 'z', title: '  ' })), 'z', 'with nothing at all, the id')
  const kept = (query, hits = null) => rows.filter((v) => m.keeps(v, query, hits)).map((v) => v.id)
  eq(kept(''), ['a', 'b', 'c'], 'no words keep every row, in order')
  eq(kept('SESSIONS'), ['a'], 'the search is case-insensitive and reads the prompt')
  eq(kept('coder t-14'), ['b'], 'every word must match, across fields')
  eq(kept('coder t-99'), [], '…all of them')
  eq(kept('gate'), ['c'], 'the /rename name is searched')
  eq(kept('subagent'), ['b'], 'the kind’s label is searched — the kind chips’ replacement')
  eq(kept('codex'), ['c'], 'the harness is searched')
  eq(kept('renderer', { a: 'we split the renderer' }), ['a', 'b'], 'a transcript hit only ever adds rows')

  // --- where a row is --------------------------------------------------------------------------
  const groups = [
    { id: 'g1', name: 'Release', parent: null, collapsed: false },
    { id: 'g2', name: 'Menus', parent: 'g1', collapsed: false },
    { id: 'g3', name: 'Ideas', parent: null, collapsed: true },
  ]
  const views = [
    row({ id: 'live-loose', live: true, activity: 'working', lastSeenMs: 9 }),
    row({ id: 'in-menus', live: true, group: 'g2', activity: 'awaitingInput', lastSeenMs: 8 }),
    row({ id: 'ended-in-release', group: 'g1', lastSeenMs: 7 }),
    row({ id: 'idea', group: 'g3', live: true, activity: 'working', lastSeenMs: 6 }),
    row({ id: 'idea-asks', group: 'g3', live: true, activity: 'idle', lastSeenMs: 5 }),
    row({ id: 'ended', lastSeenMs: 4 }),
    row({ id: 'orphan', group: 'gone', lastSeenMs: 3 }),
    row({ id: 'orphan-live', group: 'gone', live: true, activity: 'idle', lastSeenMs: 2 }),
  ]
  eq(
    views.map((v) => t.placeOf(v, groups)),
    ['loose', 'g2', 'g1', 'g3', 'g3', 'closed', 'closed', 'loose'],
    'one place each: a group (live or ended), loose when live, else Closed; a lost group is no group',
  )
  const questioned = m.questionedIds(views, [{ origin: 'idea-asks', open: true }, { origin: 'ended', open: false }], new Set())
  eq([...questioned], ['idea-asks'], 'only an open request marks its conversation questioned')
  eq(m.rowStatus(views[4], questioned), 'question', 'a live questioned row reads as a question')
  eq(m.rowStatus(views[5], new Set(['ended'])), 'finished', '…an ended one keeps what Rust said')
  eq(
    [...m.questionedIds([row({ id: 'r', task: 't-9', live: true })], [], new Set(['t-9']))],
    ['r'],
    'a run is questioned by the open board question on its task',
  )
  const lens = (query = '', hits = null) => ({
    keep: (v) => m.keeps(v, query, hits),
    status: (v) => m.rowStatus(v, questioned),
    searching: m.searchWords(query).length > 0,
    words: m.searchWords(query),
  })
  const draw = (items) =>
    items.map((i) =>
      i.type === 'group'
        ? // The roll-up is drawn on a folded group only: an open one shows what it holds.
          `${'.'.repeat(i.depth)}[${i.group.name}${i.open ? '' : '+'} ${i.count}${!i.open && i.working ? ' w' : ''}${!i.open && i.waiting ? ` ?${i.waiting}` : ''}]`
        : i.type === 'rule'
          ? '---'
          : `${'.'.repeat(i.depth)}${i.id}`,
    )
  eq(
    draw(t.buildOpened(views, groups, lens())),
    ['[Release 2]', '.[Menus 1]', '..in-menus', '.ended-in-release', '[Ideas+ 2 w ?1]', '---', 'live-loose', 'orphan-live'],
    'groups in order, each with its subgroups then its sessions; a folded group rolls up; a rule; the loose live rows',
  )
  eq(
    draw(t.buildOpened(views, groups, lens('idea asks'))),
    ['[Ideas 2]', '.idea-asks'],
    'a search opens the folded group it finds something in (no roll-up once open), and shows only what it found',
  )
  eq(draw(t.buildOpened(views, groups, lens('menus'))), ['[Release 2]', '.[Menus 1]', '..in-menus'], 'a group’s name is searched, and keeps what it holds')
  eq(draw(t.buildOpened(views, [], lens())).includes('---'), false, 'no rule with no groups')
  const cycle = [
    { id: 'x', name: 'X', parent: 'y', collapsed: false },
    { id: 'y', name: 'Y', parent: 'x', collapsed: false },
  ]
  eq(draw(t.buildOpened([], cycle, lens())), [], 'a cycle that reached the file draws nothing rather than hanging')
  eq(t.tabCounts(views, groups, lens().keep), { opened: 6, closed: 2 }, 'the sub-tabs count every row the search keeps')
  const now = new Date(2026, 10, 10, 15).getTime()
  const at = (d, h) => new Date(2026, 10, d, h).getTime()
  const dayed = [row({ id: 'p', lastSeenMs: at(10, 1) }), row({ id: 'q', lastSeenMs: at(9, 23) }), row({ id: 's', lastSeenMs: at(1, 12) })]
  eq(
    t.buildClosed(dayed, groups, () => true, (v) => m.dayOf(now, v.lastSeenMs)).map((b) => [b.day, b.views.map((v) => v.id)]),
    [['today', ['p']], ['yesterday', ['q']], ['earlier', ['s']]],
    'Closed is captioned by calendar day — an hour before midnight was yesterday',
  )

  // --- the waiting counts ----------------------------------------------------------------------
  eq(
    m.waitingSummary(views, questioned, 3),
    { sessions: 2, total: 4 },
    'two sessions wait (input, a question); the question is also one of Waiting’s three, so 3 + 2 − 1',
  )
  eq(m.waitingSummary([], new Set(), 0), { sessions: 0, total: 0 }, 'nothing waits')

  // --- dropping --------------------------------------------------------------------------------
  const drop = (load, target) => t.dropOutcome(load, target, views, groups)
  const s = (id) => ({ type: 'session', id })
  const g = (id) => ({ type: 'group', id })
  eq(drop(s('live-loose'), { kind: 'group', id: 'g2' }), {
    kind: 'move', hint: 'Move to “Release / Menus”', edit: { op: 'assign', conversation: 'live-loose', group: 'g2' },
  }, 'a session into a group carries the assign Rust is sent, and names the group where it sits')
  eq(drop(s('in-menus'), { kind: 'group', id: 'g2' }).kind, 'noop', 'into its own group is a no-op')
  eq(drop(s('in-menus'), { kind: 'root' }).edit, { op: 'assign', conversation: 'in-menus', group: null }, 'onto the loose area unfiles it')
  ok(/goes to Closed/.test(drop(s('ended-in-release'), { kind: 'root' }).hint), 'unfiling an ended one says where it goes')
  eq(drop(s('ended'), { kind: 'root' }).kind, 'refuse', 'a closed session cannot be loose')
  eq(drop(s('ended'), { kind: 'tab' }).kind, 'spring', 'over the Opened tab, a closed row waits for it to open')
  eq(drop(g('g1'), { kind: 'group', id: 'g2' }), { kind: 'refuse', hint: '“Release” cannot go inside itself' }, 'a group into its own child is refused, in words')
  eq(drop(g('g1'), { kind: 'group', id: 'g1' }).kind, 'refuse', '…and into itself')
  eq(drop(g('g2'), { kind: 'group', id: 'g1' }).kind, 'noop', 'into the parent it has is a no-op')
  eq(drop(g('g3'), { kind: 'group', id: 'g2' }).edit, { op: 'move', id: 'g3', parent: 'g2', before: null }, 'nesting a group')
  eq(drop(g('g3'), { kind: 'line', id: 'g1', where: 'before' }).edit, { op: 'move', id: 'g3', parent: null, before: 'g1' }, 'placing a group before a sibling')
  eq(drop(g('g1'), { kind: 'line', id: 'g3', where: 'after' }).edit, { op: 'move', id: 'g1', parent: null, before: null }, 'after the last sibling is last')
  eq(drop(g('g1'), { kind: 'line', id: 'g2', where: 'before' }).kind, 'refuse', 'beside its own child is inside itself')
  eq(drop(g('g2'), { kind: 'root' }).edit, { op: 'move', id: 'g2', parent: null, before: null }, 'onto the loose area: to the top level')
  eq(drop(g('g1'), { kind: 'root' }).kind, 'noop', '…unless it is there')
  eq(drop(s('live-loose'), null), null, 'over nothing, no verdict')
  eq(t.springKey({ kind: 'group', id: 'g3' }, groups), 'group:g3', 'a folded group springs open')
  eq(t.springKey({ kind: 'group', id: 'g1' }, groups), null, 'an open one does not')
  eq(t.springKey({ kind: 'tab' }, groups), 'tab', 'the Opened tab springs')

  // --- the store loads at all ----------------------------------------------------------------------
  // zustand runs a store's initializer inside `create`, at module load, so any `const` it reads must
  // be declared above the `create` call. Below it, Vite's dev server (which serves modules
  // unbundled) throws on the read and the whole shell renders nothing; a production bundle rewrites
  // the `const` to a hoisted `var` and hides it, which is why only `./run.sh` ever showed it (M199).
  {
    const store = stripComments(readFileSync(resolve(UI, 'src/sidebar/sessionsStore.ts'), 'utf8'))
    const createAt = store.indexOf('create<SessionsStore>(')
    const body = store.slice(createAt, store.indexOf('\n})', createAt))
    const read = [...new Set([...body.matchAll(/\b(NO_[A-Z_]+|[A-Z][A-Z0-9_]{3,})\b/g)].map((m) => m[1]))]
    ok(createAt > 0, 'found the sessions store’s `create` call')
    for (const name of read) {
      const declared = store.search(new RegExp(`\\bconst ${name}\\b`))
      if (declared === -1) continue
      ok(declared < createAt, `sessionsStore: \`${name}\` is declared above \`create\` — read at module load, below it is a ReferenceError`)
    }
  }

  // --- the keyboard ------------------------------------------------------------------------------
  eq(
    ['ArrowDown', 'ArrowUp', 'Home', 'End', 'x'].map((k) => t.stepSelection(['a', 'b', 'c'], 'b', k)),
    ['c', 'a', 'a', 'c', null],
    'arrows step, Home and End jump, other keys move nothing',
  )
  eq(t.stepSelection(['a', 'b'], null, 'ArrowDown'), 'a', 'from no selection Down takes the first')
  eq(t.stepSelection(['a', 'b'], 'c', 'ArrowUp'), 'b', 'a selection that left the list starts from the end')

  // --- when --------------------------------------------------------------------------------------
  const noon = Date.UTC(2026, 8, 30, 12)
  eq(m.ago(noon, noon - 5_000), 'now', 'under a minute is now')
  eq(m.ago(noon, noon - 5 * 60_000), '5m', 'minutes')
  eq(m.ago(noon, noon - 3 * 3_600_000), '3h', 'hours')
  eq(m.ago(noon, noon - 2 * 86_400_000), '2d', 'days')
  eq(m.ago(noon, Date.UTC(2026, 0, 2)), '2026-01-02', 'past a month, the date')
  eq(m.ago(noon, noon + 60_000), 'now', 'a clock skewed ahead is not negative time')

  if (failed > 0) {
    console.error(`\ncheck-sessions: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`check-sessions: ok (${kinds.length} kinds and ${activities.length} activities pinned to Rust)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
