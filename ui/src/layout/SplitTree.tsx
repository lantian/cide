/**
 * The recursive renderer for a tab's `PaneTree`.
 *
 * A split is a two-child `display: grid` with an `Nfr var(--w-splitter) Mfr` template, so
 * the divider is a real grid track rather than an absolutely positioned overlay. Nesting
 * splits nests grids; the mock's 2x2 is a `row` split whose two children are `col` splits.
 *
 * This file knows nothing about terminals and nothing about IPC. Panes arrive through
 * `renderPane` and mutations leave through callbacks, which is what lets the whole grid be
 * rendered from a fixture with no Rust behind it.
 */
import { useLayoutEffect, useRef } from 'react'
import type { CSSProperties, ReactNode } from 'react'
// Types only, erased at build time — see the note in Splitter.tsx.
import type { Axis, LayoutNode, Pane, PaneId, PaneTree, SplitId } from '@/ipc/generated'
import { applyTemplate, Splitter, splitTemplate } from './Splitter'
import styles from './SplitTree.module.css'

export interface SplitTreeProps {
  tree: PaneTree
  /** Renders one pane. Supplied by the caller so this file knows nothing about terminals. */
  renderPane: (pane: Pane, index: number) => ReactNode
  onFocus?: ((pane: PaneId) => void) | undefined
  onRatioCommit?: ((split: SplitId, ratio: number) => void) | undefined
}

/**
 * Maximize is a flag, not tree surgery.
 *
 * The branch holding the maximized pane is spanned across its parent's whole grid area, and
 * its sibling — plus the divider — is hidden. Applied at every split on the path, the
 * maximized leaf ends up covering the tab. Outside that path no track changes, so those
 * panes' terminals keep their exact pixel size and come back with no reflow. The one
 * exception is a hidden pane *inside* the maximized branch: the branch itself grows to fill
 * the tab, so its own children grow with it and that pane is resized once each way.
 *
 * `visibility: hidden`, never `display: none`: a display-none terminal measures zero in
 * every dimension, and the next `fit()` computes garbage from it. Hidden elements are also
 * not hit-test targets, so the covered siblings cannot swallow clicks meant for the
 * maximized pane.
 */
const FILL: CSSProperties = { gridArea: '1 / 1 / -1 / -1' }
const HIDDEN: CSSProperties = { visibility: 'hidden' }

/**
 * Where a child sits among its split's three tracks: `a`, the divider, `b`.
 *
 * Stated rather than left to auto-placement, and only maximize shows why. A spanned branch
 * occupies every cell of its grid, so an auto-placed sibling finds no free cell and lands in
 * a fresh *implicit* track instead — and `-1` in `FILL` means the last explicit line, so the
 * spanned branch does not cover it. Measured in Chrome on the mock's 2x2 at 1000x600: the
 * maximized pane came out 62px short of the tab, and the hidden panes, far from holding
 * still, were squeezed into 6px-wide strips, which is a live xterm reflowing to a couple of
 * columns and back again.
 */
function place(track: 1 | 2 | 3, axis: Axis): CSSProperties {
  const line = String(track)
  return axis === 'row' ? { gridColumn: line, gridRow: '1' } : { gridRow: line, gridColumn: '1' }
}

function contains(node: LayoutNode, pane: PaneId): boolean {
  if (node.kind === 'leaf') return node.pane === pane
  return contains(node.a, pane) || contains(node.b, pane)
}

interface SplitNodeProps {
  split: SplitId
  axis: Axis
  ratio: number
  a: ReactNode
  b: ReactNode
  splitterHidden: boolean
  style: CSSProperties | undefined
  onRatioCommit?: ((split: SplitId, ratio: number) => void) | undefined
}

