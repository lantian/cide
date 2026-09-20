/**
 * What a buffer indents with, read off the buffer itself. (M59)
 *
 * `EditorSurface` used to hand every buffer `indentUnit.of('    ')`, and
 * `settings.editor.tabSize` / `insertSpaces` were persisted, offered in Settings ▸ Editor and
 * read by nothing — `docs/journal.md`'s M15 finding. The cost was invisible in this repository,
 * which is space-indented throughout, and immediate in a tab-indented one: Tab in a GDScript
 * file inserted four spaces, and Godot refuses a file that mixes the two ("Used space character
 * for indentation instead of tab as used before in the file"). Go and Makefiles are the same
 * shape with a friendlier compiler.
 *
 * So a buffer's indent unit is decided per buffer, from its own text: a file that already
 * indents with tabs gets a tab, one that indents with two spaces gets two, and only a file with
 * no indentation at all falls back to the settings. The settings still decide the *display*
 * width of a tab — that is a question about the user's eyes, not about the file.
 *
 * # Why a heuristic and not a parse
 *
 * The question is answered before the grammar loads (the indent unit is part of the extension
 * array the view is built with, and the grammar arrives a tick later through a dynamic
 * `import()`), so it has to be answerable from the raw text. A count of what the first few
 * thousand non-blank lines start with is enough: a file that is mostly one thing is that thing,
 * and a genuinely mixed file has no right answer this module could give.
 *
 * # Why this is import-free
 *
 * `check:editor` compiles it standalone and drives it over fixtures — a tab-indented script, a
 * two-space YAML, a four-space Python, a file with nothing indented — because a wrong answer
 * here is silent: nothing throws, the buffer looks right, and the user finds out from Godot.
 */

/** What the buffer indents with: a tab, or `width` spaces (`null` when only "spaces" is known). */
export type IndentStyle =
  | { readonly kind: 'tab' }
  | { readonly kind: 'spaces'; readonly width: number | null }

/** The settings' answer, used where the buffer has none. */
export interface IndentDefaults {
  /** `settings.editor.tabSize`: how wide a tab draws, and how many spaces one indent is. */
  readonly tabSize: number
  /** `settings.editor.insertSpaces`: whether Tab inserts spaces rather than a tab. */
  readonly insertSpaces: boolean
}

/** What the editor is built with. */
export interface IndentPolicy {
  /** The string one indent level inserts — CodeMirror's `indentUnit`. */
  readonly unit: string
  /** How wide a tab draws — CodeMirror's `EditorState.tabSize`. */
  readonly tabSize: number
}

/**
 * How many lines the detector reads before deciding.
 *
 * A file's indentation style is settled in its first screenful; the limit exists so that a
 * ten-megabyte log opened by accident does not pay for a walk it does not need. Blank lines
 * count towards it, so a file that is mostly blank simply decides on fewer votes.
 */
export const DETECT_LINE_LIMIT = 4000

/** The widest indent step a delta is believed to be. Anything past it is alignment, not indent. */
const MAX_STEP = 8

/** The band `tabSize` is kept inside; the Settings control offers 1–16 and a hand-edited file might not. */
const MIN_TAB_SIZE = 1
const MAX_TAB_SIZE = 16

/**
 * Read a buffer's indentation style off its text, or `null` when it has none to read.
 *
 * Every non-blank line votes by what its indentation *starts* with: a tab is a tab vote, one or
 * more spaces is a space vote. The majority wins; a tie is `null` (the settings decide). For a
 * space-indented file the width is the most common *positive step* between one non-blank
 * line's indent and the next — `0 → 4 → 8` votes 4 twice — capped at [`MAX_STEP`] so an
 * alignment continuation of twelve spaces is not mistaken for the indent. Ties go to the
 * smaller step, because a file whose steps are as often 2 as 4 is a two-space file with some
 * double indents in it. A space-indented file whose only steps were all past the cap answers
 * `width: null`, which [`indentUnitFor`] resolves from the settings.
 *
 * Blank lines — nothing but spaces, tabs and a trailing `\r` — never vote and never move the
 * "previous indent" the step is measured from, so a blank line inside a Python function does
 * not make the next line look like a jump from column zero.
 */
export function detectIndent(text: string, limit: number = DETECT_LINE_LIMIT): IndentStyle | null {
  let tabs = 0
  let spaces = 0
  // Index 0 is unused; `steps[d]` counts lines that stepped in by exactly `d` spaces.
  const steps: number[] = new Array<number>(MAX_STEP + 1).fill(0)
  let previous = 0
  let lines = 0
  let at = 0
  while (at < text.length && lines < limit) {
    lines += 1
    let end = text.indexOf('\n', at)
    if (end === -1) end = text.length
    let i = at
    let tabbed = false
    let width = 0
    if (text[i] === '\t') {
      tabbed = true
      while (i < end && text[i] === '\t') i += 1
    } else {
      while (i < end && text[i] === ' ') {
        i += 1
        width += 1
      }
    }
    // Whatever whitespace follows the leading run is alignment; what matters is whether
    // anything but whitespace comes after it on this line.
    let j = i
    while (j < end && (text[j] === ' ' || text[j] === '\t' || text[j] === '\r')) j += 1
    const blank = j >= end
    if (!blank) {
      if (tabbed) {
        tabs += 1
      } else {
        if (width > 0) {
          spaces += 1
          const step = width - previous
          if (step > 0 && step <= MAX_STEP) steps[step] = (steps[step] ?? 0) + 1
        }
        previous = width
      }
    }
    at = end + 1
  }
  if (tabs === 0 && spaces === 0) return null
  if (tabs > spaces) return { kind: 'tab' }
  if (spaces > tabs) {
    let best = 0
    let votes = 0
    for (let step = 1; step <= MAX_STEP; step += 1) {
      const count = steps[step] ?? 0
      // Strictly greater: the first step to reach the top count keeps it, which is the smaller.
      if (count > votes) {
        best = step
        votes = count
      }
    }
    return { kind: 'spaces', width: best > 0 ? best : null }
  }
  return null
}

/** Keep a tab width the editor can draw. `tabSize.of(0)` is a division by zero in a layout. */
export function clampTabSize(size: number): number {
  if (!Number.isFinite(size)) return 4
  return Math.min(MAX_TAB_SIZE, Math.max(MIN_TAB_SIZE, Math.round(size)))
}

/**
 * The string one indent inserts: what the buffer does, else what the settings say.
 *
 * A detected tab is a tab. A detected space width is that many spaces. A space-indented buffer
 * whose width could not be read takes the settings' `tabSize` as its width — it is a
 * space-indented file, and that much is known. Nothing detected: the settings decide both.
 */
export function indentUnitFor(detected: IndentStyle | null, defaults: IndentDefaults): string {
  const width = clampTabSize(defaults.tabSize)
  if (detected === null) return defaults.insertSpaces ? ' '.repeat(width) : '\t'
  if (detected.kind === 'tab') return '\t'
  return ' '.repeat(detected.width ?? width)
}

/**
 * Everything the editor needs, in one call: the unit from the buffer (or not, when detection is
 * off), the tab width from the settings, both total — a buffer is never built without an answer.
 */
export function indentPolicy(text: string, defaults: IndentDefaults, detect: boolean): IndentPolicy {
  const detected = detect ? detectIndent(text) : null
  return { unit: indentUnitFor(detected, defaults), tabSize: clampTabSize(defaults.tabSize) }
}
