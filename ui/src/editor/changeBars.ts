/**
 * The change column: IDEA's markers for what this buffer changed against HEAD. (M35)
 *
 * A bar beside every added or rewritten line, a caret where lines were removed, and a card on
 * click carrying the original text, **Revert** and **Copy**. `editor/changeModel.ts` decides
 * everything that can be decided over plain values; this file is the part that genuinely needs a
 * `EditorView`.
 *
 * # It is a module-level constant, not a factory, and that is worth reading before "fixing" it
 *
 * `blame.ts` builds its extension through `blameExtension(opts)` and `EditorSurface` memoises the
 * result, because its tooltip field closes over `onShowCommit`/`onAnnotateParent` and a fresh
 * extension value handed to `Compartment.reconfigure` tears the gutter's DOM down and takes the
 * open card with it. This column takes **no host callbacks at all** — Revert is a `view.dispatch`
 * and Copy is `editor/clipboard.ts` — so it is one value built once for the process, in no
 * compartment, and that whole hazard is absent rather than defended against. The next reader will
 * pattern-match on blame and reintroduce the factory; this paragraph is why not to.
 *
 * # Three range sets, and each earns its place
 *
 * * `bars` — one **point per changed line**, at the line's start. It has to be per line, because
 *   `@codemirror/view` matches a gutter marker to a line by comparing the range's `from` against
 *   the line start: a single range spanning six lines paints one of them.
 * * `carets` — the deletions, separately, because **one line can carry both**. A deletion
 *   immediately followed by an insertion is two blocks whose markers land on the same line, and
 *   a single set would silently drop one of them. `markers` returns both and the gutter merges
 *   them per line, which is the mechanism `blame.ts` uses for its `touched` overrides — the same
 *   machinery for a different reason.
 * * `blocks` — **real ranges**, never rendered, read by the card and by Revert. Mapped through
 *   `tr.changes` like the others, so typing inside a block grows its range and the revert
 *   replaces whatever is there *now*, which is the correct meaning of "put this block back". A
 *   scan over the per-line point set could not do it: an insertion inside a block leaves the new
 *   lines unmarked until the next recompute, so a contiguity scan would stop early and revert
 *   half of it.
 *
 * # No `touched` overlay, and no undo branch
 *
 * Blame needs both because it cannot re-derive locally: its answer only comes from a round trip,
 * so a marker that survives an edit keeps another person's name over text they never wrote, for
 * ever, and an undo — itself a `docChanged` transaction — would blank the column permanently
 * without an explicit branch. This column re-derives from scratch every
 * {@link CHANGE_RECOMPUTE_MS} milliseconds, locally, for free. The worst staleness available is a
 * bar that is briefly the wrong colour, self-correcting, and an undo is just another edit. An
 * overlay here would also be *wrong*: "the user typed here" is not "this differs from HEAD", and
 * typing a line back to its committed text must **remove** a bar, which an additive set cannot
 * express.
 */
import { RangeSet, RangeSetBuilder, StateEffect, StateField } from '@codemirror/state'
import type { EditorState, Extension, Range, Text } from '@codemirror/state'
import { EditorView, GutterMarker, ViewPlugin, gutter, showTooltip } from '@codemirror/view'
import type { Tooltip, ViewUpdate } from '@codemirror/view'
import { createRoot } from 'react-dom/client'
import { createElement } from 'react'
import { flushSync } from 'react-dom'
import { diffLines } from '@/panes/mergeModel'
import {
  CHANGE_RECOMPUTE_MS,
  changeBlocks,
  popupContent,
  revertPlan,
  type ChangeBlock,
} from './changeModel'
import { ChangePopup } from './ChangePopup'
import { copyUnavailable, writeClipboard } from './clipboard'

/**
 * The HEAD lines this buffer is measured against, or `null` for "no answer, paint nothing".
 *
 * Pushed by `EditorSurface`'s effect from `changeBaseline`. The extension holds it in its own
 * field rather than reading the store, so nothing in this file imports IPC and the whole column
 * is a function of what it was handed.
 */
export const setChangeBaseline = StateEffect.define<readonly string[] | null>()

/** The recomputed blocks, dispatched by the plugin's own timer. */
const setBlocks = StateEffect.define<readonly ChangeBlock[]>()

/** Which block's card is open, by its position in the current block list. */
const setCard = StateEffect.define<number | null>()

