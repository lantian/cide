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
import { SplitTree } from './SplitTree'

const pane = (n: number): Pane => ({
  id: `pane-${n}` as PaneId,
  kind: n === 1 ? 'claude' : 'shell',
  role: n === 1 ? 'primary' : 'auxiliary',
  session: null,
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

    const isVoid = /^(area|base|br|col|embed|hr|img|input|link|meta|source|track|wbr)$/.test(
      tag ?? '',
    )
    if (!selfClosing && !isVoid) stack.push(el)
  }

  return { chains, splitters, addRow, buttons }
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

// The pure drag, checked without a DOM at all. `t` is an exact binary fraction on purpose:
// the claim is that untouched members are *copied*, and a value that rounds would make the
// assertion about IEEE754 instead.
const input = [0.25, 0.25, 0.25, 0.25]
const dragged = applyDrag(input, 0, 0.75)

console.log(
  JSON.stringify({
    plain: { chains: plain.chains, splitters: plain.splitters, addRow: plain.addRow, buttons: plain.buttons },
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
    },
    noStrip: { addRow: noStrip.addRow, splitters: noStrip.splitters.length },
    drag: {
      out: dragged,
      untouchedIdentical: dragged[2] === input[2] && dragged[3] === input[3],
      pairSum: (dragged[0] ?? 0) + (dragged[1] ?? 0),
      inputUnchanged: input.join(','),
    },
  }),
)
