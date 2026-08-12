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
 *
 * # Mixed files pay for an array, and that is the point
 *
 * A document with more than one break shape has no single answer to put back, and until this
 * change it got none: `restoreLineEndings` returned the buffer unchanged, which after
 * CodeMirror means LF throughout — the exact whole-file rewrite this module exists to
 * prevent, avoided for the three uniform cases and not for the one that needed it most.
 *
 * So a mixed document now carries the *sequence* of its breaks: one interned string per
 * line, captured at load, reapplied on save. On a 5 MB mixed file with 200,000 lines that is
 * an array of 200,000 pointers, ~1.6 MB, held for as long as the tab is open. That is the
 * honest cost and it is only paid by mixed files — [`captureLineEndings`] stores no array at
 * all for the uniform ones, which is every file anyone actually has.
 *
 * The alternative that lost was refusing to save a mixed file without an explicit choice.
 * It is defensible — it never guesses — but it puts a modal in front of a user who opened a
 * file with two stray `\r`s in it and wanted to fix a typo, and the honest thing to tell
 * them there is nothing they can act on. Silently unifying was never on the table.
 *
 * The alternative that lost *inside* this design is mapping the break array through every
 * `ChangeSet` so it stays aligned with the buffer through insertions. That is exact, and it
 * is a `StateField` and a per-keystroke splice living in `EditorSurface.tsx` where nothing
 * headless can reach it. What is here instead is index-aligned — see [`restoreLineEndings`]
 * for exactly what that does and does not preserve.
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

/** What the status bar readout says after the encoding. */
export type LineEnding = 'LF' | 'CRLF' | 'CR' | 'Mixed'

/** A break exactly as it appears in a file. The three shapes CodeMirror splits on. */
export type Break = '\n' | '\r\n' | '\r'

/** The break each uniform [`LineEnding`] is spelled as. `Mixed` has no single answer. */
const BREAK_OF: Record<Exclude<LineEnding, 'Mixed'>, Break> = {
  LF: '\n',
  CRLF: '\r\n',
  CR: '\r',
}

/**
 * What a document's endings were when it was loaded, which is the only time they can be read.
 *
 * Produced by [`captureLineEndings`] and consumed by [`restoreLineEndings`]. Carried as one
 * object rather than as a bare [`LineEnding`] so that the mixed case has somewhere to keep
 * its sequence — a caller holding only the classification cannot restore such a file, which
 * is how the loss this module documented for two milestones happened.
 */
export interface DocumentEndings {
  /** The classification, for the status bar readout. */
  readonly ending: LineEnding
  /**
   * The break that followed each line, in file order, or `null`.
   *
   * Populated only when `ending` is `Mixed`; a uniform document is restored from [`fill`]
   * alone and must not pay for an array per line. Length is one less than the line count.
   */
  readonly breaks: readonly Break[] | null
  /**
   * The break given to a line that did not exist when the document was loaded.
   *
   * For a uniform document that is simply its ending. For a mixed one it is the shape the
   * file uses *most*, because a line the user has just typed belongs to the file as a whole
   * and not to whatever happened to be at that index before. Ties go to the break that
   * appears first in the file, so the answer depends only on the file and not on iteration
   * order.
   */
  readonly fill: Break
}

/**
 * The ending a document uses, or `Mixed` when it does not use one.
 *
 * "Every break the same, or mixed" is the deliberate test.
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
 * Every break in the document, in order.
 *
 * Two cursors rather than one `indexOf` per break from the current position, and that is not
 * a micro-optimisation: `text.indexOf('\r', i)` scans to the end of the string when there is
 * no `\r` left, so a file with one stray `\r` at the top and 100,000 LF lines after it would
 * scan the whole tail once per line. Each cursor here is re-searched only when it is the one
 * that was consumed, so the total work is one pass.
 */