/**
 * The open card, and **which block it is about**.
 *
 * The index is carried beside the tooltip so a click on a *different* marker switches cards
 * instead of closing the open one — a toggle keyed only on "is something showing" makes the
 * second marker in a file take two clicks, which reads as the first click having missed.
 */
interface OpenCard {
  readonly tooltip: Tooltip
  readonly index: number
}

/**
 * A bar, or a caret. Class-only: no `toDOM`, so it contributes its `elementClass` and nothing
 * else, and the column costs zero DOM nodes beyond the gutter elements CodeMirror makes anyway.
 */
class ChangeMarker extends GutterMarker {
  /**
   * A field and not a getter: `GutterMarker` declares `elementClass` as a property, and
   * TypeScript refuses an accessor over one (TS2611). `blame.ts`'s `TouchedMarker` writes it the
   * same way.
   */
  override elementClass: string

  constructor(cls: string) {
    super()
    this.elementClass = cls
  }

  /**
   * Compared on the class, which is everything that decides how the cell looks. Position is not
   * part of it — a `RangeSet` holds that — so the four constants below are shared by every line
   * they mark and the gutter re-renders nothing when a marker merely moves.
   */
  override eq(other: GutterMarker): boolean {
    return other instanceof ChangeMarker && other.elementClass === this.elementClass
  }
}

const ADDED = new ChangeMarker('cm-change-add')
const MODIFIED = new ChangeMarker('cm-change-mod')
const DELETED = new ChangeMarker('cm-change-del')
/** A deletion past the last line: the caret draws on the bottom edge instead of the top. */
const DELETED_END = new ChangeMarker('cm-change-del cm-change-del-end')

/** A block, as a value carried by a mapped range. Never rendered. */
class BlockValue extends GutterMarker {
  constructor(readonly block: ChangeBlock) {
    super()
  }
}

interface ChangeSets {
  readonly baseline: readonly string[] | null
  readonly bars: RangeSet<GutterMarker>
  readonly carets: RangeSet<GutterMarker>
  readonly blocks: RangeSet<BlockValue>
  /**
   * The blocks in document order, as the last recompute produced them.
   *
   * Held beside `blocks` because a `RangeSet` cannot be indexed, and the card names its block by
   * index — an index into an array that only changes when `setBlocks` lands, so it cannot drift
   * under an open card the way a position would.
   */
  readonly list: readonly ChangeBlock[]
}

const EMPTY: ChangeSets = {
  baseline: null,
  bars: RangeSet.empty,
  carets: RangeSet.empty,
  blocks: RangeSet.empty,
  list: [],
}

const changeState = StateField.define<ChangeSets>({
  create: () => EMPTY,
  update(value, tr) {
    let next = value
    for (const effect of tr.effects) {
      if (effect.is(setChangeBaseline)) {
        // A new baseline invalidates every block: they were computed against the old one. The
        // plugin's timer fills them back in on its next tick, and until then the column is empty
        // rather than wrong.
        next = { ...EMPTY, baseline: effect.value }
      } else if (effect.is(setBlocks)) {
        next = { ...next, ...build(tr.newDoc, effect.value) }
      }
    }
    if (next !== value) return next
    if (!tr.docChanged) return value
    return {
      ...value,
      bars: value.bars.map(tr.changes),
      carets: value.carets.map(tr.changes),
      blocks: value.blocks.map(tr.changes),
    }
  },
})

/**
 * The three sets, from one list of blocks.
 *
 * `tr.newDoc` and not `tr.state.doc`: inside a field's own `update` the new state is only half
 * built, and the document is the one part of it available under a name that says so.
 *
 * Every line is clipped into `[1, doc.lines]` and a block outside it is dropped rather than
 * clamped. `RangeSetBuilder.add` past the end of the document **throws**, and an exception here
 * escapes a `dispatch` made inside a React effect with no boundary above it — the failure
 * `blame.ts`'s own `build` records. A clamp would be worse than the drop besides: it puts a
 * confident marker on a line nothing happened to.
 */
