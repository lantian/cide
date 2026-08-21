/**
 * The chevrons: one per conflict, in the gutter of the side pane it belongs to. (M20)
 *
 * This is IDEA's central gesture and the resolver was wrong without it. A button per *pane*
 * accepts a whole side; what a merge tool is for is accepting **this block** from the left and
 * **that block** from the right, in a file with a dozen of them. So each conflict gets its own
 * `»` (left pane, pointing into the result) or `«` (right pane), on the first line of that
 * conflict's own text in that side's document.
 *
 * # How the panes are aligned, when the resolver computes no diff
 *
 * `mergeModel.sideSpans` locates each conflict's fragment in the whole side file by searching
 * forward from the previous match — the fragment is verbatim text from that side, so it is
 * there, and searching forward keeps them in order. That is the alignment, and it is enough for
 * this: a chevron needs one line number per conflict per side, not a line-by-line correspondence.
 *
 * A side that *deleted* the region has no lines to hang a chevron on and gets none. The pane
 * heading's Accept button is what covers that case, which is why it survives alongside these.
 *
 * # Why a `StateField` of markers rather than `markers: () => build()`
 *
 * `gutter`'s `markers` callback runs on view updates, not when a React value changes, so a
 * closure over the current answers would go stale the moment somebody clicked one. The field is
 * pushed a new `RangeSet` by an effect, exactly as `mergeDecorations` is, and maps through
 * document changes in between.
 */
import { RangeSet, StateEffect, StateField } from '@codemirror/state'
import { EditorView, GutterMarker, gutter } from '@codemirror/view'

/**
 * One region's controls in one side pane: which region, on which line.
 *
 * Only regions **that side still has something to say about** get an entry. Once a side is
 * accepted or ignored its buttons go away, which is IDEA's behaviour and the fix for a real
 * complaint: a chevron that stayed after being used looked like it had not worked, and clicking
 * it again did something else entirely.
 */
export interface Chevron {
  id: string
  /** Zero-based line in this pane's document. */
  line: number
  /**
   * This side has already been answered, so it gets `↺` instead of `»` and `✕`.
   *
   * An answered side used to get nothing at all, which made it a dead end: the only way back was
   * the toolbar's Reset, on whichever block the toolbar happened to be on. It matters most for
   * blocks the user never decided — *Apply non-conflicting* settles a dozen at once, and a
   * decision made on somebody's behalf has to be reachable.
   */
  answered: boolean
}

const setChevrons = StateEffect.define<RangeSet<GutterMarker>>()

const chevronField = StateField.define<RangeSet<GutterMarker>>({
  create: () => RangeSet.empty,
  update(set, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setChevrons)) return effect.value
    }
    return set.map(tr.changes)
  },
})

class ChevronMarker extends GutterMarker {
  constructor(
    readonly id: string,
    readonly glyph: string,
    readonly answered: boolean,
    readonly act: () => (id: string, what: 'accept' | 'ignore' | 'revert') => void,
  ) {
    super()
  }

  /**
   * Compared on everything that changes the buttons' appearance.
   *
   * `act` is deliberately not compared: it is a stable accessor that reads a ref, so two markers
   * differing only by closure identity are the same pair of buttons, and re-rendering them on
   * every keystroke would drop the pointer mid-click.
   */
  override eq(other: GutterMarker): boolean {
    return (
      other instanceof ChevronMarker &&
      other.id === this.id &&
      other.glyph === this.glyph &&
      other.answered === this.answered
    )
  }

  override toDOM(): Node {
    const wrap = document.createElement('span')
    wrap.className = 'cm-mergeChevronPair'
    if (this.answered) {
      wrap.appendChild(
        this.button('↺', 'cm-mergeRevert', 'revert', 'Undo this side and ask about it again'),
      )
      return wrap
    }
    wrap.appendChild(
      this.button(
        this.glyph,
        'cm-mergeChevron',
        'accept',
        'Add this block to the result. Take the other side too and they arrive in the order you click.',
      ),
    )
    // IDEA's `X`. Rejecting is a decision, not the absence of one: it settles this side, its
    // buttons go, and the base stands for the block unless the other side is taken.
    wrap.appendChild(
      this.button('✕', 'cm-mergeIgnore', 'ignore', 'Reject this block and keep the base'),
    )
    return wrap
  }

  private button(
    glyph: string,
    className: string,
    what: 'accept' | 'ignore' | 'revert',
    title: string,
  ): HTMLButtonElement {
    const button = document.createElement('button')
    button.type = 'button'
    button.className = className
    button.textContent = glyph
    button.title = title
    button.setAttribute('data-audit', `merge-${what}`)
    // `mousedown` rather than `click`, and prevented: a click in a gutter otherwise moves the
    // editor's selection first, which scrolls the pane out from under the pointer.
    button.addEventListener('mousedown', (event) => {
      event.preventDefault()
      event.stopPropagation()
      this.act()(this.id, what)
    })
    return button
  }
}

/**
 * The gutter extension for one side pane.
 *
 * `which` decides the edge: the left pane's chevrons point right, at the result, and sit on its
 * right edge; the right pane's mirror that.
 *
 * The click handler is not here. It travels on the markers, through [`setGutter`], as an
 * *accessor* rather than a function — so the extension is built once with the editor and still
 * calls whatever React is currently holding, without the markers changing identity on every
 * render and dropping the pointer mid-click.
 */
export function conflictGutter(which: 'ours' | 'theirs'): ReturnType<typeof gutter>[] {
  return [
    chevronField,
    gutter({
      class: 'cm-mergeChevronGutter',
      side: which === 'ours' ? 'after' : 'before',
      markers: (view) => view.state.field(chevronField),
      // Nothing here depends on the *line*, only on the pushed set, so no `lineMarker`.
      initialSpacer: () =>
        new ChevronMarker('', which === 'ours' ? '»' : '«', false, () => () => {}),
    }),
  ]
}

/** Push a new set of chevrons into a side pane. */
export function setGutter(
  view: EditorView,
  which: 'ours' | 'theirs',
  chevrons: readonly Chevron[],
  act: () => (id: string, what: 'accept' | 'ignore' | 'revert') => void,
): void {
  const doc = view.state.doc
  const glyph = which === 'ours' ? '»' : '«'
  const ranges = chevrons
    .map((c) => {
      const line = Math.min(Math.max(c.line + 1, 1), doc.lines)
      return {
        at: doc.line(line).from,
        value: new ChevronMarker(c.id, glyph, c.answered, act),
      }
    })
    .sort((a, b) => a.at - b.at)
  view.dispatch({
    effects: setChevrons.of(RangeSet.of(ranges.map((r) => r.value.range(r.at)), true)),
  })
}
