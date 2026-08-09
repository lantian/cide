/**
 * Line endings, and getting a file's back after CodeMirror has eaten them.
 *
 * `EditorState.create` splits an incoming document on `/\r\n?|\n/` and `Text.toString()`
 * rejoins it with `\n`, so a CRLF file handed to an editor comes back LF-only. That is fine
 * while the text stays in the buffer and catastrophic the moment it is written back: a
 * one-character edit to a Windows file rewrites every line of it, and the git panel then
 * shows a whole-file diff for a typo fix.
 *
 * The rule is the one `panes/DiffPane.tsx` arrived at for the accept path — restore what the
 * file had, never impose an ending — but the implementation here is deliberately *not* a
 * copy of that one. `DiffPane` counts `\n` only, so it classifies a document whose breaks
 * are all bare `\r` as having no breaks at all; CodeMirror splits on `\r` too, and the
 * result is that saving such a file silently rewrites every line. That case is rare and it
 * is still a whole-file rewrite nobody asked for, so all three of the shapes CodeMirror
 * splits on are counted below. (Merging `DiffPane` onto this module is worth doing and is
 * not this file's change to make: its `crlfThroughout` feeds a diff decision, not a write.)
 */

/** How many of each break shape a document has. These are exactly what CodeMirror splits on. */
interface Breaks {
  lf: number
  crlf: number
  cr: number
}

/**
 * Count the three break shapes in one pass each.
 *
 * `indexOf` rather than a character loop: this runs on every load, and on the 5 MB file the
 * milestone tests with a JS-level loop over 5 million code units is tens of milliseconds of
 * the time between clicking a file and seeing it.
 */
function countBreaks(text: string): Breaks {
  let crlf = 0
  let cr = 0
  for (let i = text.indexOf('\r'); i !== -1; i = text.indexOf('\r', i + 1)) {
    if (text.charCodeAt(i + 1) === 10) crlf++
    else cr++
  }
  let lf = 0
  for (let i = text.indexOf('\n'); i !== -1; i = text.indexOf('\n', i + 1)) lf++
  // Every CRLF also contributed an `\n` to that count, and it is not a break of its own.
  return { lf: lf - crlf, crlf, cr }
}

/** Line count without allocating the split array — a 5 MB file makes that matter. */
export function countLines(text: string): number {
  const { lf, crlf, cr } = countBreaks(text)
  return 1 + lf + crlf + cr
}

/** What the breadcrumb readout says after the encoding. */
export type LineEnding = 'LF' | 'CRLF' | 'CR' | 'Mixed'

/**
 * The ending a document uses, or `Mixed` when it does not use one.
 *
 * "Every break the same, or mixed" is the deliberate test. A file with mixed endings is left
 * exactly as CodeMirror returned it rather than being silently unified in a direction nobody
 * asked for: the user's edit is the change they consented to, and normalising the other 400
 * lines is not.
 *
 * A file with no line break at all reports `LF`, which is what it will be given one day and
 * what every tool assumes in the meantime. Reporting "none" would be more accurate and would
 * put a state in the readout that means nothing to the reader.
 */
export function detectLineEnding(text: string): LineEnding {
  const { lf, crlf, cr } = countBreaks(text)
  const total = lf + crlf + cr
  if (total === 0 || lf === total) return 'LF'
  if (crlf === total) return 'CRLF'
  if (cr === total) return 'CR'
  return 'Mixed'
}

/**
 * Put a document's own endings back into text that came out of a CodeMirror buffer.
 *
 * `ending` is the answer [`detectLineEnding`] gave for the document *as it was loaded*, not
 * for the buffer's current contents — by then every ending is `\n` and the question can no
 * longer be asked. `Mixed` restores nothing, which is the point: there is no single answer
 * to put back, so the buffer goes out as the editor gave it.
 */
export function restoreLineEndings(text: string, ending: LineEnding): string {
  if (ending === 'CRLF') return text.replace(/\n/g, '\r\n')
  if (ending === 'CR') return text.replace(/\n/g, '\r')
  return text
}