function build(doc: Text, blocks: readonly ChangeBlock[]): Omit<ChangeSets, 'baseline'> {
  const bars = new RangeSetBuilder<GutterMarker>()
  const carets: Range<GutterMarker>[] = []
  const spans: Range<BlockValue>[] = []
  const list: ChangeBlock[] = []
  for (const block of blocks) {
    if (block.firstLine < 1 || block.firstLine > doc.lines) continue
    const first = doc.line(block.firstLine)
    if (block.kind === 'deleted') {
      carets.push((block.atEnd ? DELETED_END : DELETED).range(first.from))
      spans.push(new BlockValue(block).range(first.from, first.from))
      list.push(block)
      continue
    }
    if (block.lastLine < block.firstLine || block.lastLine > doc.lines) continue
    const marker = block.kind === 'added' ? ADDED : MODIFIED
    for (let n = block.firstLine; n <= block.lastLine; n += 1) {
      bars.add(doc.line(n).from, doc.line(n).from, marker)
    }
    spans.push(new BlockValue(block).range(first.from, doc.line(block.lastLine).to))
    list.push(block)
  }
  return {
    bars: bars.finish(),
    // `sort: true` because the carets are collected in block order, which is document order for
    // everything `changeBlocks` emits — but a caret and a bar can share a line, and RangeSet
    // refuses an unsorted `add` rather than silently reordering.
    carets: RangeSet.of(carets, true),
    blocks: RangeSet.of(spans, true),
    list,
  }
}

/** The block at a document position, and its index in the current list. */
function blockAt(state: EditorState, pos: number): { block: ChangeBlock; index: number } | null {
  const sets = state.field(changeState, false)
  if (sets === undefined) return null
  let found: ChangeBlock | null = null
  const cursor = sets.blocks.iter(pos)
  while (cursor.value !== null) {
    if (cursor.from > pos) break
    if (pos >= cursor.from && pos <= cursor.to) {
      found = cursor.value.block
      break
    }
    cursor.next()
  }
  if (found === null) return null
  const index = sets.list.indexOf(found)
  return index === -1 ? null : { block: found, index }
}

/**
 * A fingerprint of the emitted blocks, so an unchanged recompute dispatches nothing.
 *
 * Five ticks a second of an identical answer would still cost a field update and — the part that
 * matters — would close the card through [`cardField`]'s `setBlocks` branch, so a user reading
 * one could never reach its Revert button. `EditorSurface`'s lint effect keeps a fingerprint for
 * a related reason.
 */
function fingerprint(blocks: readonly ChangeBlock[]): string {
  return blocks.map((b) => `${b.kind}:${b.firstLine}:${b.lastLine}:${b.baseLines.length}`).join('|')
}

/**
 * The recompute timer.
 *
 * A **trailing throttle** — see `changeModel.CHANGE_RECOMPUTE_MS`. It lives here and not in the
 * store because two panes over one file are two independent documents that would fight over one
 * timer, and not in a React effect because there is no per-keystroke prop to depend on: the
 * buffer deliberately never re-renders `EditorSurface` while you type, and the build effect's
 * `[path, reloadKey]` array is pinned by two checks precisely to keep it that way.
 */
const recompute = ViewPlugin.fromClass(
  class {
    private timer: ReturnType<typeof setTimeout> | undefined
    private last = ''

    constructor(private readonly view: EditorView) {
      this.arm()
    }

    update(update: ViewUpdate): void {
      if (update.docChanged) {
        this.arm()
        return
      }
      for (const tr of update.transactions) {
        for (const effect of tr.effects) {
          if (effect.is(setChangeBaseline)) {
            // A new baseline is a different question, so the throttle's "already armed" guard
            // must not swallow it: reset the fingerprint or an answer identical to the previous
            // baseline's would be skipped.
            this.last = ''
            this.arm()
            return
          }
        }
      }
    }

    destroy(): void {
      if (this.timer !== undefined) clearTimeout(this.timer)
    }

    /** Trailing throttle: the first call starts the window, later ones inside it do nothing. */
    private arm(): void {
      if (this.timer !== undefined) return
      this.timer = setTimeout(() => {
        this.timer = undefined
        this.run()
      }, CHANGE_RECOMPUTE_MS)
    }

    private run(): void {
      const sets = this.view.state.field(changeState, false)
      const baseline = sets?.baseline ?? null
      if (baseline === null) {
        this.last = ''
        return
      }
      // `iterLines`, never `doc.toString().split('\n')`: the string form materialises the whole
      // document as one allocation before splitting it, which on a 500 KB file is 500 KB of
      // garbage per tick for a result the array form produces directly.
      const buffer: string[] = []
      for (const line of this.view.state.doc.iterLines()) buffer.push(line)
      let blocks: readonly ChangeBlock[]
      try {
        blocks = changeBlocks(baseline, buffer, diffLines(baseline, buffer))
      } catch (error) {
        // A diff that throws must not take the window with it: there is no error boundary
        // between this timer and the React root.
        console.error('[cide] the change markers could not be computed', error)
        return
      }
      const mark = fingerprint(blocks)
      if (mark === this.last) return
      this.last = mark
      this.view.dispatch({ effects: setBlocks.of(blocks) })
    }
  },
)