function SplitNode({
  split,
  axis,
  ratio,
  a,
  b,
  splitterHidden,
  style,
  onRatioCommit,
}: SplitNodeProps) {
  const gridRef = useRef<HTMLDivElement>(null)
  const dragging = useRef(false)

  // Deliberately runs on every render, with no dependency array.
  //
  // A drag mutates this element's style behind React's back, so React's own style diff is
  // no longer a reliable description of the DOM: when a commit is clamped or rejected and
  // the ratio comes back unchanged, the diff finds nothing to write and the divider stays
  // where the pointer left it, disagreeing with the model for good. Re-asserting from props
  // costs one string assignment per split per render and makes the model authoritative
  // again the moment a snapshot lands.
  useLayoutEffect(() => {
    const grid = gridRef.current
    if (!grid || dragging.current) return
    applyTemplate(grid, axis, ratio)
  })

  const template = splitTemplate(ratio)
  // The cross axis is one explicit track. Without it the three children auto-place into an
  // implicit grid — a `col` split would lay its two panes out side by side.
  const layout: CSSProperties =
    axis === 'row'
      ? { gridTemplateColumns: template, gridTemplateRows: 'minmax(0, 1fr)' }
      : { gridTemplateRows: template, gridTemplateColumns: 'minmax(0, 1fr)' }

  return (
    <div ref={gridRef} className={styles.split} style={{ ...style, ...layout }}>
      {a}
      <Splitter
        split={split}
        axis={axis}
        ratio={ratio}
        gridRef={gridRef}
        hidden={splitterHidden}
        onDragActive={(active) => {
          dragging.current = active
        }}
        onCommit={onRatioCommit}
      />
      {b}
    </div>
  )
}

interface WalkContext {
  tree: PaneTree
  renderPane: (pane: Pane, index: number) => ReactNode
  onFocus?: ((pane: PaneId) => void) | undefined
  onRatioCommit?: ((split: SplitId, ratio: number) => void) | undefined
  /** Hands out the next depth-first pane number. */
  next: () => number
}

function walk(node: LayoutNode, ctx: WalkContext, style: CSSProperties | undefined): ReactNode {
  if (node.kind === 'leaf') {
    // Consumed before the lookup so a pane that is somehow missing does not renumber the
    // ones after it.
    const index = ctx.next()
    const pane = ctx.tree.panes[node.pane]
    // `cide-core` asserts that leaf ids and `panes` keys are the same set, so this is
    // unreachable; rendering an empty cell still beats throwing mid-paint and taking the
    // other three live terminals down with it.
    if (!pane) return null

    // Withheld once this pane already holds focus. `onFocus` is an IPC round trip that
    // re-reads the tree and re-renders every pane in the tab, and every one of those panes
    // is then remeasured and refitted; a click inside the terminal the user is already
    // typing in — or each pointerdown of a drag-select — must not cost that.
    const raise = ctx.tree.focused === node.pane ? undefined : () => ctx.onFocus?.(node.pane)

    return (
      <div
        key={node.pane}
        className={styles.leaf}
        style={style}
        // Capture phase, because the click that focuses a pane usually lands on the
        // terminal's own textarea and is consumed there.
        onFocusCapture={raise}
        onPointerDownCapture={raise}
      >
        {ctx.renderPane(pane, index)}
      </div>
    )
  }

  const maximized = ctx.tree.maximized
  let aStyle: CSSProperties = place(1, node.axis)
  let bStyle: CSSProperties = place(3, node.axis)
  let splitterHidden = false
  if (maximized !== null) {
    if (contains(node.a, maximized)) {
      aStyle = FILL
      bStyle = { ...bStyle, ...HIDDEN }
      splitterHidden = true
    } else if (contains(node.b, maximized)) {
      aStyle = { ...aStyle, ...HIDDEN }
      bStyle = FILL
      splitterHidden = true
    }
    // Neither branch holds it: this split is already inside a hidden subtree, and
    // `visibility` inherits, so there is nothing left to do here.
  }

  // Ordered, not inlined into the JSX below: `a` must draw its pane numbers before `b`.
  const a = walk(node.a, ctx, aStyle)
  const b = walk(node.b, ctx, bStyle)

  return (
    <SplitNode
      key={node.id}
      split={node.id}
      axis={node.axis}
      ratio={node.ratio}
      a={a}
      b={b}
      splitterHidden={splitterHidden}
      style={style}
      onRatioCommit={ctx.onRatioCommit}
    />
  )
}

export function SplitTree({
  tree,
  renderPane,
  onFocus,
  onRatioCommit,
}: SplitTreeProps): ReactNode {
  // Pane numbers are the depth-first position, computed during this walk rather than
  // stored: they are presentation — the thing "focus pane 3" refers to — and storing them
  // would mean renumbering every later sibling on each split and each close.
  let n = 0
  const ctx: WalkContext = {
    tree,
    renderPane,
    onFocus,
    onRatioCommit,
    next: () => (n += 1),
  }

  return (
    <div className={styles.tree} data-audit="paneTree">
      {walk(tree.root, ctx, undefined)}
    </div>
  )
}
