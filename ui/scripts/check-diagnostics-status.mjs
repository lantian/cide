import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createRequire } from 'node:module'

const out = mkdtempSync(join(tmpdir(), 'cide-diagnostics-status-'))
try {
  execFileSync('node', ['node_modules/typescript/bin/tsc', 'src/toolwindow/problemsModel.ts', '--outDir', out, '--module', 'commonjs', '--target', 'es2022', '--strict', '--skipLibCheck'], { stdio: 'inherit' })
  const require = createRequire(import.meta.url)
  const { DEFAULT_PROBLEMS_VIEW, diagnosticIndicator, problemGroups, problemKey, problemDescription } = require(join(out, 'toolwindow/problemsModel.js'))
  const item = { path: 'src/a.rs', absPath: '/one/src/a.rs', line: 12, column: 4, severity: 'error', message: 'mismatch', source: 'rust-analyzer', code: 'E0308' }
  const otherRoot = { ...item, absPath: '/two/src/a.rs' }
  const earlier = { ...item, line: 3, severity: 'warning', message: 'unused' }
  const ready = { kind: 'ready', source: 'rust-analyzer', items: [item, earlier, otherRoot], sources: [{ id: 'rustAnalyzer', status: { kind: 'ready' } }] }
  const groups = problemGroups(ready, DEFAULT_PROBLEMS_VIEW, null)
  assert.equal(groups.length, 2, 'identical relative names in separate roots remain separate groups')
  assert.equal(groups[0].items[0].line, 3, 'position order is the default')
  assert.equal(problemGroups(ready, { ...DEFAULT_PROBLEMS_VIEW, sort: 'severity' }, null)[0].items[0].line, 12)
  assert.equal(problemGroups(ready, { ...DEFAULT_PROBLEMS_VIEW, scope: 'file' }, '/two/src/a.rs')[0].items.length, 1)
  assert.equal(problemGroups(ready, { ...DEFAULT_PROBLEMS_VIEW, scope: 'file' }, null).length, 0)
  assert.equal(problemGroups(ready, { ...DEFAULT_PROBLEMS_VIEW, source: 'other' }, null).length, 0)
  assert.notEqual(problemKey(item), problemKey(otherRoot))
  assert.match(problemDescription(item), /src\/a.rs:12:4: error: mismatch \[E0308\] \(rust-analyzer\)/)
  assert.deepEqual(diagnosticIndicator(ready).counts, { error: 2, warning: 1, info: 0, hint: 0, other: 0 })
  assert.equal(diagnosticIndicator({ ...ready, items: [] }).clean, true)
  assert.equal(diagnosticIndicator({ ...ready, items: [], truncated: 2 }).clean, false)
  assert.equal(diagnosticIndicator({ ...ready, kind: 'scanning', items: [] }).clean, false)
  assert.equal(diagnosticIndicator({ kind: 'unavailable', reason: 'no server' }).clean, false)
  assert.equal(diagnosticIndicator({ ...ready, items: [], sources: [{ status: { kind: 'unavailable', reason: 'Stopped by user' } }] }).partial, true)
  assert.equal(diagnosticIndicator({ ...ready, items: [], sources: [{ status: { kind: 'unavailable', reason: 'Turned off in Settings ▸ Inspections.' } }] }).clean, true)
  assert.equal(diagnosticIndicator({ ...ready, items: [{ ...item, severity: 'info' }] }).clean, false)
  console.log('diagnostics status and Problems views: ok')
} finally { rmSync(out, { recursive: true, force: true }) }
