/**
 * **What "a position in a file" is, once, for everything that needs one.** (M12)
 *
 * Two features landed together that both need to say where somebody is in a buffer — per-file
 * view memory ("put me back where I was") and the navigation history behind the mouse's
 * back/forward buttons. They keep separate *stores*, for reasons written out in
 * `navHistory.ts`, and they share exactly one thing: this type.
 *
 * That is not tidiness. This repository has already shipped two counts for one number in the
 * git panel, and the way it happened was two modules each deciding what the number meant. A
 * second spelling of "line and column" is the same failure waiting: an off-by-one gets into one
 * feature and not the other, and the two disagree only for files that contain a `é`.
 *
 * # The units, stated once
 *
 * `line` is **1-based**. `column` is **1-based and counted in UTF-16 code units** — not bytes,
 * not grapheme clusters. That is what a CodeMirror document position natively is, and it is
 * already the convention of `RevealTarget` (`revealRequest.ts`), `CaretPosition`
 * (`caretTrack.ts`), `SymbolSpan` and the Rust `ViewPosition`. Bytes would be off by one per
 * non-ASCII character before the caret, which lands it inside the wrong word.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-editor.mjs` compiles it alone with the `tsc` in `node_modules` and runs the
 * output under node. Every decision below — the clamp, the restore veto, whether a note is
 * worth an IPC call — would otherwise live inside a `useEffect` in a component that needs a
 * window, which is the one place in this codebase no check script can reach and where three
 * shipped bugs have now hidden.
 */

/** A place in a file. The shared currency; see the header for the units. */
export interface FilePosition {
  /** Absolute path. */
  readonly path: string
  /** 1-based. */
  readonly line: number
  /** 1-based, UTF-16 code units. */
  readonly column: number
}

/**
 * A place *and* the scroll that showed it — what the view memory stores.
 *
 * `topLine` is the first visible line, and it is why this is not just a `FilePosition`.
 * Restoring the caret alone puts the caret's line *somewhere* on screen (CodeMirror's minimal
 * scroll brings it just inside the nearest edge), which is not the same view the user left.
 * Restoring the viewport alone leaves the selection at the top of the document, so the first
 * arrow key teleports them back to line 1. The report was "same lines", and it takes both.
 *
 * **Lines and not pixels.** `EditorView.lineWrapping` is on for every file under the highlight
 * limit and the code font size is a runtime setting, so a pixel offset is wrong after a window
 * resize, a sidebar drag or a font change — pointing at a different part of the file rather
 * than merely being imprecise. This app supports both of those at runtime, which is exactly why
 * the obvious `scrollTop` is the wrong thing to store.
 */
export interface FileView extends FilePosition {
  /** First visible line, 1-based. */
  readonly topLine: number
}

/** Are these the same place? Path, line and column, exactly. */
export function samePosition(a: FilePosition | null, b: FilePosition | null): boolean {
  if (a === null || b === null) return a === b
  return a.path === b.path && a.line === b.line && a.column === b.column
}

/** Are these the same place *and* the same scroll? */
export function sameView(a: FileView | null, b: FileView | null): boolean {
  if (a === null || b === null) return a === b
  return samePosition(a, b) && a.topLine === b.topLine
}

/**
 * Whether a freshly observed view is worth one IPC call.
 *
 * This is the third rung of the write-amplification ladder described in
 * `crates/cide-app/src/positions_state.rs`, and the cheapest one: a click that lands the caret
 * exactly where it already was, a scroll that ends where it started, a focus change that moves
 * nothing — all of them reach the debounce and none of them is news. Dropping them here costs a
 * three-field comparison and saves a round trip plus a store mutation plus, eventually, a disk
 * write.
 */
export function worthNoting(previous: FileView | null, next: FileView): boolean {
  return !sameView(previous, next)
}

/**
 * Bring a remembered view inside a document of `lines` lines.
 *
 * **The clamp is the point of this function, not a courtesy.** A position is recorded against
 * one version of a file and applied to another: the agent rewrote it, a `cargo fmt` shortened
 * it, the user reopened it after editing it elsewhere. `doc.line(n)` *throws* for an `n` past
 * the end, and that exception escapes through `dispatch` into an effect with no error boundary
 * above it — the React root unmounts and every terminal in the window goes with it. Landing on
 * the last line is a disappointment; a blank window is a lost session. `revealRange` carries
 * the same reasoning for the same reason, and this is the second caller that needed it in a
 * shape `Text` was not available for.
 *
 * Non-finite input is clamped rather than rejected: `NaN` fails every comparison a clamp is
 * made of, so it sails through one written the obvious way and throws at the end of it.
 */