/**
 * The card.
 *
 * `showTooltip.from(a field)` and **not** `hoverTooltip`, which is not a style choice: that
 * helper installs its listeners on `contentDOM`, and the gutter is not part of `contentDOM`, so
 * it never sees a pointer over the margin at all. `blame.ts` records the same finding.
 */
const cardField = StateField.define<OpenCard | null>({
  create: () => null,
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setCard)) {
        if (effect.value === null) return null
        const tooltip = card(tr.state, effect.value)
        return tooltip === null ? null : { tooltip, index: effect.value }
      }
      // A recompute or a new baseline replaces the block the card was describing.
      if (effect.is(setBlocks) || effect.is(setChangeBaseline)) return null
    }
    // Typing dismisses it — including the revert's own transaction. The alternative is mapping
    // `pos` through the change and leaving a card describing a block that has just moved.
    if (tr.docChanged) return null
    return value
  },
  provide: (field) => showTooltip.from(field, (open) => open?.tooltip ?? null),
})

function card(state: EditorState, index: number): Tooltip | null {
  const sets = state.field(changeState, false)
  const block = sets?.list[index]
  if (sets === undefined || block === undefined) return null
  if (block.firstLine < 1 || block.firstLine > state.doc.lines) return null
  const pos = state.doc.line(block.firstLine).from
  const content = popupContent(block)
  return {
    pos,
    above: false,
    create: (view) => {
      const dom = document.createElement('div')
      dom.className = 'cm-change-popup'
      const root = createRoot(dom)
      /*
       * `flushSync`, for `blame.ts`'s reason: CodeMirror measures this element in the same frame
       * it is created and `root.render` is scheduled rather than synchronous, so without it the
       * card is positioned against an empty box — visibly wrong near the foot of a pane, where
       * the flip-above decision is made from a height it does not yet have. Safe here
       * specifically because `create` is reached from the gutter's own DOM listener and never
       * from inside a React render.
       */
      flushSync(() => {
        root.render(
          createElement(ChangePopup, {
            heading: content.heading,
            lines: content.lines,
            hiddenLines: content.hiddenLines,
            // Omitted, not disabled, when the buffer cannot take the edit — `BlamePopup`'s rule
            // for `onAnnotateParent`, and the reason is the same: a control that reports a
            // refusal the user could have been spared is worse than no control.
            ...(view.state.readOnly
              ? {}
              : {
                  onRevert: () => {
                    revert(view, index)
                  },
                }),
            ...(content.copyable && copyUnavailable() === null
              ? {
                  onCopy: () => {
                    void writeClipboard(block.baseLines.join('\n'))
                    hide(view)
                  },
                }
              : {}),
          }),
        )
      })
      return {
        dom,
        destroy: () => {
          // Deferred: `destroy` is reachable from a dispatch made inside a React effect, and
          // unmounting a root while React is flushing is the warning React 19 raises and then
          // recovers from badly. The container is already detached by then.
          queueMicrotask(() => root.unmount())
        },
      }
    },
  }
}

function hide(view: EditorView): void {
  if (view.state.field(cardField, false) != null) view.dispatch({ effects: setCard.of(null) })
}

/**
 * Put one block back.
 *
 * The plan comes from `changeModel.revertPlan` — its `eat` field carries the whole argument about
 * which line break a removal has to consume — and the only work here is translating line numbers
 * to offsets **against the live document**, re-reading the block from the mapped range set so an
 * edit made since the last recompute is included rather than skipped.
 *
 * The joined text is always `'\n'`. Joining with the file's own break shape would put literal
 * `\r` characters *inside* CodeMirror lines, and `restoreLineEndings` would then add the file's
 * ending on top of them on save — every reverted line ending `\r\r\n`. The buffer is LF from the
 * first frame and the revert stays entirely inside that world; `editor/lineEndings.ts` is the one
 * place the file's shape is put back.
 */
