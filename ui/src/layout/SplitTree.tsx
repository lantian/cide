/**
 * The recursive renderer for a tab's `PaneTree`.
 *
 * A *chain* — a maximal run of same-axis splits — is drawn as **one** `display: grid` with
 * `2n - 1` tracks: `f0 fr, var(--w-splitter), f1 fr, …`. The dividers are real grid tracks
 * rather than absolutely positioned overlays, and because a row is one grid, **a vertical
 * divider physically cannot be taller than its row**. That is the user's complaint answered
 * structurally: there is no clamp and no `if (axis === 'row')` anywhere below.
 *
 * The flatten is deliberately general — any shape, any depth, any arity — so a tree written
 * by an older build renders correctly the moment this ships and no migration stands behind
 * it. The one honest cost: a user who already built a 2x2 the old way has a genuine
 * `Row(Col(a,b), Col(c,d))` on disk and keeps its full-height centre divider, because that
 * really is two columns. It is not broken and `add_row` works on it; rebuilding is a few
 * clicks.
 *
 * This file knows nothing about terminals and nothing about IPC. Panes arrive through
 * `renderPane` and mutations leave through callbacks, which is what lets the whole grid be
 * rendered from a fixture with no Rust behind it.
 */
import { Fragment, useLayoutEffect, useRef } from 'react'
import type { CSSProperties, ReactNode } from 'react'
// Types only, erased at build time — see the note in Splitter.tsx.
import type {
  Axis,
  LayoutNode,
  Pane,
  PaneId,
  PaneTree,
  SplitId,
  SplitIntent,
} from '@/ipc/generated'
import { useSnapshotRev } from '@/store/snapshotRev'
import { applyTracks, Splitter, trackTemplate } from './Splitter'
import styles from './SplitTree.module.css'

export interface SplitTreeProps {
  tree: PaneTree
  /** Renders one pane. Supplied by the caller so this file knows nothing about terminals. */
  renderPane: (pane: Pane, index: number) => ReactNode
  onFocus?: ((pane: PaneId) => void) | undefined
  onRatioCommit?: ((split: SplitId, ratio: number) => void) | undefined
  /**
   * Add a full-width row holding one pane. Absent hides the strip entirely, so the tab still
   * renders — and still resizes — in a window that has no such gesture to offer.
   */
  onAddRow?: ((intent: SplitIntent | null) => void) | undefined
}

/**
 * Maximize is a flag, not tree surgery.
 *
 * The member holding the maximized pane is spanned across its chain's whole grid area, and
 * every other member — plus every divider — is hidden. Applied at every chain on the path,
 * the maximized leaf ends up covering the tab. Outside that path no track changes, so those
 * panes' terminals keep their exact pixel size and come back with no reflow. The one
 * exception is a hidden pane *inside* the maximized member: the member itself grows to fill
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
 * Where a child sits among its chain's `2n - 1` tracks: members on the odd lines, dividers
 * on the even ones.
 *
 * Stated rather than left to auto-placement, and only maximize shows why. A spanned member
 * occupies every cell of its grid, so an auto-placed sibling finds no free cell and lands in
 * a fresh *implicit* track instead — and `-1` in `FILL` means the last explicit line, so the
 * spanned member does not cover it. Measured in Chrome on the mock's 2x2 at 1000x600: the
 * maximized pane came out 62px short of the tab, and the hidden panes, far from holding
 * still, were squeezed into 6px-wide strips, which is a live xterm reflowing to a couple of
 * columns and back again.
 */
function place(track: number, axis: Axis): CSSProperties {
  const line = String(track)
  return axis === 'row' ? { gridColumn: line, gridRow: '1' } : { gridRow: line, gridColumn: '1' }
}

function contains(node: LayoutNode, pane: PaneId): boolean {
  if (node.kind === 'leaf') return node.pane === pane
  return contains(node.a, pane) || contains(node.b, pane)
}

type SplitNode = Extract<LayoutNode, { kind: 'split' }>

