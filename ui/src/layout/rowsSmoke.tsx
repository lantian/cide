/**
 * Renders `SplitTree` against a rows fixture and prints what came out.
 *
 * Driven by `ui/scripts/check-rows.mjs`. Not part of the app — nothing imports it, so it is
 * tree-shaken out of the real bundle — and it exists because the claim this milestone has to
 * keep is about *markup*: that a chain is one grid, that a vertical divider is a track of a
 * row's grid and therefore cannot be taller than that row, and that moving one divider
 * leaves every other track byte-identical.
 *
 * There is no layout engine here — no browser, no jsdom — so nothing below asserts a pixel.
 * That is a feature: the invariance is asserted *exactly*, on the numbers the browser would
 * be given, rather than to within a pixel of a screenshot.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import type { LayoutNode, Pane, PaneId, PaneTree, SplitId } from '@/ipc/generated'
import { applyDrag } from './Splitter'
import { chainAround, flattenChain, memberKey, SplitTree } from './SplitTree'

const pane = (n: number): Pane => ({
  id: `pane-${n}` as PaneId,
  kind: n === 1 ? 'claude' : 'shell',
  role: n === 1 ? 'primary' : 'auxiliary',
  session: null,
  conversation: null,
  conversationSince: null,
  continues: null,
  title: `cide : ${n}`,
})

const leaf = (n: number): LayoutNode => ({ kind: 'leaf', pane: pane(n).id })

const split = (id: string, axis: 'row' | 'col', a: LayoutNode, b: LayoutNode, ratio: number) =>
  ({ kind: 'split', id: id as SplitId, axis, a, b, ratio }) as LayoutNode

/**
 * `Col(Row(a, Row(b, Row(c, d))), Row(e, f))` — the user's own layout: four tiles over two,
 * the rows 50/50, and row one deliberately a *comb* rather than a balanced tree so the
 * flatten has to walk three levels to produce one four-member grid.
 *
 * Row one's ratios are 0.25, 1/3, 0.5, which multiply out to four equal quarters.
 */
export const FIXTURE: PaneTree = {
  root: split(
    'split-spine',
    'col',
    split(
      'split-r1a',
      'row',
      leaf(1),
      split('split-r1b', 'row', leaf(2), split('split-r1c', 'row', leaf(3), leaf(4), 0.5), 1 / 3),
      0.25,
    ),
    split('split-r2a', 'row', leaf(5), leaf(6), 0.5),
    0.5,
  ),
  focused: pane(1).id,
  maximized: null,
  panes: Object.fromEntries([1, 2, 3, 4, 5, 6].map((n) => [pane(n).id, pane(n)])) as Record<
    PaneId,
    Pane
  >,
}

/**
 * `Row(Row(a, b), c)` — the same three tiles, leaning the other way.
 *
 * `FIXTURE` is a right-hand comb, and a comb is the one shape where in-order and pre-order
 * traversal give the *same* divider list. So it cannot tell the two apart, and the divider
 * index is the single contract this renderer shares with `cide-core`: the frontend hands a
 * drag `dividers[k]`'s id and a pair share, and Rust resolves that id back to `k` through
 * its own in-order walk. Get the orders out of step and a drag silently moves the wrong
 * pair. A left-leaning chain is not a curiosity either — `add_row` with no anchor wraps the
 * whole root, which is what `pane.addRow`'s default does, so appending rows builds exactly
 * this shape.
 *
 * Ratios 2/3 and 0.5 multiply out to three equal thirds.
 */
const LEANING: PaneTree = {
  root: split(
    'lean-outer',
    'row',
    split('lean-inner', 'row', leaf(1), leaf(2), 0.5),
    leaf(3),
    2 / 3,
  ),
  focused: pane(1).id,
  maximized: null,
  panes: Object.fromEntries([1, 2, 3].map((n) => [pane(n).id, pane(n)])) as Record<PaneId, Pane>,
}

/**
 * `Row(a, Col(b, c))` — a pane stacked inside one cell of a row.
 *
 * Pane 2 is a member of the inner column and of no row of its own, so "this pane's row" can
 * only mean the one its *cell* is a tile of. `chainAround` is what decides that, and this is
 * the fixture where getting it wrong is visible: the menu item would grey itself out over a
 * two-tile row.
 */
const STACKED: PaneTree = {
  root: split(
    'stack-row',
    'row',
    leaf(1),
    split('stack-col', 'col', leaf(2), leaf(3), 0.5),
    0.75,
  ),
  focused: pane(2).id,
  maximized: null,
  panes: Object.fromEntries([1, 2, 3].map((n) => [pane(n).id, pane(n)])) as Record<PaneId, Pane>,
}

