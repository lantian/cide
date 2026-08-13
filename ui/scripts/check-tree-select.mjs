/**
 * Checks `src/sidebar/treeSelection.ts` — what Ctrl and Shift mean in the file tree.
 *
 * > *"file tree - can not select multiple elements with CTRL + mouse and SHIFT + mouse
 * > (like we've implemented for git tree)"*
 *
 * The same shape as `check-git-tree.mjs`'s selection section, and pinning the same table for the
 * other tree. Every rule here is invisible in a screenshot and each has a wrong version that
 * type-checks: a Shift-click that moves the anchor (so the second one extends from the wrong
 * end), a Ctrl-click that clears what came before, a right-click that destroys the selection
 * its own menu is about to act on, a scope that falls back to the cursor and deletes a row
 * nothing on screen marked.
 *
 * The module is deliberately import-free so `tsc` can emit a `.js` with no imports at all and
 * node can load it directly. If someone adds a value import, the `import()` below throws
 * `ERR_MODULE_NOT_FOUND` rather than quietly pulling half the sidebar into a check that is
 * supposed to be running rules.
 *
 * The tail reads `FileTree.tsx` and `treeStore.ts` as *source*, because the wiring cannot be
 * executed here: there is no DOM and no Tauri. Ugly, and it is the difference between a claim
 * that can fail and one that cannot — every rule below could pass with the modifiers never
 * reaching the store at all.
 *
 * Run: `pnpm --dir ui run check:tree-select`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-tree-select-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/treeSelection.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The app's own tsconfig sets both, and the second is what makes `band[0]` a
      // `string | undefined` the module has to handle rather than a `string` it may assume.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const s = await import(`file://${join(out, 'sidebar', 'treeSelection.js')}`)

  /** A selection, spelled the way the assertions want to read it. */
  const sel = (paths, anchor = null) => ({ paths: new Set(paths), anchor })
  /** Both plan shapes flattened into something `JSON.stringify` can compare. */
  const planOf = (plan) =>
    plan.kind === 'range'
      ? ['range', plan.from, plan.to]
      : ['set', [...plan.next.paths], plan.next.anchor]

  const A = '/w/a.rs'
  const B = '/w/b.rs'
  const C = '/w/c.rs'
  const D = '/w/dir'

  const PLAIN = { ctrl: false, shift: false }
  const CTRL = { ctrl: true, shift: false }
  const SHIFT = { ctrl: false, shift: true }
  const BOTH = { ctrl: true, shift: true }

  // ---------------------------------------------------------------- the empty state

  eq([...s.NO_PATHS.paths], [], 'NO_PATHS selects nothing')
  eq(s.NO_PATHS.anchor, null, 'NO_PATHS has no anchor')
  eq(s.NO_MODS, { ctrl: false, shift: false }, 'NO_MODS is neither modifier')

  // ---------------------------------------------------------------- a plain press

  eq(
    planOf(s.pressSelect(s.NO_PATHS, A, PLAIN)),
    ['set', [A], A],
    'a plain press selects one row and puts the anchor on it',
  )
  eq(
    planOf(s.pressSelect(sel([A, B, C], A), B, PLAIN)),
    ['set', [B], B],
    'a plain press collapses a multi-row selection, even onto a row already in it',
  )

  // ---------------------------------------------------------------- ctrl

  eq(
    planOf(s.pressSelect(sel([A], A), B, CTRL)),
    ['set', [A, B], B],
    'ctrl adds a row and moves the anchor to it',
  )
  eq(
    planOf(s.pressSelect(sel([A, B], A), A, CTRL)),
    ['set', [B], A],
    'ctrl on a selected row removes it — and still moves the anchor there',
  )
  eq(
    planOf(s.pressSelect(sel([A], A), A, CTRL)),
    ['set', [], A],
    'ctrl can empty the selection while leaving the cursor where it was',
  )

  // ---------------------------------------------------------------- shift

  eq(
    planOf(s.pressSelect(sel([A], A), C, SHIFT)),
    ['range', A, C],
    'shift asks for the band from the anchor; only the store can resolve it',
  )
  eq(
    planOf(s.pressSelect(s.NO_PATHS, C, SHIFT)),
    ['set', [C], C],
    'shift with no anchor yet is a plain press',
  )
  eq(
    planOf(s.pressSelect(sel([A], A), C, BOTH)),
    ['range', A, C],
    'ctrl+shift is shift — the same order rowSelection.ts uses',
  )

  // ---------------------------------------------------------------- the arrows

  eq(
    planOf(s.keySelect(sel([A], A), C, SHIFT)),
    planOf(s.pressSelect(sel([A], A), C, SHIFT)),
    'shift+arrow and shift+click ask for the same band',
  )
  const held = sel([A, B], A)
  ok(
    s.keySelect(held, C, CTRL).kind === 'set' && s.keySelect(held, C, CTRL).next === held,
    'ctrl+arrow leaves the selection untouched, by identity — the cursor moves alone',
  )
  eq(
    planOf(s.keySelect(sel([A, B], A), C, PLAIN)),
    ['set', [C], C],
    'a bare arrow takes the selection with it',
  )

  // ---------------------------------------------------------------- resolving a band

  eq(
    [...s.applyRange(sel([A], A), [A, B, C], C).paths],
    [A, B, C],
    'a resolved band is the selection',
  )
  eq(
    s.applyRange(sel([A], A), [A, B, C], C).anchor,
    A,
    'the anchor stays put, so a second shift-click re-extends from the same end',
  )
  eq(
    planOf({ kind: 'set', next: s.applyRange(sel([A, B], A), null, C) }),
    ['set', [C], C],
    'an unresolvable band falls back to the plain rule, anchor and all',
  )
  eq(
    planOf({ kind: 'set', next: s.applyRange(sel([A, B], A), [], C) }),
    ['set', [C], C],
    'an empty band is the same fallback — a tree that shrank under the fetch answers short',
  )
  eq(
    s.applyRange(s.NO_PATHS, [A, B], B).anchor,
    B,
    'a band resolved with no anchor takes one rather than leaving null behind',
  )

  // ---------------------------------------------------------------- the cap

  eq(s.rangeRefusal(1), null, 'one row is not too many')
  eq(s.rangeRefusal(s.RANGE_ROWS), null, 'the cap itself is allowed')
  ok(
    typeof s.rangeRefusal(s.RANGE_ROWS + 1) === 'string',
    'one row past the cap is refused with a sentence',
  )
  ok(
    s.rangeRefusal(50_000)?.includes('50000') && s.rangeRefusal(50_000)?.includes('10000'),
    'the refusal names both the range asked for and the limit',
  )

  // ---------------------------------------------------------------- escape, menu, scope

  eq([...s.collapseTo(B).paths], [B], 'escape collapses to the cursor row')
  eq(s.collapseTo(B).anchor, B, 'escape re-anchors where it collapsed')
  eq([...s.collapseTo(null).paths], [], 'escape with no cursor clears outright')

  const many = sel([A, B, D], A)
  ok(
    s.pressMenu(many, B) === many,
    'a right-click inside the selection keeps it, by identity — the menu acts on all of it',
  )
  eq(
    planOf({ kind: 'set', next: s.pressMenu(many, C) }),
    ['set', [C], C],
    'a right-click outside the selection collapses to the row under the pointer',
  )

  eq(s.actionScope(many), [A, B, D], 'the scope is every selected row, in pick order')
  eq(s.actionScope(s.NO_PATHS), [], 'nothing selected is an empty scope, never the cursor')
  eq(
    s.actionScope(sel([B], A)),
    [B],
    'the scope follows the selection, not the anchor — the two can differ by one ctrl-click',
  )

  // ---------------------------------------------------------------- the wiring, as source

  const tree = readFileSync('src/sidebar/FileTree.tsx', 'utf8')
  const store = readFileSync('src/sidebar/treeStore.ts', 'utf8')

  // Every rule above is dead if the row never reports which modifiers were held.
  ok(
    /ctrl: e\.ctrlKey \|\| e\.metaKey, shift: e\.shiftKey/.test(tree),
    'the row must fold ctrlKey/metaKey/shiftKey into the mods it hands to onAct',
  )
  // The band is drawn from the *set*; the leading rule from the cursor. Reverting either to the
  // single `row.path === selected` leaves a multi-selection with one row highlighted.
  ok(
    /selected=\{selection\.paths\.has\(row\.path\)\}/.test(tree),
    'a row is drawn as selected when it is in the selection set',
  )
  ok(/current=\{row\.path === selected\}/.test(tree), 'the cursor row is drawn from `selected`')
  ok(
    /aria-multiselectable/.test(tree),
    'a tree whose rows can all be aria-selected has to say it is multi-selectable',
  )
  // The gestures that act on files must read the selection, not the row under the pointer.
  ok(
    /takeClip\(key === 'x' \? 'cut' : 'copy', actionScope\(store\.selection\)\)/.test(tree),
    'Ctrl+X / Ctrl+C must act on the whole selection',
  )
  ok(
    /const marked = pressMenu\(store\.selection, row\.path\)/.test(tree),
    'the context menu must go through pressMenu, or right-clicking destroys what it acts on',
  )
  // `select` is the plain rule and has to collapse: without this a click on the row the cursor
  // is already on leaves a five-row selection standing.
  ok(
    /selection: collapseTo\(path\)/.test(store),
    'treeStore.select must collapse the selection to the row it selects',
  )
  ok(
    /selection: NO_PATHS/.test(store),
    'attaching to a project must drop the previous project’s selection',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-tree-select: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-tree-select: ok')
