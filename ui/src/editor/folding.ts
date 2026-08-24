/**
 * Code folding, wired to CodeMirror. (M19)
 *
 * Every *decision* is next door in `foldRanges.ts`, which imports nothing and is driven as a
 * truth table by `ui/scripts/check-editor.mjs`. What is left here is the part that genuinely
 * needs an `EditorState`: a cache with a document in it, the seven commands, and the two
 * seams the pane uses to remember what was folded. Same split as `viewTracker.ts` over
 * `position.ts`, and for the same reason — a fold that starts one line off is invisible in a
 * screenshot and obvious in an assertion.
 *
 * # Why not `foldKeymap`
 *
 * `@codemirror/language` ships one, and it is unusable here. `Ctrl-Shift-[` / `Ctrl-Shift-]`
 * are one modifier away from `defaultKeymap`'s `indentLess`/`indentMore`, and on macOS `⌘[`
 * and `⌘]` are already `navigate.back`/`navigate.forward`, pushed by
 * `cide_core::keymap::platform_layer`. More basically: the key gate in `ui/src/keys/gate.ts`
 * is a **window capture** listener, so any chord the Rust keymap binds never reaches a
 * CodeMirror keymap at all. Folding is bound in `cide-core::keymap` and dispatched through
 * `ui/src/keys/dispatch.ts` like every other command, which is also what puts it in the
 * palette, in Settings ▸ Keymap and in `keymap.json`.
 */
import { codeFolding, foldEffect, foldGutter, foldService, foldedRanges, unfoldEffect } from '@codemirror/language'
import { Facet, StateField, type EditorState, type Extension, type StateEffect, type Text } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'

import { enclosing, foldAtLine, scanFolds, within, type FoldRange, type FoldSpec } from './foldRanges'
import { iconElement } from '../icons/iconElement'

/** A plain `{from, to}`, which is what CodeMirror's fold effects carry. */
interface Span {
  readonly from: number
  readonly to: number
}

/**
 * Which language's rules this editor folds by.
 *
 * A facet rather than a closure captured by the field below, so there is exactly one
 * `foldMapField` in the process and `foldedStartLines`/`applyRememberedFolds` can find it on
 * any state without being handed the extension that built it.
 */
const foldSpecFacet = Facet.define<FoldSpec, FoldSpec>({
  combine: (values) => values[0] ?? {},
})

/**
 * The document's foldable ranges, computed at most once per document version — and only if
 * somebody asks.
 *
 * **Lazy on purpose.** `foldable()` is called once per *visible line*, so a viewport asks this
 * forty-odd times and all forty share one scan. Computing eagerly in `update()` would instead
 * pay for a full scan on every keystroke, including the overwhelming majority that nothing ever
 * queries — a buffer being typed into with the gutter scrolled out of view, an editor in a
 * hidden tab receiving a `docSync` echo. `scanFolds` is linear and capped at
 * `FOLD_LINE_LIMIT` lines, which is what makes the worst case bounded; this is what makes the
 * common case free.
 */
class FoldMap {
  private cached: readonly FoldRange[] | null = null

  constructor(
    private readonly doc: Text,
    readonly spec: FoldSpec,
  ) {}

  ranges(): readonly FoldRange[] {
    if (this.cached === null) this.cached = scanFolds(this.doc, this.spec)
    return this.cached
  }
}

const foldMapField = StateField.define<FoldMap>({
  create: (state) => new FoldMap(state.doc, state.facet(foldSpecFacet)),
  // Only `docChanged`. A fold, an unfold and a selection move all leave the ranges exactly as
  // they were, and rebuilding here would throw the cache away on the very transactions folding
  // produces.
  //
  // The facet is re-read rather than carried over from the old value. It cannot change today —
  // `EditorSurface` rebuilds the whole view when the path does — but a `Compartment` holding this
  // extension is the obvious next step (that is how the language and the lint gutter are already
  // configured), and a stale spec would leave a buffer folding by the previous file's rules with
  // nothing on screen to say so.
  update: (value, tr) =>
    tr.docChanged ? new FoldMap(tr.state.doc, tr.state.facet(foldSpecFacet)) : value,
})

/**
 * Everything an editor needs to fold, for one language.
 *
 * Placed in `EditorSurface`'s extension array **after `lineNumbers()`**: gutters are laid out in
 * extension order, so that position is what puts the fold column between the numbers and the
 * text, where IDEA has it. `blame.ts` carries the mirror image of this note for why it goes
 * first.
 *
 * `foldGutter()` pulls in `codeFolding()` itself; the explicit call is here only to hand it a
 * `placeholderDOM`, and its own extension is deduplicated by CodeMirror.
 */
export function foldExtensions(spec: FoldSpec): Extension {
  return [
    foldSpecFacet.of(spec),
    foldMapField,
    foldService.of((state, lineStart) => {
      const map = state.field(foldMapField, false)
      if (map === undefined) return null
      const range = foldAtLine(map.ranges(), state.doc.lineAt(lineStart).number)
      return range === null ? null : { from: range.from, to: range.to }
    }),
    codeFolding({ placeholderDOM: placeholder }),
    foldGutter({ markerDOM: marker }),
  ]
}

