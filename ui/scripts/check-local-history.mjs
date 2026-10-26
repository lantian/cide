import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, rmSync, readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { resolve, join } from 'node:path'
import { pathToFileURL } from 'node:url'
const ui = resolve(import.meta.dirname, '..')
const temp = mkdtempSync(join(tmpdir(), 'cide-history-model-'))
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const ssr = mkdtempSync(join(ui, 'node_modules/.cache/history-render-'))
try {
  execFileSync('node', [join(ui, 'node_modules/typescript/bin/tsc'), join(ui, 'src/localHistory/model.ts'), '--target', 'ES2022', '--module', 'commonjs', '--skipLibCheck', '--outDir', temp], { cwd: ui, stdio: 'inherit' })
  const { HistoryRequests, comparisonMatches, policyFromForm, appendUnique } = createRequire(import.meta.url)(join(temp, 'model.js'))
  const requests = new HistoryRequests(); const old = requests.begin(); requests.cancel(); const current = requests.begin()
  assert.equal(requests.accepts(old), false, 'old file selection cannot replace the current result')
  assert.equal(requests.accepts(current), true)
  requests.cancel(); assert.equal(requests.accepts(current), false, 'clear cancels in-flight selections')
  const comparison = { token: { generation: 2, path: 'a', target: 'revision-a' }, right: { revision: null } }
  assert.equal(comparisonMatches(comparison, 'a', 'revision-a', null, 2), true)
  for (const args of [['b', 'revision-a', null, 2], ['a', 'revision-b', null, 2], ['a', 'revision-a', 'revision-b', 2], ['a', 'revision-a', null, 3]]) assert.equal(comparisonMatches(comparison, ...args), false, 'stale preview cannot enable restore')
  assert.deepEqual(appendUnique([{ id: 'a' }], [{ id: 'a' }, { id: 'b' }], (r) => r.id), [{ id: 'a' }, { id: 'b' }])
  assert.deepEqual(policyFromForm(false, '7', '256', 'private/**\n**/*.secret\n'), { enabled: false, retentionDays: 7, budgetBytes: 268435456, exclusions: ['private/**', '**/*.secret'] })
  for (const days of ['', '0', '-1', '1.5', 'Infinity', '4294967296']) assert.throws(() => policyFromForm(true, days, '256', ''))
  for (const budget of ['', '0', '-1', 'Infinity', '4096']) assert.throws(() => policyFromForm(true, '7', budget, ''))
  for (const exclusion of ['../secret', '/absolute', 'a/../secret', 'bad\\path']) assert.throws(() => policyFromForm(true, '7', '256', exclusion))
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr', 'src/localHistory/smokeEntry.tsx', '--outDir', ssr, '--logLevel', 'error'], { cwd: ui, stdio: 'inherit' })
  const { renderHistoryStories } = await import(pathToFileURL(join(ssr, 'smokeEntry.js')))
  const stories = renderHistoryStories()
  for (const [name, html] of Object.entries(stories)) assert(!html.includes('class="undefined"') && !/class="[^"]*\bundefined\b/.test(html), `${name} uses real CSS classes`)
  assert.match(stories.ready, /External observation/); assert.match(stories.ready, /Renamed from old.rs/); assert.match(stories.ready, /lifecycle/)
  assert.match(stories.ready, /Deleted files only/); assert.match(stories.ready, /Load older revisions/); assert.match(stories.ready, /Load more paths/)
  assert.match(stories.empty, /No retained revisions/); assert.match(stories.loading, /Loading revisions/)
  assert.match(stories.excluded, /Capture excluded/); assert.match(stories.excluded, /Existing retained content remains/)
  assert.match(stories.degraded, /Coverage degraded/); assert.match(stories.degraded, /baseline \/ reconciling/); assert.match(stories.degraded, /Reopened root/)
  assert.match(stories.error, /History storage unavailable/); assert.match(stories.comparison, /right side is absent/)
  assert.match(stories.unavailable, /Selected revision unavailable: Missing blob/); assert.match(stories.unavailable, /disabled=""/)
  assert.match(stories.settings, /7 days/); assert.match(stories.settings, /256 MiB/); assert.match(stories.settings, /Used space/); assert.match(stories.settings, /Clear history/); assert.match(stories.settings, /already retained content remains/)
  // Enforce the live gates on both CodeMirror paths, without starting a browser.
  const strip = (s) => s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
  const diff = strip(readFileSync(join(ui, 'src/localHistory/HistoryTextDiff.tsx'), 'utf8'))
  assert.match(diff, /mergeControls: false/); assert.match(diff, /EditorState\.readOnly\.of\(true\)/); assert.match(diff, /EditorView\.editable\.of\(false\)/)
  const gate = strip(readFileSync(join(ui, 'src/editor/useHistoryRestoreGate.ts'), 'utf8'))
  const prepare = gate.indexOf("event.phase === 'prepare'"); const freeze = gate.indexOf('handle.current?.(true)', prepare); const acknowledge = gate.indexOf('localHistory.acknowledge')
  assert(prepare >= 0 && freeze > prepare && acknowledge > freeze, 'freeze precedes acknowledgement')
  console.log('local history: behavior and SSR checks passed')
} finally { rmSync(temp, { recursive: true, force: true }); rmSync(ssr, { recursive: true, force: true }) }
