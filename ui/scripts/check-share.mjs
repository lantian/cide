/**
 * Checks `src/store/shareEqual.ts` — the structural sharing the workspace mirror, the git log and
 * the diagnostics store put every wire payload through.
 *
 * The rules are identity rules, which is exactly what a render check cannot see: an SSR pass
 * renders the same markup whether a subtree kept its object or got a new one. What breaks
 * silently in each direction:
 *
 * - **An unchanged subtree loses its identity** — nothing visible; every `memo` and dependency
 *   array keyed on it fires on every message again, and the freezes this module removed come back.
 * - **A changed subtree keeps its identity** — a real bug: a `memo`'d pane or row skips the render
 *   that would have drawn the change, and the screen shows the old value until something else
 *   moves. This is why every "changed" case below asserts a *new* object all the way to the root.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-share-'))
let failed = 0

const ok = (value, what) => {
  if (value !== true) {
    console.error(`FAIL ${what}`)
    failed++
  }
}
const deepEq = (a, b) => JSON.stringify(a) === JSON.stringify(b)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      // Import-free on purpose; see the module header.
      'src/store/shareEqual.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )
  const { shareEqual } = await import(`file://${join(out, 'shareEqual.js')}`)

  const make = () => ({
    rev: 1,
    projects: [
      { id: 'a', tabs: [{ id: 't1', tree: { pane: { id: 'p1', title: 'x' } } }, { id: 't2', tree: { pane: { id: 'p2', title: 'y' } } }] },
      { id: 'b', tabs: [{ id: 't3', tree: { pane: { id: 'p3', title: 'z' } } }] },
    ],
    settings: { inspections: { severities: ['error', 'warning'] } },
  })

  // Unchanged: the whole thing comes back as `prev`.
  {
    const prev = make()
    const next = make()
    ok(shareEqual(prev, next) === prev, 'an equal payload is the previous object')
  }

  // One leaf changed: new objects exactly along its path, `prev`'s everywhere else.
  {
    const prev = make()
    const next = make()
    next.rev = 2
    next.projects[0].tabs[1].tree.pane.title = 'changed'
    const got = shareEqual(prev, next)
    ok(deepEq(got, next), 'the result is deep-equal to next')
    ok(got !== prev, 'the root is new when anything changed')
    ok(got.projects !== prev.projects, 'the array on the changed path is new')
    ok(got.projects[0] !== prev.projects[0], 'the project on the changed path is new')
    ok(got.projects[0].tabs[1] !== prev.projects[0].tabs[1], 'the tab on the changed path is new')
    ok(got.projects[0].tabs[1].tree.pane !== prev.projects[0].tabs[1].tree.pane, 'the changed pane is new')
    ok(got.projects[0].tabs[0] === prev.projects[0].tabs[0], 'a sibling tab keeps its identity')
    ok(got.projects[1] === prev.projects[1], 'another project keeps its identity')
    ok(got.settings === prev.settings, 'untouched settings keep their identity')
  }

  // Arrays of different length, both ways.
  {
    const prev = { list: [{ a: 1 }, { a: 2 }] }
    const grown = { list: [{ a: 1 }, { a: 2 }, { a: 3 }] }
    const g = shareEqual(prev, grown)
    ok(g.list !== prev.list && g.list.length === 3, 'a grown array is new')
    ok(g.list[0] === prev.list[0] && g.list[1] === prev.list[1], 'its shared prefix keeps identity')
    const shrunk = { list: [{ a: 1 }] }
    const s = shareEqual(prev, shrunk)
    ok(s.list !== prev.list && s.list.length === 1, 'a shrunk array is new')
    ok(s.list[0] === prev.list[0], 'its surviving item keeps identity')
  }

  // Keys added or removed make the object new even when every shared key is equal.
  {
    const prev = { a: 1, b: { c: 2 } }
    const added = { a: 1, b: { c: 2 }, d: 3 }
    const r = shareEqual(prev, added)
    ok(r !== prev && r.d === 3 && r.b === prev.b, 'an added key makes the object new, siblings shared')
    const removed = { a: 1 }
    ok(shareEqual(prev, removed) !== prev, 'a removed key makes the object new')
    // `undefined` vs absent is a difference, not an equality.
    ok(shareEqual({ a: 1 }, { a: 1, b: undefined }) !== undefined, 'an explicit undefined key is kept')
    ok('b' in shareEqual({ a: 1 }, { a: 1, b: undefined }), 'an explicit undefined key is present in the result')
  }

  // Type changes and non-plain values.
  {
    ok(shareEqual({ a: [1] }, { a: { 0: 1 } }).a.constructor === Object, 'an array replaced by an object is the object')
    const m1 = new Map([[1, 2]])
    const m2 = new Map([[1, 2]])
    ok(shareEqual({ m: m1 }, { m: m2 }).m === m2, 'a non-plain value is taken from next, never shared by content')
    ok(shareEqual(null, { a: 1 }).a === 1, 'null prev yields next')
    ok(shareEqual({ a: 1 }, null) === null, 'null next yields null')
    ok(Number.isNaN(shareEqual(NaN, NaN)), 'NaN is equal to itself here, as Object.is says')
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`share: ${failed} failure(s)`)
  process.exit(1)
}
console.log('share: ok')