/** One pane, no splits at all: a row of one, which is the item's disabled case. */
const LONE: PaneTree = {
  root: leaf(1),
  focused: pane(1).id,
  maximized: null,
  panes: { [pane(1).id]: pane(1) } as Record<PaneId, Pane>,
}

/**
 * Row one after `add_tile` beside its *first* tile — exactly what `graft` writes: the leaf is
 * replaced in place by a fresh two-child split, so the chain root keeps its id while a new
 * divider appears at the head of the in-order list.
 *
 * The reason this fixture exists: member keys must be stable across that insertion, or React
 * remounts terminals that only shuffled along the row. Keying by `dividers[k]` looks
 * equivalent and is not — `dividers[0]` is the one id the insertion does change.
 */
const ROW_ONE = (FIXTURE.root as Extract<LayoutNode, { kind: 'split' }>).a
const ROW_ONE_AFTER = split(
  'split-r1a',
  'row',
  split('split-r1new', 'row', leaf(1), leaf(7), 0.5),
  (ROW_ONE as Extract<LayoutNode, { kind: 'split' }>).b,
  0.5,
) as Extract<LayoutNode, { kind: 'split' }>

const before = flattenChain(ROW_ONE as Extract<LayoutNode, { kind: 'split' }>)
const after = flattenChain(ROW_ONE_AFTER)

interface Element {
  tag: string
  audit: string | undefined
  attrs: string
  chain: number | undefined
}

/**
 * Enough of an HTML reader to answer one question the flat string cannot: which chain grid
 * is a given splitter *inside*.
 *
 * A stack of open tags, nothing more. React's static markup is well-formed and closes
 * everything but the void elements, so this needs no tolerance for real-world HTML.
 */