export interface Chain {
  axis: Axis
  /** In order. Each is a leaf or a split on the *other* axis, never a same-axis split. */
  members: LayoutNode[]
  /** In in-order, so `dividers[k]` separates `members[k]` and `members[k + 1]`. */
  dividers: SplitId[]
  /** Each member's share, the product of the ratios on its path. Sums to 1. */
  fractions: number[]
}

/**
 * Flatten the maximal same-axis chain rooted at `node`.
 *
 * In-order, because that is what makes `dividers[k]` the divider between members `k` and
 * `k + 1` with no new id space: a binary chain visited in-order yields
 * `member, split, member, …, member`. The mirror of `cide-core`'s `weights` / `chain_splits`,
 * and the two must agree — a divider commit sends `dividers[k]`'s id and a pair share, which
 * Rust resolves through exactly this indexing.
 */
export function flattenChain(node: SplitNode): Chain {
  const members: LayoutNode[] = []
  const dividers: SplitId[] = []
  const fractions: number[] = []

  const visit = (n: LayoutNode, factor: number) => {
    if (n.kind === 'split' && n.axis === node.axis) {
      visit(n.a, factor * n.ratio)
      dividers.push(n.id)
      visit(n.b, factor * (1 - n.ratio))
      return
    }
    members.push(n)
    fractions.push(factor)
  }
  visit(node, 1)

  return { axis: node.axis, members, dividers, fractions }
}

/**
 * The chain of `axis` that `pane` belongs to, or `null` when no split of that axis is above
 * it — a lone pane, or one under nothing but cross-axis splits, which is a row of one.
 *
 * The mirror of `cide-core::layout`'s `chain_around`, and it has to give the same answer:
 * the *Even out this row* menu item greys itself out from this member count while the
 * command it fires acts on whatever Rust finds, so a disagreement is an item that claims a
 * row is a row of one and then evens out three tiles when you pick it anyway.
 *
 * Innermost, then outwards. The deepest same-axis ancestor is the one whose members are the
 * pane's actual neighbours; climbing back out through the unbroken run of same-axis splits
 * above it is what makes the answer the chain's *maximal* root, which is the only node
 * whose shares sum to 1 — `flattenChain` on anything less is a fraction of a row read as a
 * whole one.
 */
export function chainAround(root: LayoutNode, pane: PaneId, axis: Axis): Chain | null {
  const trail: SplitNode[] = []
  const walk = (node: LayoutNode): boolean => {
    if (node.kind === 'leaf') return node.pane === pane
    trail.push(node)
    if (walk(node.a) || walk(node.b)) return true
    trail.pop()
    return false
  }
  if (!walk(root)) return null

  let k = trail.length - 1
  while (k >= 0 && trail[k]?.axis !== axis) k -= 1
  if (k < 0) return null
  while (k > 0 && trail[k - 1]?.axis === axis) k -= 1

  const node = trail[k]
  return node === undefined ? null : flattenChain(node)
}

/**
 * A chain member's React key: its own id, which no insertion elsewhere in the chain can
 * change. Exported so `check:rows` can pin the property the keying exists for.
 */
export function memberKey(member: LayoutNode): string {
  return member.kind === 'leaf' ? member.pane : member.id
}

interface ChainNodeProps {
  chain: Chain
  /** One per member, already walked, in order. */
  rendered: ReactNode[]
  /** Which member is spanned across the whole grid, or `-1` for none. */
  maximizedMember: number
  style: CSSProperties | undefined
  onRatioCommit?: ((split: SplitId, ratio: number) => void) | undefined
}

