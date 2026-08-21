/**
 * Painting conflict regions into a CodeMirror pane. (M20)
 *
 * Split out of `MergePane.tsx` because it is the only part of the resolver that has to speak
 * CodeMirror, and keeping it here leaves `panes/mergeModel.ts` import-free — that module is
 * compiled on its own by `check:merge`, and a `@codemirror/view` import in it would end that.
 *
 * # Why a `StateField` and not `EditorView.decorations.of(set)`
 *
 * A plain set goes stale the moment the document changes and then points past the end of it,
 * which CodeMirror answers with a thrown range error rather than a wrong colour. A field maps
 * its ranges through every `ChangeSet`, so an edit in the centre pane moves the highlights with
 * the text instead of invalidating them. Replacing the set is an effect, which is what lets the
 * spans be recomputed from `mergeModel` and pushed in.
 */
import { StateEffect, StateField } from '@codemirror/state'
import { Decoration, EditorView, type DecorationSet } from '@codemirror/view'

/** One region to paint: which lines, and which of the resolver's three tones. */
export interface Painted {
  /** Zero-based, end-exclusive — `mergeModel`'s `Span`. */
  from: number
  to: number
  tone: 'conflict' | 'pending'
  /** The one the toolbar is currently on, drawn brighter so stepping is visible. */
  current: boolean
}

/**
 * The two tones, as class names.
 *
 * Only two, because a highlight means *work you have not done*: an answered block is not painted
 * at all, in any pane. See `mergeModel.regionTone`.
 *
 * The colours live in `MergePane.module.css` beside everything else this pane draws; these names
 * are the whole of the coupling, and they are `:global` there because CodeMirror writes them
 * onto its own DOM, where a CSS module's hashed names cannot reach.
 */
const TONE: Record<Painted['tone'], string> = {
  conflict: 'cm-mergeConflict',
  pending: 'cm-mergePending',
}

/** One run of characters inside a line that actually differs. */
export interface Marked {
  line: number
  from: number
  to: number
}

/** Line and inline decorations, already resolved to document offsets. */
const setPainted = StateEffect.define<DecorationSet>()

export const paintedField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setPainted)) return effect.value
    }
    // Mapped rather than dropped, so a keystroke in the centre pane moves the highlight with the
    // line it is on instead of leaving it behind or throwing a range error.
    return deco.map(tr.changes)
  },
  provide: (field) => EditorView.decorations.from(field),
})

/**
 * Push a new set of regions into a pane.
 *
 * The line-to-offset conversion happens here, against the document the view actually holds,
 * which is what keeps the field's ranges in bounds no matter what the model believed.
 */
const WORD = Decoration.mark({ class: 'cm-mergeWord' })

export function paint(
  view: EditorView,
  painted: readonly Painted[],
  marked: readonly Marked[] = [],
): void {
  const doc = view.state.doc
  const clamp = (line: number) => Math.min(Math.max(line + 1, 1), doc.lines)
  const ranges: { at: number; value: Decoration }[] = []
  /*
   * Mark decorations are collected separately because they must be sorted with the line
   * decorations into one set, and a `Decoration.line` at the same offset must come *first* —
   * `Decoration.set(_, true)` sorts by position but not by kind, and a mark before a line at the
   * same offset is the one ordering CodeMirror rejects outright.
   */
  const marks: { at: number; to: number }[] = []
  for (const run of marked) {
    const line = doc.line(clamp(run.line))
    const from = Math.min(line.from + run.from, line.to)
    // A zero-width run is a pure insertion at a join; widened by one so it can be seen at all.
    const to = Math.min(line.from + Math.max(run.to, run.from + 1), line.to)
    if (to > from) marks.push({ at: from, to })
  }

  for (const span of painted) {
    const cls = `${TONE[span.tone]}${span.current ? ' cm-mergeCurrent' : ''}`
    const value = Decoration.line({ class: cls })
    /*
     * A region that occupies no lines still gets one marked. `from === to` is *take neither* —
     * the region was deleted — and painting nothing would make that decision invisible, so the
     * line that closed over the gap carries the mark instead.
     */
    const first = clamp(span.from)
    const last = Math.max(clamp(span.to - 1), first)
    for (let line = first; line <= last; line += 1) {
      ranges.push({ at: doc.line(line).from, value })
    }
  }

  // `Decoration.set` requires document order, and a dropped region can share its line with the
  // region after it, so the list is not sorted by construction.
  ranges.sort((a, b) => a.at - b.at)
  const all = [
    ...ranges.map((r) => r.value.range(r.at)),
    ...marks.map((m) => WORD.range(m.at, m.to)),
  ]
  view.dispatch({ effects: setPainted.of(Decoration.set(all, true)) })
}