/**
 * The `…` a collapsed block leaves behind.
 *
 * Ours rather than CodeMirror's because its base theme spells the placeholder's colours as
 * `#eee`, `#ddd` and `#888` — three literals that survive a theme switch and read as a light
 * chip in a dark buffer. That is the same defect the `.cm-lint-marker` overrides in
 * `EditorSurface.module.css` exist to fix, and `check:theme` is the gate for it.
 *
 * The `onclick` handed in is CodeMirror's own unfold, and it is attached rather than replaced:
 * clicking the placeholder is how a person expands a block they can see, and it is the only
 * mouse gesture for expanding one whose gutter marker has scrolled away.
 */
function placeholder(_view: EditorView, onclick: (event: Event) => void): HTMLElement {
  const element = document.createElement('span')
  element.className = 'cm-foldPlaceholder'
  element.append(iconElement('ellipsis', 1))
  element.title = 'Expand'
  element.setAttribute('aria-label', 'folded code')
  element.onclick = onclick
  return element
}

/** The gutter chevron. Drawn, not CodeMirror's SVG, and the same mark every other tree uses. */
function marker(open: boolean): HTMLElement {
  const element = document.createElement('span')
  element.className = 'cm-cide-foldMarker'
  element.dataset['open'] = open ? 'true' : 'false'
  element.append(iconElement(open ? 'chevron-down' : 'chevron-right', 1))
  element.title = open ? 'Collapse' : 'Expand'
  return element
}

/* -- the seven commands ------------------------------------------------------------------- */

/**
 * Collapse the innermost block at the caret.
 *
 * `foldAtLine` first and `enclosing` second, which is the order IDEA behaves in: the caret on a
 * `fn` line collapses that function, and the caret three lines into its body collapses it too.
 * A rule that only looked at the header line would make the commonest gesture — read a bit of a
 * function, decide to collapse it — do nothing.
 */
export function foldHere(view: EditorView): boolean {
  const target = targetRange(view.state)
  if (target === null) return false
  if (foldedSpanAt(view.state, target.from, target.to) !== null) return false
  view.dispatch({ effects: foldEffect.of({ from: target.from, to: target.to }) })
  return true
}

/** Expand the outermost collapsed block at the caret — one level, which is what is visible. */
export function unfoldHere(view: EditorView): boolean {
  const span = visibleFoldAt(view.state)
  if (span === null) return false
  view.dispatch({ effects: unfoldEffect.of(span) })
  return true
}

/** Collapse or expand, whichever the caret's block is not. */
export function toggleFoldHere(view: EditorView): boolean {
  return unfoldHere(view) || foldHere(view)
}

/**
 * Collapse every foldable range in the document, at every level.
 *
 * Not `@codemirror/language`'s `foldAll`, which folds **top-level** ranges only — after it, one
 * expand of the outermost block reveals every inner block already open. IDEA's Collapse All
 * collapses all of them, so expanding one level at a time works all the way down, and that is
 * the behaviour this reproduces.
 */
export function foldAllRanges(view: EditorView): boolean {
  const map = view.state.field(foldMapField, false)
  if (map === undefined) return false
  const effects = map
    .ranges()
    .filter((range) => foldedSpanAt(view.state, range.from, range.to) === null)
    .map((range) => foldEffect.of({ from: range.from, to: range.to }))
  return dispatchAll(view, effects)
}

/** Expand everything, at every level. */
export function unfoldAllRanges(view: EditorView): boolean {
  return dispatchAll(
    view,
    allFolded(view.state).map((span) => unfoldEffect.of(span)),
  )
}

/** Collapse the block at the caret and everything nested inside it. */
export function foldRecursive(view: EditorView): boolean {
  const map = view.state.field(foldMapField, false)
  const target = targetRange(view.state)
  if (map === undefined || target === null) return false
  const effects = within(map.ranges(), target)
    .filter((range) => foldedSpanAt(view.state, range.from, range.to) === null)
    .map((range) => foldEffect.of({ from: range.from, to: range.to }))
  return dispatchAll(view, effects)
}

/**
 * Expand the block at the caret and everything nested inside it.
 *
 * The span is taken from what is *folded* when something is, and from the scanner otherwise:
 * after Collapse all, the only thing the caret can see is the outermost placeholder, and the
 * ranges nested inside it are folded but invisible. Unfolding only what is visible would need
 * as many presses as the file has levels, which is the opposite of what "recursively" means.
 */
export function unfoldRecursive(view: EditorView): boolean {
  const span = visibleFoldAt(view.state) ?? targetRange(view.state)
  if (span === null) return false
  return dispatchAll(
    view,
    allFolded(view.state)
      .filter((folded) => folded.from >= span.from && folded.to <= span.to)
      .map((folded) => unfoldEffect.of(folded)),
  )
}

/* -- persistence -------------------------------------------------------------------------- */