export function clampView(at: FileView, lines: number): FileView {
  const total = Math.max(1, whole(lines, 1))
  return {
    path: at.path,
    line: clamp(whole(at.line, 1), 1, total),
    column: Math.max(1, whole(at.column, 1)),
    topLine: clamp(whole(at.topLine, 1), 1, total),
  }
}

/**
 * Which line to *call* the top one, given the line the top pixel actually hit.
 *
 * # Measured, and both directions of the error were measured
 *
 * Seeding `positions.json` with `topLine: 200`, launching the real app and reading the value
 * back gave **199**: `EditorView.scrollIntoView(pos, { y: 'start' })` leaves a 5px `yMargin` by
 * default, so the restored line sat five pixels below the viewport top and the hit test at the
 * top pixel landed on the line above. One line per restore, *cumulative* — every relaunch would
 * creep another line up, which is exactly the defect that gets reported six months later as
 * "it drifts".
 *
 * `yMargin: 0` at the call site removed that. A strict "the first line whose top edge is at or
 * below the viewport top" rule was then tried, and it over-corrected in the other direction —
 * 200 came back as **201**. The instrumented numbers say why: with the margin gone the line's
 * top is at 66 and the scroller's border box starts at 68, because `.cm-scroller` sits inside a
 * border and CodeMirror aligns to the *client* area rather than to the box
 * `getBoundingClientRect` reports. Two pixels.
 *
 * So the rule is neither "any part visible" nor "entirely visible" but the one a reader would
 * recognise: **the top line is the first one you can actually read** — the hit line unless more
 * than half of it is cut off. That is stable under a two-pixel offset, stable under a fractional
 * device pixel ratio, and stable under a user who parks the scroll between two lines, because
 * restoring the answer reproduces an offset far below half a line.
 */
export function topVisibleLine(
  hitLine: number,
  lineTop: number,
  lineHeight: number,
  viewportTop: number,
  totalLines: number,
): number {
  // A line height of zero is a geometry read from a pane that has not been laid out — every
  // rect is 0 and `hidden` is therefore the whole viewport offset. Answering "not cut off" keeps
  // the hit line rather than walking off the end of a document nobody is looking at yet.
  if (!(lineHeight > 0)) return hitLine
  const hidden = viewportTop - lineTop
  // Non-positive `hidden` is a line starting at or below the viewport top. Written as a negated
  // `>` so a `NaN` from an unlaid-out rect answers "keep the hit line" rather than walking.
  if (!(hidden > lineHeight / 2)) return hitLine
  return Math.min(hitLine + 1, Math.max(1, totalLines))
}

/**
 * What a mounting editor should do with a remembered position — or `null` for "nothing".
 *
 * # The one rule that is not arithmetic
 *
 * **An explicit navigation outranks a remembered position, always.** Both arrive at the same
 * moment: an editor mounts, `registerReveal` spends whatever `requestReveal` parked for that
 * path, and the restore below wants to install a selection of its own. If the user pressed
 * Ctrl+B, clicked a search hit, or walked Back into a file they had previously scrolled, the
 * place they *named* wins over the place they left — otherwise Go to definition into a file
 * you had open last week lands you where you were last week.
 *
 * Today the ordering also falls out for free, because the state is created before
 * `registerReveal` runs, so a parked reveal dispatches last and overwrites the restore. That is
 * exactly the kind of correctness that survives until someone reorders two statements, so it is
 * stated here as a rule and pinned by a check instead of being left to the line numbers.
 *
 * # Why the veto is a parameter and not a call
 *
 * `pendingReveals()` lives in `revealRequest.ts`, which imports `@codemirror/state`. Taking the
 * answer as a boolean keeps this module import-free — which is what lets the check script
 * compile it standalone — and keeps the decision, rather than the lookup, as the thing being
 * tested.
 */
export function planRestore(
  at: FileView | null,
  lines: number,
  hasParkedReveal: boolean,
): FileView | null {
  if (at === null) return null
  if (hasParkedReveal) return null
  return clampView(at, lines)
}

function whole(value: number, fallback: number): number {
  return Number.isFinite(value) ? Math.trunc(value) : fallback
}

function clamp(value: number, lo: number, hi: number): number {
  if (value < lo) return lo
  return value > hi ? hi : value
}
