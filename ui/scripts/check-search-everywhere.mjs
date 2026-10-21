/** Search result identity, provider limits, Unicode locations and modifier-only opening. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-search-everywhere-'))
try {
  execFileSync('node', ['node_modules/typescript/bin/tsc', 'src/search/model.ts', 'src/keys/doubleShift.ts',
    '--rootDir', 'src', '--outDir', out, '--module', 'commonjs', '--target', 'es2022', '--lib', 'es2023',
    '--strict', '--skipLibCheck'], { stdio: 'inherit' })
  const require = createRequire(import.meta.url)
  const { results, resultRows, fileGroupOwners, location, needsAnotherPage } = require(join(out, 'search/model.js'))
  const { createDoubleShift } = require(join(out, 'keys/doubleShift.js'))
  const frame = { files: { items: Array.from({ length: 30 }, (_, i) => ({ value: `/file${i}`, text: `file${i}` })) }, symbols: null,
    text: { hits: [{ path: '/utf8', rel: 'utf8', text: 'é😀 alpha', line: 5, start: 7, end: 12 }], total: 450 }, running: false }
  const items = results(frame)
  assert.deepEqual(location(items.at(-1)), { path: '/utf8', line: 5, column: 5, endColumn: 10 })
  const all = resultRows(items, 'all', new Set(), false)
  assert.equal(all.filter(r => r.kind === 'file').length, 20)
  assert.equal(all.find(r => r.kind === 'more').mode, 'files')
  assert.equal(all.find(r => r.key === 'provider:text').level, 'provider')
  assert.equal(all.find(r => r.key === 'path:/utf8').level, 'file')
  assert.equal(resultRows(items, 'all', new Set(), true).filter(r => r.kind === 'file').length, 30)
  assert.equal(resultRows(items, 'files', new Set(), false).length, 30)
  assert.equal(resultRows(items, 'all', new Set(['provider:files', 'path:/utf8']), true).length, 3)
  assert.equal(needsAnotherPage(frame, 200), true, 'a completed worker still has unread pages')
  assert.equal(needsAnotherPage(frame, 450), false)
  assert.equal(needsAnotherPage({ ...frame, running: true }, 450), true)
  assert.equal(items[0].key, results({ ...frame, text: { ...frame.text, hits: [] } })[0].key)

  const grouped = results({ ...frame, text: { hits: Array.from({ length: 25 }, (_, i) => ({
    ...frame.text.hits[0], path: i < 22 ? '/first' : '/second', rel: i < 22 ? 'first' : 'second', line: i + 1,
  })) } })
  const pinnedRows = resultRows(grouped, 'all', new Set(), true)
  const owners = fileGroupOwners(pinnedRows)
  const first = pinnedRows.findIndex(r => r.key === 'path:/first')
  const second = pinnedRows.findIndex(r => r.key === 'path:/second')
  assert(owners.slice(0, first).every(owner => owner === null), 'providers and filename matches have no sticky file context')
  assert(owners.slice(first, second).every(owner => owner.header === first && owner.end === second))
  assert(owners.slice(second).every(owner => owner.header === second && owner.end === pinnedRows.length))
  const limited = resultRows(grouped, 'all', new Set(), false)
  assert.equal(fileGroupOwners(limited).at(-1), null, 'the show-more row ends the file context')
  assert.equal(fileGroupOwners(limited).at(-2).end, limited.length - 1)
  const collapsedRows = resultRows(grouped, 'text', new Set(['path:/first']), false)
  assert.deepEqual(fileGroupOwners(collapsedRows)[0], { header: 0, end: 1 }, 'collapsed headings cannot stick over the next file')
  assert.deepEqual(fileGroupOwners([]), [])

  let time = 0
  const detector = createDoubleShift(() => time)
  const event = (type, extra = {}) => ({ type, key: 'Shift', code: 'ShiftLeft', ctrlKey: false, altKey: false, metaKey: false, ...extra })
  const tap = () => { detector.feed(event('keydown')); time += 40; return detector.feed(event('keyup')) }
  assert.equal(tap(), false)
  time += 100
  assert.equal(tap(), true)
  assert.equal(tap(), false, 'a third tap begins another gesture')
  time += 401
  assert.equal(tap(), false, 'taps outside the interval cannot combine')
  for (const extra of [{ code: 'KeyA', key: 'A' }, { ctrlKey: true }, { altKey: true }, { metaKey: true }, { isComposing: true }, { repeat: true }]) {
    detector.reset(); tap(); detector.feed(event('keydown', extra)); detector.feed(event('keyup', extra))
    assert.equal(tap(), false, `interrupted tap: ${JSON.stringify(extra)}`)
  }
  detector.reset(); tap(); detector.reset(); assert.equal(tap(), false, 'blur/pointer/composition reset')
  detector.reset(); detector.feed(event('keydown')); time += 401; detector.feed(event('keyup'))
  assert.equal(tap(), false, 'holding Shift is not a tap')
  detector.reset(); detector.feed(event('keydown')); detector.feed(event('keydown', { code: 'ShiftRight' }))
  detector.feed(event('keyup')); detector.feed(event('keyup', { code: 'ShiftRight' }))
  assert.equal(tap(), false, 'overlapping Shift keys are not two isolated taps')
  console.log('search everywhere: provider grouping, sticky file boundaries, independent result identities, completed pagination, UTF-8 locations and double Shift passed')
} finally { rmSync(out, { recursive: true, force: true }) }