function revert(view: EditorView, index: number): void {
  try {
    const sets = view.state.field(changeState, false)
    const block = sets?.list[index]
    if (block === undefined) return
    const doc = view.state.doc
    const plan = revertPlan(block, doc.lines)
    const insertion = plan.toLine < plan.fromLine
    // One past the last line is a legal insertion point and not a legal `doc.line` argument: it
    // is the end of the document. See `revertPlan`'s deletion branch.
    const past = plan.fromLine > doc.lines
    let from = past ? doc.length : doc.line(Math.max(plan.fromLine, 1)).from
    let to = insertion ? from : doc.line(Math.min(Math.max(plan.toLine, 1), doc.lines)).to
    let text = plan.insert.join('\n')
    if (plan.eat === 'trailing') {
      if (text !== '') text = `${text}\n`
      else to = Math.min(doc.length, to + 1)
    } else if (plan.eat === 'leading') {
      if (text !== '') text = `\n${text}`
      else from = Math.max(0, from - 1)
    }
    view.dispatch({
      changes: { from, to, insert: text },
      // One undo step, and a named one — this is a document edit like any other, which is why it
      // needs no confirmation: the Git panel's `git_rollback` writes the disk and cannot be
      // undone, and Ctrl+Z puts this straight back.
      userEvent: 'revert',
      scrollIntoView: true,
    })
  } catch (error) {
    console.error('[cide] the change could not be reverted', error)
  }
}

/**
 * The column, the card and the timer.
 *
 * One value for the process — see the header. Placed last in `EditorSurface`'s extension array so
 * the strip sits against the text, where IDEA draws it.
 */
export const CHANGE_BARS: Extension = [
  changeState,
  cardField,
  recompute,
  gutter({
    class: 'cm-cide-changes',
    // Both sets, merged per line by the gutter plugin: a line can carry a bar *and* the caret of
    // a deletion that landed on it.
    markers: (view) => {
      const sets = view.state.field(changeState, false)
      return sets === undefined ? RangeSet.empty : [sets.bars, sets.carets]
    },
    domEventHandlers: {
      mousedown: (view, block, event) => {
        const line = view.state.doc.lineAt(block.from)
        const hit = blockAt(view.state, line.from)
        if (hit === null) {
          hide(view)
          return false
        }
        // Prevented: a click in a gutter otherwise moves the editor's selection first, which
        // scrolls the pane out from under the pointer — `mergeGutter.ts` records the same.
        event.preventDefault()
        // Same marker closes; a different one switches. See [`OpenCard`].
        const showing = view.state.field(cardField, false)
        const next = showing != null && showing.index === hit.index ? null : hit.index
        view.dispatch({ effects: setCard.of(next) })
        return true
      },
    },
  }),
  /*
   * Dismiss on a click in the text.
   *
   * # Why there is no "…unless the click was on a marker" guard here
   *
   * `EditorView.domEventHandlers` attaches to **`contentDOM`** (`InputState.ensureHandlers`), and
   * neither the gutter nor a tooltip is inside it: gutters are siblings of the content inside
   * `.cm-scroller`, and a tooltip is appended to `view.dom`. So this handler's scope is already
   * exactly the one wanted, and the two exclusions the defensive version of this function writes
   * by hand — not on a marker, not on the card — would both be dead code guarding against events
   * that never arrive.
   *
   * It is worth saying which fact is load-bearing, because the neighbouring one is not true: a
   * gutter's own `domEventHandlers` are a plain `addEventListener` on the gutter element and
   * returning `true` from one calls `preventDefault()` and **never** `stopPropagation()`. So the
   * click that opens the card really does keep bubbling — it is only the attachment point above
   * that keeps it away from this handler. Move this to `view.dom` and the card is opened and
   * closed by one gesture, which reads as the marker simply not being clickable.
   *
   * Not `Escape`: that key is already contested by the find bar and the completion popup, and
   * their ordering is pinned by `check:completion`.
   */
  EditorView.domEventHandlers({
    mousedown: (_event, view) => {
      hide(view)
      return false
    },
  }),
]
