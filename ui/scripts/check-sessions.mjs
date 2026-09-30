/**
 * Checks `src/sidebar/AgentsPanel/sessionsModel.ts` — the Sessions tab's pure core (M134) — and
 * pins its kind vocabulary against `cide_ipc::sessions::SessionKind`.
 *
 * `check-agents.mjs`' shape and reason: no JS test runner, and the module is import-free so the
 * TypeScript in `node_modules` compiles it alone and node imports the result.
 *
 * # What it makes unrepresentable
 *
 * * **A kind from Rust the tab cannot name.** A tenth `SessionKind` would arrive with no label and
 *   no place in the filter row; `SESSION_KINDS` must be exactly Rust's variants, camelCased.
 * * **A label lookup that misses.** Every kind has a non-empty label, and `'constructor'` reads
 *   as itself — not as `Object.prototype.constructor` stringified into a chip.
 * * **A search that hides what it should find.** Every word must match somewhere across the
 *   fields; a transcript hit keeps a row the text search would drop, so turning transcripts on
 *   only ever adds rows; and the kind filter composes with both.
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

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/AgentsPanel/sessionsModel.ts',
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
  const m = await import(`file://${join(out, 'sessionsModel.js')}`)

  // --- the vocabulary, against Rust ---------------------------------------------------------
  const rust = stripComments(readFileSync(resolve(UI, '../crates/cide-ipc/src/sessions.rs'), 'utf8'))
  const start = rust.indexOf('pub enum SessionKind')
  ok(start >= 0, 'found `pub enum SessionKind` in cide-ipc/src/sessions.rs')
  const body = rust.slice(rust.indexOf('{', start) + 1, rust.indexOf('\n}', start))
  const variants = [...body.matchAll(/^[ \t]+([A-Z][A-Za-z0-9]*)[ \t]*,/gm)].map(
    (v) => v[1].charAt(0).toLowerCase() + v[1].slice(1),
  )
  ok(variants.length >= 9, `read at least nine SessionKind variants (read ${variants.length})`)
  eq([...m.SESSION_KINDS].sort(), [...variants].sort(), 'SESSION_KINDS is exactly Rust’s SessionKind')
  for (const kind of m.SESSION_KINDS) {
    const label = m.kindLabel(kind)
    ok(typeof label === 'string' && label !== '' && label !== kind, `${kind} has a label of its own`)
  }
  eq(m.kindLabel('constructor'), 'constructor', 'a prototype key reads as itself')
  eq(m.kindLabel('someNewKind'), 'someNewKind', 'an unknown kind reads as itself')
  eq(m.harnessLabel('codex'), 'Codex', 'harnesses are labelled')
  eq(m.harnessLabel('toString'), 'toString', 'a prototype key is not a harness label')

  // --- the filter ------------------------------------------------------------------------------
  const base = {
    name: null, prompt: null, task: null, taskTitle: null, agent: null, branch: null,
    cwd: '/p', startedMs: 0, lastSeenMs: 0, live: false, pane: null, searchable: true,
  }
  const rows = [
    { ...base, id: 'a', kind: 'console', harness: 'claude', title: 'proj : claude', prompt: 'Add a Sessions tab' },
    { ...base, id: 'b', kind: 'subagent', harness: 'claude', title: 'Coder', agent: 'Coder', task: 't-14', taskTitle: 'Split the renderer' },
    { ...base, id: 'c', kind: 'tab', harness: 'codex', title: 'Tab', name: 'gate retry' },
  ]
  const ids = (list) => list.map((v) => v.id)
  const none = new Set()
  eq(ids(m.filterSessions(rows, { query: '', kinds: none, agent: null }, null)), ['a', 'b', 'c'], 'no filter keeps every row, in order')
  eq(ids(m.filterSessions(rows, { query: 'SESSIONS', kinds: none, agent: null }, null)), ['a'], 'the search is case-insensitive and reads the prompt')
  eq(ids(m.filterSessions(rows, { query: 'coder t-14', kinds: none, agent: null }, null)), ['b'], 'every word must match, across fields')
  eq(ids(m.filterSessions(rows, { query: 'coder t-99', kinds: none, agent: null }, null)), [], '…all of them')
  eq(ids(m.filterSessions(rows, { query: 'gate', kinds: none, agent: null }, null)), ['c'], 'the /rename name is searched')
  eq(ids(m.filterSessions(rows, { query: 'subagent', kinds: none, agent: null }, null)), ['b'], 'the kind label is searched')
  eq(
    ids(m.filterSessions(rows, { query: 'renderer', kinds: none, agent: null }, { a: 'we split the renderer' })),
    ['a', 'b'],
    'a transcript hit keeps a row the text search would drop, and drops none it kept',
  )
  eq(
    ids(m.filterSessions(rows, { query: 'renderer', kinds: new Set(['console']), agent: null }, { a: '…' })),
    ['a'],
    'the kind filter composes with both searches',
  )
  eq(
    ids(m.filterSessions(rows, { query: '', kinds: new Set(['tab', 'console']), agent: null }, null)),
    ['a', 'c'],
    'several kinds can be picked at once',
  )
  const runs = [
    ...rows,
    { ...base, id: 'd', kind: 'subagent', harness: 'codex', title: 'Artist', agent: 'Artist', task: 't-2' },
    { ...base, id: 'e', kind: 'subagent', harness: 'codex', title: 'Artist', agent: 'Artist', task: 't-3' },
  ]
  eq(
    ids(m.filterSessions(runs, { query: '', kinds: none, agent: 'Artist' }, null)),
    ['d', 'e'],
    'the role filter keeps only that subagent’s runs',
  )
  eq(
    ids(m.filterSessions(runs, { query: 't-3', kinds: new Set(['subagent']), agent: 'Artist' }, null)),
    ['e'],
    '…and composes with the search and the kind filter',
  )
  eq(
    m.agentCounts(runs),
    [{ agent: 'Artist', count: 2 }, { agent: 'Coder', count: 1 }],
    'agentCounts lists the roles present, most runs first, and skips rows with no role',
  )
  eq(
    m.kindCounts(rows),
    [{ kind: 'console', count: 1 }, { kind: 'tab', count: 1 }, { kind: 'subagent', count: 1 }],
    'kindCounts lists only kinds present, in SESSION_KINDS order',
  )
  eq(m.displayTitle(rows[2]), 'gate retry', 'a /rename name wins over the title')
  eq(m.displayTitle({ ...rows[0], title: '  ' }), 'a', 'with neither, the id')

  // --- when ------------------------------------------------------------------------------------
  const now = Date.UTC(2026, 8, 30, 12)
  eq(m.ago(now, now - 5_000), 'just now', 'under a minute is just now')
  eq(m.ago(now, now - 5 * 60_000), '5m ago', 'minutes')
  eq(m.ago(now, now - 3 * 3_600_000), '3h ago', 'hours')
  eq(m.ago(now, now - 2 * 86_400_000), '2d ago', 'days')
  eq(m.ago(now, Date.UTC(2026, 0, 2)), '2026-01-02', 'past a month, the date')
  eq(m.ago(now, now + 60_000), 'just now', 'a clock skewed ahead is not negative time')

  if (failed > 0) {
    console.error(`\ncheck-sessions: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`check-sessions: ok (${variants.length} kinds pinned to Rust)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
