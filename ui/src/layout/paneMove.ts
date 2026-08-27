/**
 * Where a dragged pane would land, decided as arithmetic over plain data.
 *
 * The rules half of the pane grab handle, split out from the pointer state machine in
 * `usePaneDrag.ts` for the reason `chrome/tabDrag.ts` and `sidebar/treeDrag.ts` are: a pointer
 * gesture cannot be run under node, so everything that can be *decided* is put where a check
 * script can drive it, and what is left in the hook is listeners and refs.
 *
 * **This module imports nothing.** `MoveNode` is a structural mirror of the generated
 * `LayoutNode` rather than an import of it, the same trick `tabDrag.DragTab` uses — the
 * generated module reaches `@tauri-apps/api`, and one value import here would end the standalone
 * compile that `check:pane-move` is built on. The mirror is asserted against the real DTO by
 * that check, so a rename in Rust cannot drift past it silently.
 *
 * # The two decisions that live here
 *
 * **Which edge of a pane the pointer is over** is resolved by *normalised* distance, so a wide
 * short pane does not report `top`/`bottom` across almost its whole area. Inside a pane it
 * always answers something: the four quadrants meet at the centre and there is no dead zone,
 * for the reason `tabDrag.caretIndex` always answers an index — a highlight that blinks out
 * over the middle of a pane is indistinguishable from a drag that has stopped tracking.
 *
 * **Whether the drop would change anything**, which is the off-by-one this module exists for
 * and the reason `dropOutcome` needs the tree rather than two ids. Dropping a tile on the left
 * edge of the tile it already sits left of is a no-op — but the domain would happily perform
 * it, and the composition is not free: `move_pane` lifts the pane (spreading its width over the
 * row) and re-inserts it at `1 / (n + 1)`, so the whole row visibly re-flows, `rev` bumps, and
 * every window repaints, to put the pane back where it started.
 */

/** The shape of a pane rectangle this module reasons about. Screen coordinates, any origin. */
export interface Box {
  readonly left: number
  readonly top: number
  readonly width: number
  readonly height: number
}

/** A pane's box, tagged with which pane it is. */
export interface PaneBox extends Box {
  readonly pane: string
}

/**
 * A structural mirror of `LayoutNode`, so this module can be compiled with no imports.
 *
 * `check:pane-move` asserts the tags and the field names against `cide_ipc`'s own generated
 * TypeScript, because a mirror that drifts is worse than an import: it compiles.
 */
export type MoveNode =
  | { readonly kind: 'leaf'; readonly pane: string }
  | {
      readonly kind: 'split'
      readonly axis: 'row' | 'col'
      readonly a: MoveNode
      readonly b: MoveNode
    }

/** Which band of a pane the pointer is in. */
export type DropEdge = 'left' | 'right' | 'top' | 'bottom'

/** What a release commits — exactly the arguments `pane_move` takes. */
export interface MoveOutcome {
  readonly pane: string
  readonly target: string
  readonly axis: 'row' | 'col'
  readonly side: 'before' | 'after'
}

/** Enough movement to mean "drag" rather than "click". The same 4px the splitters use. */
export const THRESHOLD = 4

/**
 * Which edge of `box` the point belongs to, or `null` when the point is outside it.
 *
 * Normalised distance, not raw pixels: on a 900x200 pane a raw comparison calls almost every
 * point `top` or `bottom`, because the vertical edges are simply nearer in absolute terms.
 * Dividing each distance by the span it is measured along makes the four bands the four
 * quadrants of the box, which is what the highlight draws and what the user is aiming at.
 *
 * Ties resolve left, right, top, bottom — the exact centre of a pane has to answer something,
 * and which of the four it answers is arbitrary and stable.
 */
export function edgeAt(box: Box, x: number, y: number): DropEdge | null {
  if (box.width <= 0 || box.height <= 0) return null
  if (x < box.left || x > box.left + box.width) return null
  if (y < box.top || y > box.top + box.height) return null

  const left = (x - box.left) / box.width
  const right = (box.left + box.width - x) / box.width
  const top = (y - box.top) / box.height
  const bottom = (box.top + box.height - y) / box.height

  const nearest = Math.min(left, right, top, bottom)
  if (left === nearest) return 'left'
  if (right === nearest) return 'right'
  if (top === nearest) return 'top'
  return 'bottom'
}

/**
 * The pane under the pointer and the band it is in, skipping the pane being dragged.
 *
 * First match wins. The boxes come from one tab's pane tree and do not overlap, so there is
 * never a second — see `usePaneDrag`, which scopes the scan to the active tab's tree for the
 * reason stated there.
 */
export function dropTarget(
  boxes: readonly PaneBox[],
  x: number,
  y: number,
  dragged: string,
): { readonly target: string; readonly edge: DropEdge } | null {
  for (const box of boxes) {
    if (box.pane === dragged) continue
    const edge = edgeAt(box, x, y)
    if (edge !== null) return { target: box.pane, edge }
  }
  return null
}

/**
 * The `(axis, side)` an edge means, in the vocabulary `pane_move` and `pane_split` share.
 *
 * Left and right are positions in a *row*; top and bottom mint or join a column. This is the
 * mapping that makes a drop land where splitting there would have put it, which is what stops
 * the drag and the `+` button building different trees from the same picture.
 */
