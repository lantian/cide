/**
 * Checks `src/localhistory/model.ts`, the pure half of the Local History tab.
 *
 * `check-pools.mjs`'s recipe: compile the rules module standalone, import the emitted JavaScript
 * and drive it, plus source assertions that pin two sides of a contract together.
 *
 * What is worth pinning, each naming a failure this surface can have silently:
 *
 *   - **A row with no copy cannot be restored.** Offering it would write an empty file over the
 *     real one, which is the opposite of what this feature is for.
 *   - **Two selected rows diff older to newer.** Reversed sides draw every deletion as an addition,
 *     and a reader would restore the wrong one with confidence.
 *   - **A folder restore's report names every count.** It carries on past failures, so a one-word
 *     "done" would hide the files it could not write.
 *   - **Every source and kind Rust can produce has words.** A missing one renders `undefined`.
 *   - **The restore saves unsaved edits first**, and **the folder restore confirms with a list**,
 *     because a count is not something anyone can check before clicking.
 *
 * What this does NOT cover: anything Rust decides. Which files a restore touches and in what order
 * is `cargo test -p cide-app local_history` and `cargo test -p cide-local-history`.
 *
 * Run: `pnpm --dir ui run check:local-history`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ui = fileURLToPath(new URL('..', import.meta.url))
const out = mkdtempSync(join(tmpdir(), 'cide-local-history-'))
let failed = 0

function eq(actual, expected, what) {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    failed += 1
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
  }
}

function has(source, needle, what) {
  if (!source.includes(needle)) {
    failed += 1
    console.error(`FAIL ${what}\n  missing: ${needle}`)
  }
}

/** A comment is not behaviour, and house-style comments name the failure. */
function code(path) {
  return readFileSync(join(ui, path), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/^\s*\/\/.*$/gm, '')
}

try {
  execFileSync(
    join(ui, 'node_modules/.bin/tsc'),
    [
      '--target', 'es2022', '--module', 'es2022', '--moduleResolution', 'bundler',
      '--skipLibCheck', '--noCheck', '--outDir', out, join(ui, 'src/localhistory/model.ts'),
    ],
    { stdio: 'pipe' },
  )
  writeFileSync(join(out, 'package.json'), '{"type":"module"}')
  const model = await import(join(out, 'model.js'))

  const version = (over = {}) => ({
    id: 1, ts: 0, group: 1, path: '/p/a.rs', kind: 'modified', source: 'external', size: 10,
    kept: true, moved: null, label: null, ...over,
  })

  eq(model.restoreRefusal(version()), null, 'a kept version can be restored')
  eq(
    typeof model.restoreRefusal(version({ kept: false })),
    'string',
    'an over-limit row refuses, because restoring it would empty the file',
  )
  eq(
    model.restoreRefusal(version({ kept: false, kind: 'deleted' })).includes('earlier version'),
    true,
    'a deletion row says where the content actually is',
  )
  eq(
    typeof model.compareRefusal(version({ size: (1 << 20) + 1 })),
    'string',
    'past the editable compare’s limit the version can only be restored whole',
  )

  const older = version({ id: 3 })
  const newer = version({ id: 9 })
  for (const [a, b] of [[older, newer], [newer, older]]) {
    const pair = model.comparedPair(a, b)
    eq([pair.from.id, pair.to.id], [3, 9], 'two rows diff older to newer, in either click order')
  }
  eq(model.comparedPair(older, null).to, null, 'one row compares with the file as it is now')

  eq(
    model.reportLine({ written: 2, removed: 1, left: 3, failed: [{}] }),
    '2 files restored, 1 file moved to the trash, 3 files left as they are, 1 file failed',
    'the report names every count, failures included',
  )
  eq(
    model.reportLine({ written: 0, removed: 0, left: 0, failed: [] }),
    'Nothing needed restoring',
    'an empty restore says so',
  )

  for (const source of ['saved', 'external', 'detected', 'restored', 'fileOp']) {
    eq(typeof model.sourceLabel(source), 'string', `the source ${source} has words`)
  }
  for (const kind of ['baseline', 'baselineDone', 'created', 'modified', 'deleted', 'label']) {
    eq(typeof model.kindLabel(kind), 'string', `the kind ${kind} has words`)
  }
  eq(
    model.versionTitle(version({ kind: 'deleted', moved: 'new.rs' })),
    'Moved to new.rs',
    'the far end of a rename does not read as a deletion',
  )
  eq(
    model.groupTitle({ group: 1, ts: 0, source: 'detected', files: 12000, label: null, snapshot: true }),
    'First snapshot, 12,000 files',
    'a first scan is one line',
  )

  const day = 86_400_000
  const now = new Date(2026, 5, 15, 12).getTime()
  const lines = model.withDays(
    [{ ts: now }, { ts: now - 1000 }, { ts: now - day }, { ts: now - 3 * day }],
    now,
    (row) => String(row.ts),
  )
  eq(
    lines.filter((line) => line.kind === 'day').map((line) => line.label),
    ['Today', 'Yesterday', '2026-06-12'],
    'each day is headed once, newest first',
  )

  const actions = code('src/localhistory/actions.ts')
  has(actions, 'await saveOpenBuffers(project, path)', 'a restore saves unsaved edits first')
  const tab = code('src/localhistory/LocalHistoryTab.tsx')
  has(tab, 'files: [...plan.write, ...plan.remove]', 'a folder restore names every file at stake')
  has(tab, 'ConfirmDestructive', 'a folder restore asks before it runs')

  const rust = readFileSync(join(ui, '../crates/cide-app/src/cmd/local_history.rs'), 'utf8')
  has(
    rust,
    'crate::emit::session_tool(&app, "local-history"',
    'a restore tells open editors: nothing else re-reads a file changed outside a git operation',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`check-local-history: ${failed} failure(s)`)
  process.exit(1)
}
console.log('local history: ok')
