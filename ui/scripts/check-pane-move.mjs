/**
 * The pane grab handle's rules, compiled alone and driven under node.
 *
 * `src/layout/paneMove.ts` decides where a dragged pane lands; `usePaneDrag.ts` decides
 * nothing and only listens. That split exists so this script can exist: a pointer gesture
 * cannot be replayed here, but every question it asks can be.
 *
 * Compiling the module **by itself** is half the assertion. It imports nothing — its
 * `MoveNode` is a structural mirror of the generated `LayoutNode` rather than an import of it —
 * and the day someone reaches for `@/ipc/client` in it, `tsc` below fails on the unresolved
 * path rather than the check silently starting to test a different thing.
 *
 * What is checked here and nowhere else:
 *
 *  * the four edge bands are the four *quadrants*, resolved by normalised distance, so a wide
 *    short pane does not report `top`/`bottom` across almost all of itself;
 *  * `left/right → row`, `top/bottom → col`, which is what makes a drop land where a split
 *    there would have put it;
 *  * **the no-op drop**, which is the whole reason `dropOutcome` takes the tree. Dropping a
 *    tile on the edge it already borders must answer `null`: performing it would lift the pane,
 *    spread its width over the row, re-insert it at `1/(n+1)` and repaint every window, to put
 *    it back where it started;
 *  * `moveIntent` is a row on all four directions, including up and down — the decision the
 *    keyboard rests on, pinned here so a "tidy" symmetry edit has to argue with a test.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-pane-move-'))
let failures = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a === b) return
  failures += 1
  console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (cond) return
  failures += 1
  console.error(`FAIL ${what}`)
}

const strip = (src) =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/layout/paneMove.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // Import-free, so plain ESM output loads under node with no interop shim — and the
      // compile failing is itself the assertion that it stayed that way.
      '--module', 'esnext',
      '--moduleResolution', 'bundler',
      '--target', 'es2022',
      '--strict',
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
      '--lib', 'es2023',
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'layout', 'paneMove.js')}`)
  const { THRESHOLD, dropOutcome, dropTarget, edgeAt, intentFor, movable, moveIntent } = m

  // =====================================================================================
  // edgeAt — the four quadrants
  // =====================================================================================
  const box = { left: 0, top: 0, width: 100, height: 100 }
  eq(edgeAt(box, 5, 50), 'left', 'the left band')
  eq(edgeAt(box, 95, 50), 'right', 'the right band')
  eq(edgeAt(box, 50, 5), 'top', 'the top band')
  eq(edgeAt(box, 50, 95), 'bottom', 'the bottom band')
  eq(edgeAt(box, -1, 50), null, 'outside to the left is no target')
  eq(edgeAt(box, 101, 50), null, 'outside to the right is no target')
  eq(edgeAt(box, 50, -1), null, 'above the box is no target')
  eq(edgeAt(box, 50, 101), null, 'below the box is no target')
  ok(edgeAt(box, 50, 50) !== null, 'the exact centre still answers — there is no dead zone')

  // A pane offset from the origin: the bands are relative to the box, not to the viewport.
  const off = { left: 200, top: 100, width: 100, height: 100 }
  eq(edgeAt(off, 205, 150), 'left', 'an offset box still has its own left band')
  eq(edgeAt(off, 295, 150), 'right', 'and its own right band')

  /*
   * The reason the distances are normalised, stated as a test.
   *
   * On a 900x100 pane the point (450, 30) is 30px from the top and 450px from either side — so
   * a raw-pixel comparison calls it `top`. As a *fraction* it is 0.30 of the height from the
   * top and 0.5 of the width from each side, so it is the top band, correctly. Now take
   * (100, 45): 100px from the left, 45px from the top. Raw pixels say `top` again; normalised
   * says 0.111 of the width versus 0.45 of the height — the left band, which is what the user
   * aiming at the left quarter of a wide pane meant.
   */
  const wide = { left: 0, top: 0, width: 900, height: 100 }
  eq(edgeAt(wide, 100, 45), 'left', 'a wide pane still has a usable left band')
  eq(edgeAt(wide, 800, 55), 'right', 'and a usable right band')
  eq(edgeAt(wide, 450, 10), 'top', 'its top band is the middle-top, not the whole pane')

  // =====================================================================================
  // intentFor / moveIntent
  // =====================================================================================
  eq(intentFor('left'), { axis: 'row', side: 'before' }, 'left is a tile before the target')
  eq(intentFor('right'), { axis: 'row', side: 'after' }, 'right is a tile after it')
  eq(intentFor('top'), { axis: 'col', side: 'before' }, 'top is a row above it')
  eq(intentFor('bottom'), { axis: 'col', side: 'after' }, 'bottom is a row below it')

  /*
   * The keyboard's rule, and the one most likely to be "fixed" by someone making it symmetric
   * with `intentFor`. All four are rows: `pane.move.up` joins the row above rather than minting
   * a new one, because minting would promote the neighbour's own row-mates to full-width rows
   * as a side effect of a keystroke aimed at this pane — and because joining is the only
   * reading that can ever *reduce* the row count.
   */
  eq(moveIntent('left'), { axis: 'row', side: 'before' }, 'move left is a row move')
  eq(moveIntent('right'), { axis: 'row', side: 'after' }, 'move right is a row move')
  eq(moveIntent('up'), { axis: 'row', side: 'before' }, 'move UP is a row move, deliberately')
  eq(moveIntent('down'), { axis: 'row', side: 'after' }, 'move DOWN is a row move, deliberately')

  eq(movable(1), false, "a tab's only pane has nowhere to go")
  eq(movable(2), true, 'two panes can be rearranged')
  eq(THRESHOLD, 4, 'the same 4px threshold the splitters and the tab strip use')

  // =====================================================================================
  // dropTarget
  // =====================================================================================
  const boxes = [
    { pane: 'a', left: 0, top: 0, width: 100, height: 100 },
    { pane: 'b', left: 100, top: 0, width: 100, height: 100 },
  ]
  eq(dropTarget(boxes, 150, 50, 'a'), { target: 'b', edge: 'left' }, 'aims at the other pane')
  eq(dropTarget(boxes, 50, 50, 'a'), null, 'and never at the pane being dragged')
  eq(dropTarget(boxes, 900, 50, 'a'), null, 'a pointer outside every pane targets nothing')

  // =====================================================================================
  // dropOutcome — the no-op rules, which need the tree
  // =====================================================================================
  const leaf = (pane) => ({ kind: 'leaf', pane })
  const split = (axis, a, b) => ({ kind: 'split', axis, a, b })

  // One row of three tiles: a | b | c
  const row3 = split('row', leaf('a'), split('row', leaf('b'), leaf('c')))

  eq(dropOutcome(row3, 'a', 'a', 'right'), null, 'a pane dropped on itself is nothing')
  eq(dropOutcome(row3, 'a', 'zz', 'right'), null, 'an unknown target is nothing')
  eq(dropOutcome(row3, 'zz', 'a', 'right'), null, 'an unknown pane is nothing')

  /*
   * The adjacency no-op, in both directions. `a` is immediately left of `b`, so dropping `a` on
   * `b`'s LEFT edge would put it back exactly where it is — and dropping `b` on `a`'s RIGHT
   * edge is the same statement read the other way.
   */
  eq(dropOutcome(row3, 'a', 'b', 'left'), null, 'dropping a tile on the edge it already borders')
  eq(dropOutcome(row3, 'b', 'a', 'right'), null, 'and the same boundary named from the other side')

  // But the far side of the same neighbour is a real move.
  eq(
    dropOutcome(row3, 'a', 'b', 'right'),
    { pane: 'a', target: 'b', axis: 'row', side: 'after' },
    'past the neighbour is a real reorder',
  )
  eq(
    dropOutcome(row3, 'c', 'a', 'left'),
    { pane: 'c', target: 'a', axis: 'row', side: 'before' },
    'and so is jumping to the head of the row',
  )

  /*
   * A vertical drop is never the adjacency no-op of a horizontal one: `a` sits left of `b` in a
   * row, and dropping it on `b`'s top edge asks for a *column*, which is a different chain and
   * a real change.
   */
  eq(
    dropOutcome(row3, 'a', 'b', 'top'),
    { pane: 'a', target: 'b', axis: 'col', side: 'before' },
    'the same pair on the other axis is a real move',
  )

  // Two rows: [a|b] over [c]. Moving c up beside a is a real move; the chains differ.
  const twoRows = split('col', split('row', leaf('a'), leaf('b')), leaf('c'))
  eq(
    dropOutcome(twoRows, 'c', 'a', 'right'),
    { pane: 'c', target: 'a', axis: 'row', side: 'after' },
    'a pane from another row joins this one',
  )
  eq(
    dropOutcome(twoRows, 'a', 'c', 'bottom'),
    { pane: 'a', target: 'c', axis: 'col', side: 'after' },
    'and a pane can be dropped below the bottom row',
  )

  // =====================================================================================
  // The mirror must still describe the real DTO
  // =====================================================================================
  const generated = readFileSync('src/ipc/generated.ts', 'utf8')
  const layoutNode = /export type LayoutNode =([\s\S]*?);\n/.exec(generated)?.[1] ?? ''
  ok(layoutNode !== '', 'the generated bindings still declare a LayoutNode')
  ok(
    /"kind":\s*"leaf",\s*pane:\s*PaneId/.test(layoutNode),
    'a leaf is still tagged `leaf` and still names its pane `pane`',
  )
  ok(
    /"kind":\s*"split"/.test(layoutNode) &&
      /\baxis:\s*Axis\b/.test(layoutNode) &&
      /\ba:\s*LayoutNode\b/.test(layoutNode) &&
      /\bb:\s*LayoutNode\b/.test(layoutNode),
    'and a split is still tagged `split` with `axis`, `a` and `b` — the four names `MoveNode` '
      + 'mirrors by hand so this module can compile with no imports. A rename in Rust reaches '
      + 'TypeScript through codegen and would otherwise pass silently here',
  )

  // =====================================================================================
  // The hook must stay a hook: no HTML5 drag-and-drop, and a scoped hit test
  // =====================================================================================
  const hook = strip(readFileSync('src/layout/usePaneDrag.ts', 'utf8'))
  for (const banned of ['dataTransfer', 'draggable', 'dragstart', 'ondragover']) {
    ok(
      !hook.includes(banned),
      `the pane drag uses pointer events, never HTML5 DnD (found \`${banned}\`) — `
        + "Tauri's native drag-drop handler is on, so those events never reach the page",
    )
  }
  ok(hook.includes('setPointerCapture'), 'the gesture captures the pointer')
  ok(
    hook.includes("data-audit=\"paneTree\""),
    'the hit test is rooted at the active tab\'s pane tree. Unscoped, it would collect the '
      + 'panes of every open tab: `TabContent` lays them all out at full size and hides them '
      + 'with `visibility: hidden`, so their rects are real and overlapping, and a drop would '
      + 'land in a tab the user cannot see',
  )
  ok(
    hook.includes('lockBodyForDrag') && hook.includes('unlockBodyAfterDrag'),
    'the body is locked for the drag and unlocked after it',
  )
  ok(hook.includes("'Escape'"), 'Escape abandons the drag')
  ok(
    /removeEventListener\('pointermove'/.test(hook) && /removeEventListener\('keydown'/.test(hook),
    'and every exit path detaches its listeners — an Escaped drag never sees a pointerup, so '
      + 'a teardown hung off that event alone would leak the whole state machine',
  )

  // =====================================================================================
  // The store's snapshot must stay a primitive
  // =====================================================================================
  const store = strip(readFileSync('src/layout/paneDropZone.ts', 'utf8'))
  ok(
    !/export function paneDropMark[^}]*return \{/s.test(store),
    'paneDropMark returns a string, never an object: `useSyncExternalStore` compares with '
      + '`Object.is`, so a freshly built object re-renders for ever and unmounts the root',
  )
  ok(
    store.includes('paneDropMarkServer'),
    'and it ships a server snapshot, because the render checks SSR the pane views under node',
  )

  // The frame must pass `children` straight through, or a drop-zone re-render reaches the
  // terminal — the claim the whole store design rests on.
  const frame = strip(readFileSync('src/layout/PaneTitleBar.tsx', 'utf8'))
  ok(
    frame.includes('{children}'),
    'PaneFrame renders `children` untouched, so a mark change never rebuilds the pane body',
  )
  ok(
    frame.includes("data-drop="),
    'and the mark reaches the DOM as an attribute the stylesheet reads',
  )

  if (failures > 0) {
    console.error(`\n${failures} failure(s)`)
    process.exit(1)
  }
  console.log('pane move: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