function parse(html: string) {
  const chains: { axis: string; tracks: string; fractions: number[]; depth: number }[] = []
  const splitters: { split: string; orientation: string; chain: number; hidden: boolean }[] = []
  const stack: Element[] = []
  const buttons: string[] = []
  // Per pane, which `data-edge-*` marks its leaf wrapper carries — the marks that drop a
  // frame border side where the pane sits flush against the app chrome. Panes with no mark
  // are absent, so an interior pane appearing here is a failure the assertion can name.
  const edges: Record<string, string> = {}
  let addRow = 0

  const attr = (attrs: string, name: string) =>
    new RegExp(`${name}="([^"]*)"`).exec(attrs)?.[1]

  const tags = /<(\/?)([a-zA-Z0-9]+)((?:"[^"]*"|[^>])*?)(\/?)>/g
  for (let m = tags.exec(html); m !== null; m = tags.exec(html)) {
    const [, closing, tag, attrs = '', selfClosing] = m
    if (closing) {
      stack.pop()
      continue
    }
    const audit = attr(attrs, 'data-audit')
    const el: Element = { tag: tag ?? '', audit, attrs, chain: undefined }

    if (audit === 'chain') {
      const axis = attr(attrs, 'data-axis') ?? ''
      const style = attr(attrs, 'style') ?? ''
      const which = axis === 'row' ? 'grid-template-columns' : 'grid-template-rows'
      const tracks = new RegExp(`${which}:([^;"]*)`).exec(style)?.[1]?.trim() ?? ''
      el.chain = chains.length
      chains.push({
        axis,
        tracks,
        fractions: tracks
          .split('var(--w-splitter)')
          .map((t) => Number.parseFloat(t.trim().replace(/fr$/, ''))),
        depth: stack.filter((n) => n.audit === 'chain').length,
      })
    }
    if (audit === 'splitter') {
      const owner = [...stack].reverse().find((n) => n.chain !== undefined)
      splitters.push({
        split: attr(attrs, 'data-split') ?? '',
        orientation: attr(attrs, 'aria-orientation') ?? '',
        chain: owner?.chain ?? -1,
        hidden: (attr(attrs, 'style') ?? '').includes('hidden'),
      })
    }
    if (audit === 'addRow') addRow += 1
    if (tag === 'button' && attr(attrs, 'aria-label')?.startsWith('Add a row')) {
      buttons.push(attr(attrs, 'aria-label') ?? '')
    }
    // The pane div is rendered inside the leaf wrapper, so when its open tag arrives the
    // wrapper — the element the marks live on — is already on the stack.
    const paneId = attr(attrs, 'data-pane-id')
    if (paneId !== undefined) {
      const marks = ['left', 'top', 'bottom'].filter((e) =>
        stack.some((n) => n.attrs.includes(`data-edge-${e}`)),
      )
      if (marks.length > 0) edges[paneId] = marks.join(',')
    }

    const isVoid = /^(area|base|br|col|embed|hr|img|input|link|meta|source|track|wbr)$/.test(
      tag ?? '',
    )
    if (!selfClosing && !isVoid) stack.push(el)
  }

  return { chains, splitters, addRow, buttons, edges }
}

function render(tree: PaneTree, withAddRow: boolean) {
  return parse(
    renderToStaticMarkup(
      <SplitTree
        tree={tree}
        {...(withAddRow ? { onAddRow: () => {} } : {})}
        renderPane={(p, index) => (
          <div data-audit="pane" data-pane-id={p.id}>
            {index}: {p.title}
          </div>
        )}
      />,
    ),
  )
}

const plain = render(FIXTURE, true)
const maximized = render({ ...FIXTURE, maximized: pane(3).id, focused: pane(3).id }, true)
const noStrip = render(FIXTURE, false)
const leaning = render(LEANING, false)

// The pure drag, checked without a DOM at all. `t` is an exact binary fraction on purpose:
// the claim is that untouched members are *copied*, and a value that rounds would make the
// assertion about IEEE754 instead.
const input = [0.25, 0.25, 0.25, 0.25]
const dragged = applyDrag(input, 0, 0.75)

/**
 * What *Even out this row* would act on: the members of the chain, and their shares.
 *
 * `sum` is the assertion that matters most. A chain's shares sum to 1 only at its maximal
 * root, so a walk that stopped at the innermost same-axis split without climbing back out
 * reports a fraction of a row as a whole one — and on `LEANING`, which is a *left*-hand
 * comb, that is exactly the mistake that is invisible on every right-hand one.
 */
const around = (tree: PaneTree, n: number, axis: 'row' | 'col') => {
  const chain = chainAround(tree.root, pane(n).id, axis)
  if (chain === null) return null
  return {
    members: chain.members.map(memberKey),
    fractions: chain.fractions,
    sum: chain.fractions.reduce((a, b) => a + b, 0),
  }
}

console.log(
  JSON.stringify({
    around: {
      rowOne: around(FIXTURE, 3, 'row'),
      rowTwo: around(FIXTURE, 5, 'row'),
      spine: around(FIXTURE, 1, 'col'),
      leaning: around(LEANING, 1, 'row'),
      stacked: around(STACKED, 2, 'row'),
      stackedCol: around(STACKED, 2, 'col'),
      lone: around(LONE, 1, 'row'),
      noColumn: around(LEANING, 1, 'col'),
    },
    plain: {
      chains: plain.chains,
      splitters: plain.splitters,
      addRow: plain.addRow,
      buttons: plain.buttons,
      edges: plain.edges,
    },
    maximized: {
      chains: maximized.chains.length,
      // Per chain, how many of its dividers were explicitly hidden.
      hiddenByChain: maximized.chains.map(
        (_, i) => maximized.splitters.filter((s) => s.chain === i && s.hidden).length,
      ),
      splittersByChain: maximized.chains.map(
        (_, i) => maximized.splitters.filter((s) => s.chain === i).length,
      ),
      addRow: maximized.addRow,
      edges: maximized.edges,
    },
    noStrip: { addRow: noStrip.addRow, splitters: noStrip.splitters.length },
    leaning: {
      chains: leaning.chains.length,
      fractions: leaning.chains[0]?.fractions ?? [],
      // In document order, which for a grid whose members are placed on explicit tracks is
      // also left-to-right order.
      splitters: leaning.splitters.map((s) => s.split),
      edges: leaning.edges,
    },
    keys: {
      before: before.members.map(memberKey),
      after: after.members.map(memberKey),
      // The id the insertion moves, and the one it leaves alone.
      dividerZeroMoved: before.dividers[0] !== after.dividers[0],
      chainRootHeld: ROW_ONE_AFTER.id === (ROW_ONE as Extract<LayoutNode, { kind: 'split' }>).id,
    },
    drag: {
      out: dragged,
      untouchedIdentical: dragged[2] === input[2] && dragged[3] === input[3],
      pairSum: (dragged[0] ?? 0) + (dragged[1] ?? 0),
      inputUnchanged: input.join(','),
    },
  }),
)
