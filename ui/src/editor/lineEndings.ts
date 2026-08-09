/**
 * Line endings, and getting a file's back after CodeMirror has eaten them.
 *
 * `EditorState.create` splits an incoming document on `/\r\n?|\n/` and `Text.toString()`
 * rejoins it with `\n`, so a CRLF file handed to an editor comes back LF-only. That is fine
 * while the text stays in the buffer and catastrophic the moment it is written back: a
 * one-character edit to a Windows file rewrites every line of it, and the git panel then
 * shows a whole-file diff for a typo fix.
 *
 * The rule and the reasoning are the same ones `panes/DiffPane.tsx` arrived at for the
 * accept path — see its `crlfThroughout`. This module is the shared home the editor needs
 * because it also has to *display* the answer in the breadcrumb readout, and because a
 * second hand-rolled copy of a rule this easy to get subtly wrong is how the two paths
 * would drift.
 */

/** Line count without allocating the split array — a 5 MB file makes that matter. */
export function countLines(text: string): number {
  let n = 1
  for (let i = text.indexOf('\n'); i !== -1; i = text.indexOf('\n', i + 1)) n++
  return n
}

/**
 * Whether every line break in the text is a CRLF.
 *
 * "Every break, or none" is the deliberate test. A file with mixed endings is left exactly
 * as CodeMirror returned it rather than being silently unified in a direction nobody asked
 * for: the user's edit is the change they consented to, and normalising the other 400 lines
 * is not.
 */
export function crlfThroughout(text: string): boolean {
  const breaks = countLines(text) - 1
  if (breaks === 0) return false
  let crlf = 0
  for (let i = text.indexOf('\r\n'); i !== -1; i = text.indexOf('\r\n', i + 2)) crlf++
  // A bare CR is a line break to CodeMirror as well, so one anywhere means the endings are
  // mixed however the CRLFs count up, and the text goes back exactly as the editor gave it.
  return crlf === breaks && !/\r(?!\n)/.test(text)
}

/** What the breadcrumb readout says after the encoding. */
export type LineEnding = 'LF' | 'CRLF' | 'Mixed'

/**
 * The label for a document's line endings.
 *
 * A file with no line break at all reports `LF`, which is what it will be given one day and
 * what every tool assumes in the meantime. Reporting "none" would be more accurate and
 * would put a third state in the readout that means nothing to the reader.
 */
export function detectLineEnding(text: string): LineEnding {
  const breaks = countLines(text) - 1
  if (breaks === 0) return 'LF'
  if (crlfThroughout(text)) return 'CRLF'
  return text.includes('\r') ? 'Mixed' : 'LF'
}

/**
 * Put CRLF back into text that came out of a CodeMirror buffer.
 *
 * `crlf` is the answer [`crlfThroughout`] gave for the document *as it was loaded*, not for
 * the buffer's current contents — by then every ending is `\n` and the question can no
 * longer be asked.
 */
export function restoreLineEndings(text: string, crlf: boolean): string {
  return crlf ? text.replace(/\n/g, '\r\n') : text
}