/**
 * The 1-based start lines of everything currently folded, ascending.
 *
 * **Lines and not offsets**, and that is the whole design of fold persistence. A record is
 * written against one version of a file and applied to another — the agent rewrote it, a
 * `cargo fmt` shortened it, the user edited it in another editor. A stale line number simply
 * fails to name a foldable range and is skipped; a stale offset would fold a range of text
 * nobody chose. Same argument `position.ts` makes for storing a line rather than a `scrollTop`.
 */
export function foldedStartLines(state: EditorState): number[] {
  const doc = state.doc
  const lines = allFolded(state).map((span) => doc.lineAt(span.from).number)
  return [...new Set(lines)].sort((a, b) => a - b)
}

/**
 * The effects that restore `lines`, for a state that has just been created.
 *
 * Returned rather than dispatched so `EditorSurface` can put them in the **same transaction** as
 * the remembered scroll position. Two dispatches would lay the document out at its unfolded
 * heights, scroll to a line, and then fold — moving the user somewhere they never were, on
 * every single restore.
 *
 * A line naming no foldable range is dropped in silence. That is the common case after an edit
 * elsewhere and it is not an error.
 */
export function foldEffectsFor(state: EditorState, lines: readonly number[]): StateEffect<Span>[] {
  const map = state.field(foldMapField, false)
  if (map === undefined) return []
  const ranges = map.ranges()
  const effects: StateEffect<Span>[] = []
  for (const line of lines) {
    const range = foldAtLine(ranges, line)
    if (range !== null) effects.push(foldEffect.of({ from: range.from, to: range.to }))
  }
  return effects
}

/* -- what is possible here, for the menu ---------------------------------------------------- */

/*
 * The four predicates below exist because a context menu has to decide *before* the click
 * whether a row can do anything, and the commands only answer afterwards by returning `false`.
 *
 * The alternative was a menu row that always looks live and silently does nothing — which is
 * the one thing `menus/model.ts` refuses to represent, in as many words: a greyed control with
 * no explanation is "the single most common way this app has wasted a user's time". So the
 * question is asked twice, in one file, over the same map.
 */

/** Is there a block at the caret that is not already collapsed? */
export function canFold(state: EditorState): boolean {
  const target = targetRange(state)
  return target !== null && foldedSpanAt(state, target.from, target.to) === null
}

/** Is anything collapsed at the caret? */
export function canUnfold(state: EditorState): boolean {
  return visibleFoldAt(state) !== null
}

/** Is there anything in this document that could be collapsed? */
export function canFoldAll(state: EditorState): boolean {
  const map = state.field(foldMapField, false)
  if (map === undefined) return false
  return map.ranges().some((range) => foldedSpanAt(state, range.from, range.to) === null)
}

/** Is anything in this document collapsed? */
export function canUnfoldAll(state: EditorState): boolean {
  return foldedRanges(state).size > 0
}

/**
 * Whether an effect folds or unfolds something.
 *
 * `viewTracker.ts` is the caller, and it asks by name rather than leaning on the height change a
 * fold happens to produce — see the note beside its `update()`. Here rather than there so
 * `foldEffect`/`unfoldEffect` stay imported in one file.
 */
export function isFoldEffect(effect: StateEffect<unknown>): boolean {
  return effect.is(foldEffect) || effect.is(unfoldEffect)
}

/* -- shared helpers ----------------------------------------------------------------------- */

/** The scanner's range for the caret: the one starting on its line, else the one containing it. */
function targetRange(state: EditorState): FoldRange | null {
  const map = state.field(foldMapField, false)
  if (map === undefined) return null
  const ranges = map.ranges()
  const line = state.doc.lineAt(state.selection.main.head).number
  return foldAtLine(ranges, line) ?? enclosing(ranges, line)
}

/** Every folded span, in document order. */
function allFolded(state: EditorState): Span[] {
  const spans: Span[] = []
  const folded = foldedRanges(state)
  const cursor = folded.iter()
  while (cursor.value !== null) {
    spans.push({ from: cursor.from, to: cursor.to })
    cursor.next()
  }
  return spans
}

/** The folded span with exactly these bounds, or null. Answers "is this range already folded". */
function foldedSpanAt(state: EditorState, from: number, to: number): Span | null {
  let found: Span | null = null
  foldedRanges(state).between(from, from, (a, b) => {
    if (a === from && b === to) found = { from: a, to: b }
  })
  return found
}

/**
 * The outermost folded span covering the caret's line, or null.
 *
 * Outermost, because that is the one the user can see: nested folds inside a collapsed block are
 * hidden behind its placeholder, and expanding one of those would change nothing on screen.
 */
function visibleFoldAt(state: EditorState): Span | null {
  const line = state.doc.lineAt(state.selection.main.head)
  let found: Span | null = null
  for (const span of allFolded(state)) {
    if (span.to < line.from || span.from > line.to) continue
    if (found === null || span.from < found.from) found = span
  }
  return found
}

/** Dispatch a batch in one transaction, or report that there was nothing to do. */
function dispatchAll(view: EditorView, effects: readonly StateEffect<Span>[]): boolean {
  if (effects.length === 0) return false
  view.dispatch({ effects: [...effects] })
  return true
}
