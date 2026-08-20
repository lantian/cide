/**
 * The annotate gutter: a column of `author date` to the left of the line numbers, an age tint,
 * and a hover card. (M19)
 *
 * Every *decision* is `blameModel.ts` — what a cell says, which lines carry a label, how old is
 * old, what the card's rows are — because that file is import-free and `check:blame` compiles and
 * drives it. What is left here is the part that genuinely needs CodeMirror: a `RangeSet` of
 * markers, a gutter, a tooltip, and the four DOM events that move between them.
 *
 * # Why a `showTooltip` and not a decoration, and not `hoverTooltip`
 *
 * `ctrlLink.ts`' header *rejects* CodeMirror's hover machinery, and it is worth being explicit
 * that this is not a contradiction. That case wanted **a mark on a range and a cursor change** —
 * `crosshairCursor`'s mechanic, one level down from a tooltip — and getting it out of
 * `hoverTooltip` would have meant fighting a component built to show a floating card. This case
 * wants exactly the floating card: it has to escape the pane's `overflow: hidden`, flip above the
 * line when it is near the bottom, follow the buffer when it scrolls, and be reachable by the
 * pointer so its two buttons can be clicked. That is `showTooltip`, and reimplementing it over a
 * decoration would be reimplementing all four.
 *
 * `showTooltip` and not `hoverTooltip`, though, and the reason is structural rather than a
 * preference: **the gutter is not part of `contentDOM`**. `hoverTooltip` installs its listeners on
 * the content element and reports a *document position*, so it never sees a pointer that is over
 * the margin — the entire surface this feature lives on. So the hover comes from the gutter's own
 * `domEventHandlers`, which the gutter plugin attaches to its own element, and the delay
 * ([`BLAME_HOVER_MS`]) is ours to keep.
 *
 * # A touched line is drawn uncommitted, whatever marker it still carries
 *
 * The markers are mapped through `tr.changes` on every edit rather than re-fetched, which is exact
 * and costs no round trip per keystroke. But mapping is the *wrong* answer for a line the user
 * edited: its anchor survives the change, so the marker survives with it, and the line keeps the
 * previous author's name over text they never wrote. That is a per-line falsehood, not a stale
 * view — the distinction `cmd::git::git_blame` draws — so a second `RangeSet`, `touched`, records
 * every line a `docChanged` transaction hit, and a touched line is painted with the uncommitted
 * tint, has its label blanked, and answers the hover as uncommitted.
 *
 * `touched` is a separate set rather than an edit of the markers precisely so that **undo can put
 * the attribution back**: the markers are never destroyed, only overridden, and an undo maps the
 * override away again. See [`blameState`]'s `update` for the one place that needs more than
 * `map()`.
 *
 * # Why the override is a CSS class and not a rewritten marker
 *
 * Rebuilding the marker set so that touched lines carry an uncommitted cell would be O(number of
 * lines in the file) on **every keystroke** — twenty thousand `GutterMarker`s rebuilt to change
 * one. Instead the touched set holds a marker with no `toDOM` and only an `elementClass`, which
 * `@codemirror/view`'s `GutterElement.setMarkers` merges into the line's class list at no cost,
 * and the tint and the blanked label are two rules in `EditorSurface.module.css`.
 */
import { createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { flushSync } from 'react-dom'
import {
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
  type EditorState,
  type Extension,
  type Range,
  type Text,
} from '@codemirror/state'
import {
  EditorView,
  GutterMarker,
  gutter,
  showTooltip,
  type BlockInfo,
  type Tooltip,
} from '@codemirror/view'
import {
  BLAME_HOVER_MS,
  BLAME_LABEL_CHARS,
  UNCOMMITTED_BUCKET,
  popupLines,
  type BlameMarker,
} from './blameModel'
import { BlamePopup } from './BlamePopup'

/**
 * Replace the column's contents, or clear it with `null`.
 *
 * Module-scope, because `EditorSurface.tsx` dispatches it and the extension that reads it is built
 * per pane. Clearing is not the same as removing the gutter from its compartment: reconfiguring
 * the compartment away takes the *column* out and leaves this field's value — and therefore a
 * hover card — standing behind it. Both have to happen, in that order.
 */
export const setBlame = StateEffect.define<BlameMarker[] | null>()

/** Show the card for one line, or hide it with `null`. Internal to this module. */
const setHover = StateEffect.define<HoverTarget | null>()

/** What the card is about. `oid: ''` means the line is uncommitted and has no actions. */
interface HoverTarget {
  readonly pos: number
  readonly oid: string
  readonly lines: readonly string[]
}

/** The card an uncommitted or freshly edited line gets, built once. */
const UNCOMMITTED_CARD = popupLines(null)

/**
 * How long the card survives the pointer leaving the gutter.
 *
 * It has to survive at all: the card is positioned over the *text*, so reaching its two buttons
 * means crossing out of the gutter, and a card that closes on `mouseleave` is a card whose buttons
 * can never be clicked. The card's own `mouseenter` cancels this, so the grace period only has to
 * cover the few pixels between the two.
 */
const HIDE_GRACE_MS = 240

class BlameCell extends GutterMarker {
  constructor(readonly marker: BlameMarker) {
    super()
    this.elementClass =
      marker.bucket === UNCOMMITTED_BUCKET
        ? 'cm-blame-cell cm-blame-uncommitted'
        : `cm-blame-cell cm-blame-age-${marker.bucket}`
  }

  /**
   * Compared per line on every gutter update, so it is a field-by-field test and not a deep one.
   * `line` is deliberately absent: the marker's *position* is the range set's business, and
   * including it would make every line below an inserted one compare unequal and re-render.
   */
  override eq(other: GutterMarker): boolean {
    return (
      other instanceof BlameCell
      && other.marker.label === this.marker.label
      && other.marker.bucket === this.marker.bucket
      && other.marker.oid === this.marker.oid
    )
  }

  override toDOM(): Node {
    const span = document.createElement('span')
    span.className = 'cm-blame-label'
    span.textContent = this.marker.label
    /*
     * No `title` attribute, deliberately. WebKitGTK would draw its own tooltip about a second
     * after ours appears, and the user would be looking at two cards saying the same thing in two
     * type faces. The text is on the marker (`BlameMarker.title`) and reaches the reader through
     * the card.
     */
    return span
  }
}

/**
 * The override marker. No `toDOM`, so it contributes only its class — see the header.
 */
class TouchedMarker extends GutterMarker {
  override elementClass = 'cm-blame-touched'
}

const TOUCHED = new TouchedMarker()

interface BlameSets {
  /** One entry per line of the file, at the line's start. */
  readonly cells: RangeSet<GutterMarker>
  /** The lines this editing session has changed. Usually tiny; empty on a fresh fetch. */
  readonly touched: RangeSet<GutterMarker>
}

const EMPTY: BlameSets = { cells: RangeSet.empty, touched: RangeSet.empty }

/**
 * The markers, and which lines the user has since edited.
 *
 * Module-scope so the gutter's `markers` callback and the per-pane tooltip field can both read it
 * without being handed a reference. A `StateField` and not a `ViewPlugin` because the value has to
 * survive a compartment reconfigure and has to map through changes, which is what fields do and
 * plugins do not.
 */
const blameState = StateField.define<BlameSets>({
  create: () => EMPTY,
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setBlame)) {
        /*
         * A fresh answer clears `touched` on purpose: the fetch was made against *this* buffer —
         * the store hands Rust the dirty text through `registerDirtyBuffer` — so the runs already
         * account for every edit made so far, and an override on top would count them twice.
         *
         * `tr.newDoc` and not `tr.state.doc`: the new state is only half-built while a field's own
         * `update` is running, and the document is the one part of it available under a name that
         * says so.
         */
        return effect.value === null
          ? EMPTY
          : { cells: build(tr.newDoc, effect.value), touched: RangeSet.empty }
      }
    }
    if (!tr.docChanged) return value

    const cells = value.cells.map(tr.changes)
    let touched = value.touched.map(tr.changes)

    /*
     * **Undo removes overrides; everything else adds them.**
     *
     * Mapping alone does not restore an attribution, and the difference is worth spelling out. An
     * undo *is* a `docChanged` transaction, so the plain path below would mark the very lines the
     * undo just restored — the user presses Ctrl+Z, the text comes back, and the column stays
     * blank for ever. Point ranges map with `MapMode.Simple`, so the overrides also survive the
     * deletion rather than being dropped by it.
     *
     * So an undo filters its changed lines *out* of the set instead. This is an approximation in
     * exactly one direction: undoing one of two edits made to the same line un-touches a line that
     * is still modified, and the column credits it again. That is the cheap error — the expensive
     * one is a line that can never get its author back, which is what the alternative does on
     * every undo.
     */
    const undo = tr.isUserEvent('undo')
    /*
     * The start offsets of every line this transaction wrote to, ascending.
     *
     * A `Set`, because two changed ranges in one transaction can land on the same line — a
     * multi-cursor edit is the ordinary case — and `RangeSet.update` would happily store the
     * override twice. Insertion order is already ascending: `iterChangedRanges` yields its ranges
     * in order and the inner loop walks lines upward, which is what `add` below needs.
     */
    const hit = new Set<number>()
    tr.changes.iterChangedRanges((_fromA, _toA, fromB, toB) => {
      // `fromB`/`toB` are positions in the NEW document, which is what `tr.newDoc` is.
      const first = tr.newDoc.lineAt(fromB)
      const last = toB <= first.to ? first : tr.newDoc.lineAt(toB)
      // Bounded by the size of the change and not of the file: one typed character adds one
      // position, and a paste of ten thousand lines adds ten thousand — which is right, because
      // every one of those lines genuinely is uncommitted.
      for (let n = first.number; n <= last.number; n += 1) hit.add(tr.newDoc.line(n).from)
    })
    if (hit.size === 0) return { cells, touched }

    if (undo) {
      // Unbounded `filter`, because `touched` holds the lines edited in this session and not the
      // lines of the file: bounding the scan would cost a min/max pass to save a walk of a set
      // that is normally single digits long.
      touched = touched.update({ filter: (from) => !hit.has(from) })
      return { cells, touched }
    }

    const add: Range<GutterMarker>[] = []
    for (const pos of hit) if (markerAt(touched, pos) === null) add.push(TOUCHED.range(pos))
    if (add.length > 0) touched = touched.update({ add, sort: true })
    return { cells, touched }
  },
})