function ChainNode({
  chain,
  rendered,
  maximizedMember,
  style,
  onRatioCommit,
}: ChainNodeProps) {
  const gridRef = useRef<HTMLDivElement>(null)
  const dragging = useRef(false)
  const { axis, fractions, dividers, members } = chain
  /*
   * Subscribed so this chain re-renders — and the effect below re-asserts — on every snapshot,
   * which is the contract that effect states. It used to get that for free: every snapshot gave
   * every tab a new identity and re-rendered every `SplitTree`. With the mirror structurally
   * shared and panes memoised, a snapshot that did not touch this tab re-renders nothing here,
   * and a clamped commit or a cancelled drag would keep its divider where the pointer let go.
   * A primitive, so this costs one cheap render of the chain (its children are memoised
   * elements) per revision — not the whole tab. Through `snapshotRev`, not the store, because
   * `check:rows` renders this file under node and the store's import graph cannot load there.
   */
  useSnapshotRev()

  // Deliberately runs on every render, with no dependency array.
  //
  // A drag mutates this element's style behind React's back, so React's own style diff is
  // no longer a reliable description of the DOM: when a commit is clamped or rejected and
  // the shares come back unchanged, the diff finds nothing to write and the divider stays
  // where the pointer left it, disagreeing with the model for good. Re-asserting from props
  // costs one string assignment per chain per render and makes the model authoritative
  // again the moment a snapshot lands.
  useLayoutEffect(() => {
    const grid = gridRef.current
    if (!grid || dragging.current) return
    applyTracks(grid, axis, fractions)
  })

  const template = trackTemplate(fractions)
  // The cross axis is one explicit track. Without it the children auto-place into an
  // implicit grid — a `col` chain would lay its panes out side by side.
  const layout: CSSProperties =
    axis === 'row'
      ? { gridTemplateColumns: template, gridTemplateRows: 'minmax(0, 1fr)' }
      : { gridTemplateRows: template, gridTemplateColumns: 'minmax(0, 1fr)' }

  const hideDividers = maximizedMember >= 0

  return (
    <div
      ref={gridRef}
      className={styles.split}
      data-audit="chain"
      data-axis={axis}
      style={{ ...style, ...layout }}
    >
      {members.map((member, i) => (
        // A `Fragment`, not a wrapper element: two children per member — the member and the
        // divider after it — and anything real between them and the grid would break the
        // explicit track placement below.
        //
        // Keyed by the *member's* own id, never by the array index and never by the divider
        // beside it. Both of those move when a tile is inserted mid-row — divider `k`
        // separates members `k` and `k + 1`, so an insertion shifts every later divider onto
        // a different member — and React would then unmount live terminals that merely
        // shuffled along the row. A leaf's `PaneId` and a cross-axis member's `SplitId` both
        // survive an insertion anywhere else in the chain.
        <Fragment key={memberKey(member)}>
          {rendered[i]}
          {i < dividers.length && (
            <Splitter
              split={dividers[i] as SplitId}
              axis={axis}
              fractions={fractions}
              index={i}
              gridRef={gridRef}
              hidden={hideDividers}
              onDragActive={(active) => {
                dragging.current = active
              }}
              onCommit={onRatioCommit}
            />
          )}
        </Fragment>
      ))}
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

/**
 * Which outer edges of the canvas a subtree touches — all three true at the root, narrowed on
 * the way down: a `row` chain keeps `left` only for its first member, a `col` chain keeps
 * `top` only for its first and `bottom` only for its last, and each axis passes the other
 * two flags through untouched.
 *
 * These are the edges where the tree meets the app's own chrome, and each of those chrome
 * pieces already draws the seam: the sidebar splitter's 6px `--panel-2` strip (plus the
 * sidebar's `border-right`) on the left, the tab strip's `border-bottom` above, and the
 * add-row strip's / status bar's `border-top` below. The canvas gutter there was a second
 * gutter and the pane's frame border a second line — the user circled the left one in a
 * screenshot, then pointed out top and bottom carry the same padding — pushing a pane's
 * content out of line with the tab strip's tabs. So the canvas pads only the right edge
 * (`SplitTree.module.css`), which meets the bare window edge and has no chrome line to lean
 * on, and a leaf on a flush edge drops that side of its frame border via `data-edge-*` →
 * `--pane-edge-*`, which `PaneTitleBar.module.css` reads as that side's border width.
 * Content then starts where a tab starts.
 */
interface Edges {
  left: boolean
  top: boolean
  bottom: boolean
}

const ALL_EDGES: Edges = { left: true, top: true, bottom: true }

function walk(
  node: LayoutNode,
  ctx: WalkContext,
  style: CSSProperties | undefined,
  edges: Edges,
): ReactNode {
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

    // A maximized leaf spans every chain on its path, so it covers the canvas and every
    // outer edge is its edge whatever its resting position was — without this an interior
    // pane grows stray 1px lines against the surrounding chrome for exactly as long as it
    // is maximized.
    const flush = ctx.tree.maximized === node.pane ? ALL_EDGES : edges

    return (
      <div
        key={node.pane}
        className={styles.leaf}
        style={style}
        data-edge-left={flush.left ? '' : undefined}
        data-edge-top={flush.top ? '' : undefined}
        data-edge-bottom={flush.bottom ? '' : undefined}
        // Capture phase, because the click that focuses a pane usually lands on the
        // terminal's own textarea and is consumed there.
        onFocusCapture={raise}
        onPointerDownCapture={raise}
      >
        {ctx.renderPane(pane, index)}
      </div>
    )
  }

  const chain = flattenChain(node)
  const maximized = ctx.tree.maximized
  const holder =
    maximized === null ? -1 : chain.members.findIndex((m) => contains(m, maximized))

  // Ordered, not built inside the JSX: member `i` must draw its pane numbers before `i + 1`.
  const rendered = chain.members.map((member, i) => {
    let style: CSSProperties = place(2 * i + 1, chain.axis)
    if (holder >= 0) style = i === holder ? FILL : { ...style, ...HIDDEN }
    // Neither this member nor any other holds it: the chain is already inside a hidden
    // subtree, and `visibility` inherits, so there is nothing left to do here.
    //
    // A chain narrows the flags along its own axis — first member keeps the leading edge,
    // last keeps the trailing one — and passes the cross-axis flags through: every member of
    // a `row` chain shares its chain's top and bottom, every member of a `col` its left.
    const memberEdges: Edges =
      chain.axis === 'row'
        ? { left: edges.left && i === 0, top: edges.top, bottom: edges.bottom }
        : {
            left: edges.left,
            top: edges.top && i === 0,
            bottom: edges.bottom && i === chain.members.length - 1,
          }
    return walk(member, ctx, style, memberEdges)
  })

  return (
    // Keyed by the chain root's own split id, which an insertion anywhere inside the chain
    // leaves alone. `dividers[0]` looks equivalent and is not: it is the deepest split down
    // the `a` spine, so adding a tile beside the *first* tile of a row mints a new one and
    // React would remount that row's entire grid — every terminal in it — for a gesture that
    // touched one cell.
    <ChainNode
      key={node.id}
      chain={chain}
      rendered={rendered}
      maximizedMember={holder}
      style={style}
      onRatioCommit={ctx.onRatioCommit}
    />
  )
}

/** What the two `+ row` buttons ask for. Named here so the audit can assert on them. */
const ROW_INTENTS: ReadonlyArray<{ label: string; name: string; intent: SplitIntent }> = [
  { label: 'bash row', name: 'Add a row with a shell', intent: { kind: 'shell' } },
  { label: 'claude row', name: 'Add a row with a Claude session', intent: { kind: 'newClaude' } },
]

export function SplitTree({
  tree,
  renderPane,
  onFocus,
  onRatioCommit,
  onAddRow,
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
      <div className={styles.canvas}>{walk(tree.root, ctx, undefined, ALL_EDGES)}</div>
      {/*
        * A flex sibling of the tree, outside every grid, so the divider arithmetic stays
        * pure — an extra track inside the chain would have to be subtracted from `usable` in
        * every drag computation. Withheld while a pane is maximized: the gesture would
        * un-maximize the tab out from under the user to show a row they cannot see yet.
        */}
      {onAddRow && tree.maximized === null && (
        <div className={styles.addRow} data-audit="addRow">
          {ROW_INTENTS.map(({ label, name, intent }) => (
            <button
              key={label}
              type="button"
              className={styles.addRowButton}
              title={name}
              aria-label={name}
              onClick={() => onAddRow(intent)}
            >
              {label}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
