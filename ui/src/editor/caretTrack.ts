/**
 * Where the caret is, for the surfaces that are not inside an editor.
 *
 * The File Structure popup is mounted by `OverlayHost` and the member walk runs in
 * `keys/dispatch.ts`; neither has an `EditorView`, and neither can get one — a pane's CodeMirror
 * instance lives in a module-level map outside React precisely so nothing above it holds a
 * reference. So the caret comes here instead, written by whichever editor has it.
 *
 * # Why this is a plain assignment and not a store
 *
 * `EditorSurface`'s update listener runs on every selection change — thirty times a second under
 * a held arrow key — and that listener already goes to some length to avoid React: it writes the
 * cursor readout straight into a DOM node. A zustand `set()` here would undo that and re-render
 * whatever subscribed, per keystroke. Nothing subscribes; [`focusedCaret`] is a *read*, called
 * once when a popup opens or a chord fires.
 *
 * # Why claims are a stack
 *
 * The same reason `statusReadout.ts`'s are, and the two move together: a split shows two editors
 * and there is one caret worth reporting — the one in the editor the user is in. Mounting claims
 * it, focus takes it back, and releasing hands it to the editor underneath rather than blanking
 * it, so closing one half of a split leaves the other half's caret answering.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-outline.mjs` compiles it alone with a bare `tsc` and runs it under node.
 */

/** 1-based line and 1-based UTF-16 column, matching `RevealTarget` and `SymbolSpan`. */
export interface CaretPosition {
  readonly path: string
  readonly line: number
  readonly column: number
  /**
   * How many lines the buffer has. At least 1 — an empty document is one empty line.
   *
   * Here rather than fetched on demand because there is nowhere to fetch it from: the document
   * lives in a CodeMirror `EditorState` inside a pane, and the whole reason this module exists is
   * that the surfaces which need it cannot reach one. Go to line is the caller — it has to tell
   * the user *before* they press Enter that 1200 is past the end of an 892-line file, and the
   * clamp inside `revealRange` can only tell them afterwards, by moving the caret somewhere they
   * did not name.
   *
   * It rides along on the position rather than being a second claim because it changes on
   * exactly the transactions the position does, and two stacks answering "which editor is the
   * user in" is one more than there should be.
   */
  readonly lines: number
}

/** One mounted editor's hold on the caret slot. */
export interface CaretSlot {
  /** Report a new position. Reaches [`focusedCaret`] only while this slot is on top. */
  set(line: number, column: number, lines: number): void
  /** Take the slot — the user is in this editor now. A no-op when it is already held. */
  focus(): void
  /** Give it up on unmount. The editor under this one gets it back. */
  release(): void
}

interface Claim {
  path: string
  line: number
  column: number
  lines: number
}

/**
 * Newest last, so the top of the stack is the editor the user is in.
 *
 * A plain array rather than a `Map`: two panes can show the *same* path, so a path is not a key.
 * Identity is the claim object itself, which is what `release` removes.
 */
const claims: Claim[] = []

/** The caret of the editor the user is in, or `null` when no editor holds the slot. */
export function focusedCaret(): CaretPosition | null {
  const top = claims.at(-1)
  if (top === undefined) return null
  return { path: top.path, line: top.line, column: top.column, lines: top.lines }
}

/**
 * Claim the slot for an editor showing `path`.
 *
 * Mounting claims it, because a freshly opened file is the one being looked at — the same rule
 * `claimStatusReadout` follows, and the two are claimed and released together.
 */
export function claimCaret(path: string): CaretSlot {
  // `lines: 1` rather than 0, because "an empty document is one empty line" is CodeMirror's own
  // arithmetic and this value is read before the first `set` arrives. A 0 here would let Go to
  // line report a file with no lines in it for one frame after a split opens.
  const claim: Claim = { path, line: 1, column: 1, lines: 1 }
  claims.push(claim)

  let released = false
  return {
    set(line, column, lines) {
      // Written whether or not this claim is on top: an editor the user is not in still knows
      // where its own caret is, and it becomes the answer the moment `focus` is called. Guarding
      // on `released` rather than on position in the stack is what makes that true without a
      // scan per keystroke.
      if (released) return
      claim.line = line
      claim.column = column
      claim.lines = lines
    },
    focus() {
      if (released) return
      if (claims.at(-1) === claim) return
      const at = claims.indexOf(claim)
      if (at === -1) return
      claims.splice(at, 1)
      claims.push(claim)
    },
    release() {
      if (released) return
      released = true
      const at = claims.indexOf(claim)
      if (at !== -1) claims.splice(at, 1)
    },
  }
}

/** Testing seam: forget every claim. Never called by the app. */
export function resetCaretsForTest(): void {
  claims.length = 0
}