export function intentFor(edge: DropEdge): { axis: 'row' | 'col'; side: 'before' | 'after' } {
  switch (edge) {
    case 'left':
      return { axis: 'row', side: 'before' }
    case 'right':
      return { axis: 'row', side: 'after' }
    case 'top':
      return { axis: 'col', side: 'before' }
    case 'bottom':
      return { axis: 'col', side: 'after' }
  }
}

/**
 * Where `pane.move.<dir>` puts the pane, relative to the neighbour it moves past.
 *
 * **Always a row**, including for up and down, and that is the interesting decision. The other
 * reading — up/down mints a full-width row — is wrong on the case that matters: in a 2x2, moving
 * the bottom-left pane up under that reading gives *three* rows and promotes its former
 * row-mate to a full-width row of its own, as a side effect of a keystroke aimed at another
 * pane. Joining the neighbour's row instead lands the pane at the horizontal position it
 * already had, and it is the only reading that can *reduce* the row count — which is what makes
 * "move it out of this row" a gesture at all. A new full-width row is still reachable, by
 * dragging onto a pane's top or bottom band and by `pane.split.down`.
 *
 * `side` is a position in a horizontal chain, so a vertical direction of travel has no opinion
 * about it. `navigate` picked the target *because* the moving pane's horizontal midpoint falls
 * inside it, so the pane belongs immediately beside it; `after` is reading order, and it is what
 * every split gesture in this app already does. The two identical arms are deliberate.
 */
export function moveIntent(
  direction: 'left' | 'right' | 'up' | 'down',
): { axis: 'row' | 'col'; side: 'before' | 'after' } {
  switch (direction) {
    case 'left':
      return { axis: 'row', side: 'before' }
    case 'right':
      return { axis: 'row', side: 'after' }
    case 'up':
      return { axis: 'row', side: 'before' }
    case 'down':
      return { axis: 'row', side: 'after' }
  }
}

/** Whether a pane may be picked up at all: a tab's only pane has nowhere to go. */
export function movable(paneCount: number): boolean {
  return paneCount > 1
}

/** Every node from the root down to the leaf holding `pane`, or `null` when it holds none. */
function trailTo(node: MoveNode, pane: string, out: MoveNode[]): boolean {
  out.push(node)
  if (node.kind === 'leaf') {
    if (node.pane === pane) return true
    out.pop()
    return false
  }
  if (trailTo(node.a, pane, out)) return true
  if (trailTo(node.b, pane, out)) return true
  out.pop()
  return false
}

/** Whether `pane` is a leaf anywhere under `node`. */
function holds(node: MoveNode, pane: string): boolean {
  const trail: MoveNode[] = []
  return trailTo(node, pane, trail)
}

/**
 * The maximal chain of `axis` that `pane` is a member of, or `null` when it is a member of none.
 *
 * The frontend's mirror of `layout::chain_around`: take the deepest same-axis ancestor, then
 * climb back out through the unbroken run of them, because a chain's shares — and its member
 * indices — are only meaningful against its maximal root.
 */
function chainRootAround(root: MoveNode, pane: string, axis: 'row' | 'col'): MoveNode | null {
  const trail: MoveNode[] = []
  if (!trailTo(root, pane, trail)) return null

  let deepest = -1
  for (let i = trail.length - 1; i >= 0; i -= 1) {
    const node = trail[i]
    if (node !== undefined && node.kind === 'split' && node.axis === axis) {
      deepest = i
      break
    }
  }
  if (deepest === -1) return null

  let top = deepest
  while (top > 0) {
    const parent = trail[top - 1]
    if (parent === undefined || parent.kind !== 'split' || parent.axis !== axis) break
    top -= 1
  }
  return trail[top] ?? null
}

/** A chain's members, in order: the in-order run of same-axis splits, flattened. */
function membersOf(node: MoveNode, axis: 'row' | 'col'): MoveNode[] {
  if (node.kind === 'split' && node.axis === axis) {
    return [...membersOf(node.a, axis), ...membersOf(node.b, axis)]
  }
  return [node]
}

/**
 * What a release over `target`'s `edge` commits, or `null` when it would change nothing.
 *
 * Three ways to change nothing, and the third is the one worth having:
 *
 * 1. The pane was dropped on itself.
 * 2. Either id is no longer a leaf of this tree — a snapshot that moved under the drag.
 * 3. **The pane is already the member immediately on that side of the target**, within the
 *    chain the drop would insert into. This is `tabDrag.dropOutcome`'s "the boundary either
 *    side of where I already am" in tree clothing, and it is not cosmetic: see the header for
 *    what performing it actually costs.
 *
 * The adjacency test only fires when the pane is a member of the destination chain *in its own
 * right* — a pane nested inside a cross-axis subtree that happens to be a member is not
 * adjacent to anything in that chain, and moving it out is a real change.
 */
export function dropOutcome(
  root: MoveNode,
  pane: string,
  target: string,
  edge: DropEdge,
): MoveOutcome | null {
  if (pane === target) return null
  if (!holds(root, pane) || !holds(root, target)) return null

  const { axis, side } = intentFor(edge)
  const chain = chainRootAround(root, target, axis)
  if (chain !== null) {
    const members = membersOf(chain, axis)
    const here = members.findIndex((m) => holds(m, target))
    const mine = members.findIndex((m) => m.kind === 'leaf' && m.pane === pane)
    if (here >= 0 && mine >= 0) {
      if (side === 'before' && mine === here - 1) return null
      if (side === 'after' && mine === here + 1) return null
    }
  }

  return { pane, target, axis, side }
}
