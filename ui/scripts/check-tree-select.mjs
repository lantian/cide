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
  /**
   * A press answers `{plan, deferred}` since the drag landed; the arrows still answer a bare plan.
   * Spelled out here so every assertion below reads as the rule it is pinning rather than as a
   * field access.
   */
  const pressOf = (press) => planOf(press.plan)

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
    pressOf(s.pressSelect(s.NO_PATHS, A, PLAIN)),
    ['set', [A], A],
    'a plain press selects one row and puts the anchor on it',
  )
  eq(
    pressOf(s.pressSelect(sel([A, B, C], A), D, PLAIN)),
    ['set', [D], D],
    'a plain press on a row OUTSIDE the selection collapses to it at once',
  )
  eq(
    pressOf(s.pressSelect(sel([A], A), A, PLAIN)),
    ['set', [A], A],
    'a plain press on the ONLY selected row is the same thing again, not a deferral',
  )

  // ------------------------------------------------- the deferred press, and its release
  /*
   * This is the rule that makes dragging a multi-row selection possible at all, and it replaced
   * an assertion that said the opposite ("a plain press collapses a multi-row selection, even
   * onto a row already in it"). That was not a weaker claim, it was the *wrong* one: the press
   * that collapses is also the press that begins a drag of the whole set, so collapsing on
   * mousedown destroyed the selection before the pointer had moved a pixel and "drag the four
   * files I selected" could only ever have dragged one. The collapse now waits for the release.
   */
  const many3 = sel([A, B, C], A)
  ok(
    s.pressSelect(many3, B, PLAIN).deferred,
    'a plain press on a row already in a MULTI-row selection defers its collapse to the release',
  )
  ok(
    s.pressSelect(many3, B, PLAIN).plan.kind === 'set'
      && s.pressSelect(many3, B, PLAIN).plan.next === many3,
    'and it leaves the selection exactly as it was, by identity — nothing to re-render, nothing '
      + 'for the drag to lose',
  )
  ok(
    !s.pressSelect(sel([A], A), A, PLAIN).deferred,
    'the only selected row is not deferred: there is no set to preserve and the anchor would go '
      + 'stale',
  )
  ok(
    !s.pressSelect(many3, D, PLAIN).deferred,
    'a press outside the selection is not deferred either — it has already collapsed',
  )
  eq(
    planOf({ kind: 'set', next: s.releaseSelect(B) }),
    ['set', [B], B],
    'the release finishes what the deferred press did not: one row, anchored on it',
  )

  // ---------------------------------------------------------------- ctrl

  eq(
    pressOf(s.pressSelect(sel([A], A), B, CTRL)),
    ['set', [A, B], B],
    'ctrl adds a row and moves the anchor to it',
  )
  eq(
    pressOf(s.pressSelect(sel([A, B], A), A, CTRL)),
    ['set', [B], A],
    'ctrl on a selected row removes it — and still moves the anchor there',
  )
  eq(
    pressOf(s.pressSelect(sel([A], A), A, CTRL)),
    ['set', [], A],
    'ctrl can empty the selection while leaving the cursor where it was',
  )
  ok(
    !s.pressSelect(sel([A, B, C], A), B, CTRL).deferred
      && !s.pressSelect(sel([A, B, C], A), B, SHIFT).deferred,
    'a MODIFIED press is never deferred: it is a selection gesture and it never starts a drag',
  )

  // ---------------------------------------------------------------- shift

  eq(
    pressOf(s.pressSelect(sel([A], A), C, SHIFT)),
    ['range', A, C],
    'shift asks for the band from the anchor; only the store can resolve it',
  )
  eq(
    pressOf(s.pressSelect(s.NO_PATHS, C, SHIFT)),
    ['set', [C], C],
    'shift with no anchor yet is a plain press',
  )
  eq(
    pressOf(s.pressSelect(sel([A], A), C, BOTH)),
    ['range', A, C],
    'ctrl+shift is shift — the same order rowSelection.ts uses',
  )

  // ---------------------------------------------------------------- the arrows

  eq(
    planOf(s.keySelect(sel([A], A), C, SHIFT)),
    pressOf(s.pressSelect(sel([A], A), C, SHIFT)),
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
  /*
   * The gestures that act on files must read the selection, not the row under the pointer.
   *
   * Two halves now, because M13 put a refusal between them: the scope is computed, checked
   * against `mutationRefusal`, and only then handed to `takeClip`. Both halves are pinned, and
   * both are pinned **inside the clipboard branch** rather than anywhere in the file — the
   * Delete branch a few lines below computes an identically-spelled `scope`, so a whole-file
   * regex passes while the clipboard's own scope is narrowed to the cursor row. That is not
   * hypothetical: it is exactly what the first version of this assertion missed.
   */
  const clipBranch = /&& clipboardKey\) \{[\s\S]*?\n      \}/.exec(tree)?.[0] ?? ''
  ok(clipBranch !== '', 'the Ctrl+X / Ctrl+C branch is still there to check')
  ok(
    /const scope = actionScope\(store\.selection\)/.test(clipBranch),
    'Ctrl+X / Ctrl+C must take their scope from the whole selection',
  )
  ok(
    /takeClip\(key === 'x' \? 'cut' : 'copy', scope\)/.test(clipBranch),
    'Ctrl+X / Ctrl+C must act on that scope, not on the row under the pointer',
  )
  ok(
    /const marked = pressMenu\(store\.selection, row\.path\)/.test(tree),
    'the context menu must go through pressMenu, or right-clicking destroys what it acts on',
  )
  // `select` is the plain rule and has to collapse: it is what a reveal, a paste landing and a
  // freshly created file use, and none of those has a release to wait for.
  ok(
    /selection: collapseTo\(path\)/.test(store),
    'treeStore.select must collapse the selection to the row it selects',
  )
  /*
   * The deferral is a rule in a pure module and a *wire* through two files, and the wire is the
   * half that has been missing four times in this project. Every mouse press has to reach
   * `pressRow` — the plain one too, which is the press that can be deferred — and the release has
   * to be able to finish it.
   */
  ok(
    /const press = store\.pressRow\(row\.path, index, mods\)/.test(tree)
      && /deferredPress\.current = press\.deferred \? row\.path : null/.test(tree),
    'every selecting press goes through `pressRow` and remembers a deferral; the plain press '
      + 'calling `select` directly is what made a multi-row drag impossible',
  )
  ok(
    /releaseRow\(path, index\)/.test(store) && /getState\(\)\.releaseRow\(path, index\)/.test(tree),
    '`treeStore.releaseRow` exists and the panel calls it — a deferral nothing resolves leaves '
      + 'five rows selected after a click that meant one',
  )
  ok(
    /if \(drag\.dragged\(\)\) \{[\s\S]{0,200}?deferredPress\.current = null[\s\S]{0,40}?return/.test(
      tree,
    ),
    'the release is SKIPPED when the gesture became a drag: the whole selection has just been '
      + 'moved and collapsing to one row would undo the widening',
  )
  ok(
    /onMouseUp=\{\(e\) => \{[\s\S]{0,240}?onRelease\(row\.path, index\)/.test(tree),
    'and the row actually carries the mouseup that runs it',
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
