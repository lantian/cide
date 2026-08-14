/**
 * A file's diagnostics, as CodeMirror wants them.
 *
 * Pure arithmetic over positions, kept out of `EditorSurface.tsx` for the only reason anything is
 * kept out of it: that file needs a DOM and a theme, so `check-editor.mjs` cannot reach it. This
 * one it compiles standalone.
 *
 * # The conversion, and the two ways it goes wrong
 *
 * A diagnostic carries **1-based line and 1-based UTF-16 column**; CodeMirror wants a **0-based
 * document offset**. Both steps have a failure mode that is invisible in a fixture and obvious to
 * a user:
 *
 * 1. **A line past the end of the document.** Routine rather than exotic — the buffer is edited
 *    while the analyser is still thinking, so a diagnostic for line 400 can arrive at a document
 *    that now has 380. `EditorView.dispatch` **throws** on an out-of-range range, and there is no
 *    error boundary over the editor, so an unclamped offset takes the whole React root down and
 *    every terminal in the window with it. That is the same reasoning `revealRange` states.
 * 2. **A zero-width range.** `setDiagnostics` accepts `from === to`, and CodeMirror draws exactly
 *    nothing for it — a real error with no squiggle, which reads as a missed diagnostic rather
 *    than as a zero-width one. So an empty range is widened to one character.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-outline.mjs` and `check-editor.mjs` compile it with a bare `tsc`. The two
 * shapes it needs — a diagnostic and a document — are restated structurally, which also means it
 * can be driven from a plain object in a test without constructing an `EditorState`.
 */

/** The half of `cide_ipc::Diagnostic` this module reads. */
export interface LintSource {
  readonly line: number
  readonly column: number
  /**
   * The end of the span, exclusive. Both optional: a producer may report a *point*, and the panel
   * model's own fixtures predate the editor drawing anything at all.
   *
   * Absent means a one-character span starting at `line`/`column` — see [`lintRanges`], which
   * would otherwise produce a zero-width range CodeMirror draws nothing for.
   */
  readonly endLine?: number | undefined
  readonly endColumn?: number | undefined
  readonly severity: string
  readonly message: string
  readonly source?: string | undefined
  readonly code?: string | undefined
}

/** The half of CodeMirror's `Text` this module reads. */
export interface DocLike {
  readonly lines: number
  readonly length: number
  line(n: number): { readonly from: number; readonly to: number }
}

/** What `setDiagnostics` takes. */
export interface LintRange {
  from: number
  to: number
  severity: 'error' | 'warning' | 'info' | 'hint'
  message: string
  /**
   * `rust-analyzer E0308`, dimmed after the message. Absent when neither is known.
   *
   * `source?: string` and deliberately **not** `string | undefined`: CodeMirror's own
   * `Diagnostic` declares it the first way, and under `exactOptionalPropertyTypes` the two are
   * different — the second permits an explicitly-`undefined` property, which the first refuses.
   * The conditional spread below is what makes this accurate rather than a cast.
   */
  source?: string
}

/** The four CodeMirror knows. Anything else is drawn as an error rather than dropped. */
function severityOf(from: string): LintRange['severity'] {
  switch (from) {
    case 'warning':
      return 'warning'
    case 'info':
      return 'info'
    case 'hint':
      return 'hint'
    case 'error':
      return 'error'
    default:
      /*
       * An unrecognised severity is **shown**, not dropped — the `other`-bucket rule from
       * `ProblemsPanel/model.ts`, applied to a squiggle. The producer is a process we do not
       * control, and a diagnostic the panel lists but the gutter silently omits is worse than one
       * drawn in the wrong colour.
       */
      return 'error'
  }
}

/**
 * A 1-based line and 1-based UTF-16 column as a document offset, clamped into the document.
 *
 * Exported because the clamp *is* the contract, and a caller that wants one position — a jump, a
 * tooltip anchor — should not have to reimplement it.
 */
export function offsetOf(doc: DocLike, line: number, column: number): number {
  if (doc.lines === 0) return 0
  const clampedLine = Math.min(Math.max(line, 1), doc.lines)
  const at = doc.line(clampedLine)
  // `column - 1` because the column is 1-based; clamped to the line's own end so a stale column
  // cannot reach into the next line, which would put a squiggle under text it is not about.
  const offset = at.from + Math.max(column - 1, 0)
  return Math.min(Math.max(offset, at.from), at.to)
}

/**
 * Diagnostics for one document, in CodeMirror's shape and document order.
 *
 * Sorted because `setDiagnostics` requires it — an unsorted list throws, and the throw lands
 * inside a `dispatch` with no error boundary above it.
 */
export function lintRanges(items: readonly LintSource[], doc: DocLike): LintRange[] {
  const ranges = items.map((item) => {
    const from = offsetOf(doc, item.line, item.column)
    let to = offsetOf(doc, item.endLine ?? item.line, item.endColumn ?? item.column)
    // Never backwards: a producer that reports an end before its start, or a clamp that pulled
    // the end above the start, would otherwise make an inverted range CodeMirror rejects.
    if (to < from) to = from
    // And never empty — see the header.
    if (to === from) to = Math.min(from + 1, doc.length)
    const label = [item.source, item.code].filter((part) => part !== undefined && part !== '')
    return {
      from,
      to,
      severity: severityOf(item.severity),
      message: item.message,
      ...(label.length > 0 ? { source: label.join(' ') } : {}),
    }
  })
  // `from` then `to`, which is the order `setDiagnostics` documents. Ties on both keep their
  // arrival order, so two findings at one position stay stable between renders.
  ranges.sort((a, b) => a.from - b.from || a.to - b.to)
  return ranges
}