/** The marker at exactly `pos`, or `null`. */
function markerAt(set: RangeSet<GutterMarker>, pos: number): GutterMarker | null {
  let found: GutterMarker | null = null
  const cursor = set.iter(pos)
  if (cursor.value !== null && cursor.from === pos) found = cursor.value
  return found
}

/** One `BlameCell` per line, at the line's start offset. */
function build(doc: Text, markers: readonly BlameMarker[]): RangeSet<GutterMarker> {
  const builder = new RangeSetBuilder<GutterMarker>()
  const total = doc.lines
  for (const marker of markers) {
    // `collapseRuns` covers exactly `[1, file.lines]`, but the buffer may already have been edited
    // between the fetch going out and the answer coming back, so the tail is clipped rather than
    // thrown: `RangeSetBuilder.add` past the end of the document throws, and an exception here
    // escapes a `dispatch` inside a React effect with no boundary above it.
    if (!Number.isInteger(marker.line) || marker.line < 1 || marker.line > total) continue
    const at = doc.line(marker.line).from
    builder.add(at, at, new BlameCell(marker))
  }
  return builder.finish()
}

/** What the card for `line` should say, or `null` when there is nothing to say. */
function targetAt(state: EditorState, line: number): HoverTarget | null {
  const sets = state.field(blameState, false)
  if (sets === undefined) return null
  if (line < 1 || line > state.doc.lines) return null
  const pos = state.doc.line(line).from
  const cell = markerAt(sets.cells, pos)
  if (!(cell instanceof BlameCell)) return null
  // The override beats the run, here as well as in the paint — a card that named an author for a
  // line the column is drawing as uncommitted would be the two halves disagreeing in the one
  // place a user looks to resolve exactly that.
  if (cell.marker.oid === '' || markerAt(sets.touched, pos) !== null) {
    return { pos, oid: '', lines: UNCOMMITTED_CARD }
  }
  return { pos, oid: cell.marker.oid, lines: cell.marker.title.split('\n') }
}