function breakSequence(text: string): Break[] {
  const breaks: Break[] = []
  let cr = text.indexOf('\r')
  let lf = text.indexOf('\n')
  for (;;) {
    if (cr === -1 && lf === -1) return breaks
    if (cr !== -1 && (lf === -1 || cr < lf)) {
      if (lf === cr + 1) {
        // A CRLF consumes both cursors; `\n` is not a break of its own here.
        breaks.push('\r\n')
        lf = text.indexOf('\n', cr + 2)
      } else {
        breaks.push('\r')
      }
      cr = text.indexOf('\r', cr + 1)
    } else {
      breaks.push('\n')
      lf = text.indexOf('\n', lf + 1)
    }
  }
}

/**
 * The break shape a mixed document uses most; ties go to the one that appears first in it.
 *
 * The tiebreak is not decoration. `a\r\nb\nc` is two breaks, one each, and without a rule
 * derived from the file itself the answer would come from whichever order this function
 * happens to test the three shapes in — so a file's newly typed lines would get their ending
 * from an implementation detail. First appearance is the only tiebreak the *file* supplies.
 */
function dominantBreak(breaks: readonly Break[]): Break {
  const count: Record<Break, number> = { '\n': 0, '\r\n': 0, '\r': 0 }
  /** -1 until seen, so a shape the file does not use can never win. */
  const firstAt: Record<Break, number> = { '\n': -1, '\r\n': -1, '\r': -1 }
  for (let i = 0; i < breaks.length; i++) {
    const shape = breaks[i] as Break
    count[shape]++
    if (firstAt[shape] === -1) firstAt[shape] = i
  }

  let best: Break = '\n'
  for (const shape of ['\n', '\r\n', '\r'] as const) {
    if (firstAt[shape] === -1) continue
    if (
      firstAt[best] === -1 ||
      count[shape] > count[best] ||
      (count[shape] === count[best] && firstAt[shape] < firstAt[best])
    ) {
      best = shape
    }
  }
  return best
}

/**
 * Read a document's endings, once, before CodeMirror is allowed to touch it.
 *
 * The uniform cases cost one classification and no allocation. Only `Mixed` walks the text a
 * second time to record the sequence, which is the trade [`DocumentEndings`] describes.
 */
export function captureLineEndings(text: string): DocumentEndings {
  const ending = detectLineEnding(text)
  if (ending !== 'Mixed') return { ending, breaks: null, fill: BREAK_OF[ending] }

  const breaks = breakSequence(text)
  return { ending, breaks, fill: dominantBreak(breaks) }
}

/**
 * Put a document's own endings back into text that came out of a CodeMirror buffer.
 *
 * `endings` is what [`captureLineEndings`] said about the document *as it was loaded*, not
 * about the buffer's current contents — by then every ending is `\n` and the question can no
 * longer be asked.
 *
 * # What the mixed case preserves, and what it does not
 *
 * The breaks are reapplied by index: the *n*th line of the buffer gets the break that
 * followed the *n*th line of the file. An edit that does not change the number of lines —
 * which is most edits, and every edit a typo fix makes — therefore restores the file exactly,
 * byte for byte. Lines past the end of the record get [`DocumentEndings.fill`].
 *
 * An insertion or deletion shifts every line after it against the record, so the tail of a
 * mixed file can come back with its shapes rotated by however many lines were added or
 * removed. That is a real limitation and it is bounded in the right direction: on the mixed
 * files that exist in practice — one shape throughout and a handful of strays — a rotation
 * changes nothing at all, and where it does change something it rewrites the lines after the
 * edit rather than every line in the file. Keeping the record aligned exactly needs it mapped
 * through each `ChangeSet`; see this module's header for why that is not what is here.
 */
export function restoreLineEndings(text: string, endings: DocumentEndings): string {
  const { breaks, fill } = endings
  if (breaks === null) {
    // Uniform. `\n` is what the buffer already holds, so LF is the identity and not a scan.
    return fill === '\n' ? text : text.replace(/\n/g, fill)
  }

  const lines = text.split('\n')
  if (lines.length === 1) return text

  // Built as an array and joined once. `+=` in a loop over a five-megabyte document leaves
  // V8 to rope together several million cons strings, which it does flatten — after holding
  // all of them.
  const parts: string[] = [lines[0] as string]
  for (let i = 1; i < lines.length; i++) {
    parts.push(breaks[i - 1] ?? fill, lines[i] as string)
  }
  return parts.join('')
}
