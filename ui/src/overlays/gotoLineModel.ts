/**
 * What the Go to line box accepts, and what it says about what it refuses.
 *
 * # Why this is a module and not a regex in the component
 *
 * The jump itself is safe whatever arrives: `editor/revealRequest.ts::revealRange` clamps the
 * line to `[1, doc.lines]` and the column to the line, and its header explains at length why
 * that clamp is load-bearing rather than defensive — an out-of-range `doc.line` throws inside
 * `dispatch`, escapes a handler with no error boundary above it, and takes the React root and
 * every terminal in the window with it.
 *
 * What the clamp cannot do is refuse. `whole()` turns a non-finite number into `1`, so a
 * mistyped `l20` would sail through and jump silently to the top of the file. "Somewhere
 * unexpected" is a worse outcome than a disabled button, so the *refusal* lives here, in front
 * of it, and it lives in a module with no imports so `ui/scripts/check-picker.mjs` can compile it
 * standalone and drive every case. This project has paid three times for a rule that lived only
 * in an event handler, which is the one place no check script can reach.
 *
 * The `…Model` suffix is the same repair `closeConfirmModel.ts` explains: `gotoLine.ts` beside
 * `GoToLine.tsx` differs only in case, which is one path on macOS and a build failure there.
 * `ui/scripts/check-casing.mjs` is the gate that sees it from Linux.
 *
 * # The syntax, and the three things deliberately left out
 *
 * `120` and `120:8`, and nothing else.
 *
 * * **No `+n` / `-n` relative jumps.** CodeMirror's own `gotoLine` parses them, and IDEA — whose
 *   Ctrl+G this is — does not. More to the point they are *ambiguous with a typo*: a leading `-`
 *   is far more likely to be a stray keystroke than a request to go back five lines, and
 *   guessing wrong moves the caret somewhere the user did not name. Refused with a sentence
 *   rather than clamped to line 1, which is what silently accepting them would amount to.
 * * **No `n%`.** Same source, same reason, and nobody has ever asked for it.
 * * **No thousands separators.** `1,200` is refused rather than read as 1200; accepting it would
 *   mean picking a locale, and a European user's `1.200` would then have to mean something too.
 *
 * `0` *is* accepted, and it is the one case where clamping beats refusing: it is unambiguous —
 * every user who types it means the top of the file — and [`gotoNote`] says so out loud before
 * they commit to it. A line past the end is accepted for the same reason, with the same warning.
 */

/** A place in the file, exactly as typed. Not clamped — `revealRange` owns the clamp. */
export interface GotoTarget {
  /** 1-based, and possibly 0 or past the end of the document. */
  readonly line: number
  /** 1-based UTF-16 column. 1 when the user typed a bare line number. */
  readonly column: number
}

/** What the field currently holds. */
export type GotoParse =
  /** Nothing typed yet. Not an error — it is the state the box opens in. */
  | { readonly kind: 'empty' }
  /** Typed, and not a place. `reason` is shown under the field verbatim. */
  | { readonly kind: 'invalid'; readonly reason: string }
  | { readonly kind: 'ok'; readonly at: GotoTarget }

/**
 * The place a parsed target actually resolves to.
 *
 * `parseGoto` returns what was *typed* — that is its contract, and `gotoNote` reads the raw
 * value to warn "Before the first line". But the number that leaves this module has to be a
 * real line, for a reason that is not merely tidiness: `editor/jump.ts` uses **0 as its
 * `UNKNOWN_LINE` sentinel**, so handing it a literal `0` meant the reveal was skipped
 * entirely. The caret did not move, the popup's own promise — "goes to line 1" — was broken,
 * and because the reveal is also what returns focus to the editor, the keyboard was left on
 * `<body>` with nothing to type into.
 *
 * So the clamp the header describes happens *here*, before the value crosses a boundary where
 * one of its values means something else. `revealRange` still clamps the upper end; this owns
 * the lower one, where the collision is.
 */
export function resolveGoto(at: GotoTarget): GotoTarget {
  return at.line < 1 ? { line: 1, column: at.column } : at
}

/** `120`, or `120:8`. Nothing else — see the header. */
const LINE_ONLY = /^\d+$/
const LINE_AND_COLUMN = /^(\d+):(\d+)$/

export function parseGoto(text: string): GotoParse {
  const trimmed = text.trim()
  if (trimmed.length === 0) return { kind: 'empty' }

  if (LINE_ONLY.test(trimmed)) {
    return { kind: 'ok', at: { line: Number(trimmed), column: 1 } }
  }

  const pair = LINE_AND_COLUMN.exec(trimmed)
  if (pair !== null) {
    // `[1]` and `[2]` are non-optional groups of a regex that matched, but
    // `noUncheckedIndexedAccess` cannot know that and a `!` would be the one assertion in this
    // file that a check script could not falsify.
    const line = Number(pair[1] ?? '')
    const column = Number(pair[2] ?? '')
    /*
     * Column 0 is refused where line 0 is accepted, and the asymmetry is deliberate. Line 0 is
     * a whole-file gesture — "the top" — and everybody who types it means that. Column 0 is
     * arithmetic: somebody is pasting a 0-based position out of a tool that counts columns from
     * zero, and quietly treating it as column 1 hides an off-by-one they would want to know
     * about. `1` is the first column everywhere else in this app.
     */
    if (column === 0) {
      return { kind: 'invalid', reason: 'Columns start at 1' }
    }
    return { kind: 'ok', at: { line, column } }
  }

  // Named separately from the general failure, because the user is not typing nonsense — they
  // are typing a syntax another editor has and this box does not, and "type a line number" would
  // read as though they had not.
  if (/^[+-]\d/.test(trimmed) || /^\d+%$/.test(trimmed)) {
    return { kind: 'invalid', reason: 'Type an absolute line number — no +n, -n or n%' }
  }

  return { kind: 'invalid', reason: 'Type a line number, or line:column' }
}

/**
 * The sentence under the field, or `null` when the input speaks for itself.
 *
 * Written from the parse plus the document's size rather than from the text, so the "past the
 * end" warning and the jump that follows it cannot disagree about what was typed.
 *
 * `lines` is the document's line count, which is at least 1 — CodeMirror counts an empty
 * document as one empty line, and so does `caretTrack`.
 */
export function gotoNote(parsed: GotoParse, lines: number): string | null {
  if (parsed.kind === 'empty') return null
  if (parsed.kind === 'invalid') return parsed.reason
  if (parsed.at.line < 1) return 'Before the first line — goes to line 1'
  if (parsed.at.line > lines) {
    // The number is in the sentence because the whole point is to correct the user's mental
    // model of the file, not merely to warn them: they typed 1200 and this file has 892.
    return `Past the end — this file has ${lines} ${lines === 1 ? 'line' : 'lines'}`
  }
  return null
}

/**
 * Whether Enter should do anything.
 *
 * A separate predicate rather than `parsed.kind === 'ok'` inlined at the two call sites — the
 * button's `disabled` and the Enter handler — because those two agreeing is the whole of "the
 * key and the button do the same thing", and two spellings of one condition is how they stop.
 */
export function canGo(parsed: GotoParse): parsed is { kind: 'ok'; at: GotoTarget } {
  return parsed.kind === 'ok'
}