export interface BlameOptions {
  /** The user clicked a cell, or the card's *Show in log*. Never called with an empty oid. */
  onShowCommit: (oid: string) => void
  /**
   * The card's *Annotate previous revision*. Never called with an empty oid.
   *
   * Optional, and the card omits the button when it is absent — see `BlamePopup`'s prop for why
   * that is the right shape rather than a disabled control. Presence is decided when the
   * extension is built, which is once per surface, so this is not a value that can go stale
   * between the hover and the click.
   */
  onAnnotateParent?: ((oid: string) => void) | undefined
}

/**
 * The column, the card and the four events between them.
 *
 * Built **once per editor surface** and held in a `useMemo`, not rebuilt per render: the value is
 * what `Compartment.reconfigure` is given, and handing it a fresh object would tear the gutter's
 * DOM down and lose the tooltip on every paint. The callbacks are therefore read from the object
 * this closure captured, which `EditorSurface` keeps pointing at refs for the same reason every
 * other callback in that file is a ref.
 */
export function blameExtension(opts: BlameOptions): Extension {
  /** Pending *show*. Cleared when the pointer moves to another line or leaves. */
  let hoverTimer: ReturnType<typeof setTimeout> | undefined
  /** Pending *hide*. See [`HIDE_GRACE_MS`]. */
  let hideTimer: ReturnType<typeof setTimeout> | undefined
  /** The line the pending show is for, so a `mousemove` inside one cell does not restart it. */
  let pending = -1

  const cancelHide = (): void => {
    if (hideTimer !== undefined) clearTimeout(hideTimer)
    hideTimer = undefined
  }

  const hideNow = (view: EditorView): void => {
    cancelHide()
    if (view.state.field(hover, false) != null) view.dispatch({ effects: setHover.of(null) })
  }

  const hideSoon = (view: EditorView): void => {
    cancelHide()
    hideTimer = setTimeout(() => {
      hideTimer = undefined
      hideNow(view)
    }, HIDE_GRACE_MS)
  }

  /**
   * The tooltip, as a field so it survives scrolling and so `showTooltip` can read it.
   *
   * Per call rather than module-scope because `create` closes over `opts`, and two panes over the
   * same file have two different `onShowCommit`s. A new field per `blameExtension()` is safe
   * precisely because the extension is memoised: the field is created once per surface, and a
   * compartment reconfigure that hands back the same extension value keeps the field's value.
   */
  const hover = StateField.define<Tooltip | null>({
    create: () => null,
    update(value, tr) {
      for (const effect of tr.effects) {
        if (effect.is(setHover)) return effect.value === null ? null : card(effect.value)
        // A new answer invalidates the card that was drawn from the old one.
        if (effect.is(setBlame)) return null
      }
      // Typing dismisses it. The alternative is mapping `pos` through the change and leaving a
      // card describing a line the user is in the middle of rewriting.
      if (tr.docChanged) return null
      return value
    },
    provide: (field) => showTooltip.from(field),
  })

  const card = (target: HoverTarget): Tooltip => ({
    pos: target.pos,
    above: false,
    create: (view) => {
      const dom = document.createElement('div')
      dom.className = 'cm-blame-popup'
      const root = createRoot(dom)
      /*
       * `flushSync`, because CodeMirror measures this element in the same frame it is created and
       * `root.render` is scheduled rather than synchronous. Without it the tooltip is positioned
       * against an empty box and lands in the wrong place — visibly so near the bottom of the
       * pane, where the flip-above decision is made from the height it does not yet have.
       *
       * Safe here specifically: `create` is reached from the gutter's own DOM listener, never from
       * inside a React render or effect, which is the situation `flushSync` warns about.
       */
      flushSync(() => {
        root.render(
          createElement(BlamePopup, {
            oid: target.oid,
            lines: target.lines,
            onShowCommit: () => {
              hideNow(view)
              opts.onShowCommit(target.oid)
            },
            ...(opts.onAnnotateParent === undefined
              ? {}
              : {
                  onAnnotateParent: () => {
                    hideNow(view)
                    opts.onAnnotateParent?.(target.oid)
                  },
                }),
          }),
        )
      })
      /*
       * The card keeps itself open while the pointer is on it, and starts the grace period again
       * on the way out. Without the first of these the two buttons are unreachable: the card is
       * positioned over the *text*, so getting to it means leaving the gutter.
       */
      const leave = (): void => hideSoon(view)
      dom.addEventListener('mouseenter', cancelHide)
      dom.addEventListener('mouseleave', leave)
      return {
        dom,
        destroy: () => {
          dom.removeEventListener('mouseenter', cancelHide)
          dom.removeEventListener('mouseleave', leave)
          /*
           * Deferred. `destroy` can be reached from a dispatch made inside a React effect — the
           * one in `EditorSurface` that pushes new markers — and unmounting a root while React is
           * flushing is the warning React 19 raises and then recovers from badly. The container is
           * already detached by then, so a microtask later costs nothing.
           */
          queueMicrotask(() => root.unmount())
        },
      }
    },
  })

  const lineOf = (view: EditorView, block: BlockInfo): number =>
    view.state.doc.lineAt(block.from).number

  return [
    blameState,
    hover,
    gutter({
      class: 'cm-blame',
      markers: (view) => {
        const sets = view.state.field(blameState, false)
        // Both sets, merged per line by `RangeSet.iter` inside the gutter plugin. The touched one
        // carries no DOM, so the line gets one cell and two classes.
        return sets === undefined ? RangeSet.empty : [sets.cells, sets.touched]
      },
      /*
       * A widest-case cell, so the column has its full width from the first frame and does not
       * jitter as the viewport scrolls past a run whose author has a shorter name. `M` because it
       * is the widest glyph in a proportional face and identical to every other in a monospace
       * one, which is what this column is set in.
       */
      initialSpacer: () =>
        new BlameCell({
          line: 0,
          oid: '',
          label: 'M'.repeat(BLAME_LABEL_CHARS),
          bucket: 0,
          title: '',
        }),
      domEventHandlers: {
        mousemove: (view, block) => {
          // The pointer is back over the column, so a hide scheduled by a `mouseleave` on the way
          // to the card is cancelled.
          cancelHide()
          const line = lineOf(view, block)
          if (line === pending) return false
          pending = line
          /*
           * Already showing this line's card — which is the state after a trip out to the card and
           * back. Re-dispatching would build a *new* `Tooltip` object, and CodeMirror keys its
           * views on that identity, so the card would be destroyed and rebuilt under the pointer.
           */
          const shown = view.state.field(hover, false)
          if (shown != null && shown.pos === view.state.doc.line(line).from) return false
          if (hoverTimer !== undefined) clearTimeout(hoverTimer)
          hoverTimer = setTimeout(() => {
            hoverTimer = undefined
            const target = targetAt(view.state, line)
            // No guard for a view destroyed inside the delay: `EditorView.update` returns early
            // on a destroyed view rather than throwing (`@codemirror/view` L7957), so a pane
            // closed 200 ms into a hover costs one no-op dispatch.
            view.dispatch({ effects: setHover.of(target) })
          }, BLAME_HOVER_MS)
          return false
        },
        mouseleave: (view) => {
          pending = -1
          if (hoverTimer !== undefined) clearTimeout(hoverTimer)
          hoverTimer = undefined
          hideSoon(view)
          return false
        },
        click: (view, block) => {
          const target = targetAt(view.state, lineOf(view, block))
          // An uncommitted line is not a click target: there is no commit to show, and a click
          // that silently does nothing is the failure this project has paid for repeatedly.
          if (target === null || target.oid === '') return false
          hideNow(view)
          opts.onShowCommit(target.oid)
          return true
        },
      },
    }),
  ]
}
