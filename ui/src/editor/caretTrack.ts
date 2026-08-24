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
  word: WordReader | null
  folds: FoldActions | null
  format: FormatActions | null
}

/**
 * Reads the identifier the caret is sitting in, on demand.
 *
 * **A getter and not a field**, and that is the whole point: the word could ride along on every
 * `set` like the line and column do, but `set` runs on every selection change — thirty times a
 * second under a held arrow key, on the path this module's header goes out of its way to keep off
 * React. Extracting a word per keystroke to answer a question asked once, when ⌥F7 fires, is the
 * wrong way round.
 *
 * `null` when the caret is not in a word — whitespace, punctuation, an empty line.
 */
export type WordReader = () => string | null

/**
 * The folding commands of the editor that holds the slot.
 *
 * # Why these ride on the caret slot rather than on a registry of their own
 *
 * Because the question they answer is the one this module already answers: *which editor is the
 * user in*. `keys/dispatch.ts` fires `editor.fold` with no `EditorView` and no way to get one —
 * the same predicament [`focusedCaret`] exists for — and a second module-level stack would be a
 * second answer to that question, kept in step by hand. The note above `CaretPosition.lines`
 * makes exactly this argument about a field, and it applies harder to a whole registry.
 *
 * `openBuffers.ts` is the other shape available and it is the wrong one here: it is keyed by
 * *tab*, which is right for Ctrl+S (a save means a tab, whether or not it is focused) and wrong
 * for a fold (a split shows two editors of the same file and only one of them is being read).
 *
 * Each returns whether it did anything, so a command that finds nothing to collapse can say so
 * through `unmet(...)` rather than being a keystroke that silently did nothing — which is the
 * failure `cide_core::commands`' header is written against.
 */
export interface FoldActions {
  fold(): boolean
  unfold(): boolean
  toggle(): boolean
  foldAll(): boolean
  unfoldAll(): boolean
  foldRecursively(): boolean
  unfoldRecursively(): boolean
}

/**
 * Reading and rewriting the text of the editor that holds the slot. (M26)
 *
 * Rides here for the reason [`FoldActions`] gives at length — the question *which editor is the
 * user in* has one answer and this module is it — and the argument is sharper for formatting
 * than for folding. `openBuffers.ts` is the other shape available and it is keyed by **tab**,
 * which is right for Ctrl+S (a save means a tab, focused or not) and wrong here: a split showing
 * one file twice has two selections, and Ctrl+Alt+F is about the one the user is looking at.
 *
 * # Why this hands out functions rather than the text
 *
 * The same narrowing `EditorSurface`'s `onSaveHandle` documents. `keys/dispatch.ts` has no
 * `EditorView` and cannot get one — pane DOM lives in a module-level map outside React precisely
 * so nothing above it holds a reference — so it must be given capabilities, not a document. What
 * it gets is exactly enough to format: read the text, read the selection, refuse if read-only,
 * put text back.
 */
export interface FormatActions {
  /** The buffer's path, so the caller can flush the right document's pending `didChange`. */
  path(): string
  /**
   * The text, in the units the language server has it — LF, whatever the file's real endings
   * are. `docSync` sends exactly this, so an edit computed against it lands correctly here; the
   * file's own endings are restored on save and never round-tripped through a formatter.
   */
  text(): string
  /**
   * The selection as 1-based lines and 1-based UTF-16 columns, or `null` when it is empty.
   *
   * `null` for an empty selection and not a zero-width range, because the two mean different
   * things to the caller: a caret is "format the document", a range is "format this much".
   */
  selection(): { startLine: number; startColumn: number; endLine: number; endColumn: number } | null
  /** Whether this buffer refuses edits — a diff pane, a file with no write permission. */
  readOnly(): boolean
  /**
   * Replace the text, preserving as much as the change allows.
   *
   * Returns `false` when the buffer has moved since `text()` was read, which the caller must
   * treat as "abandon" rather than "retry": the answer describes a document that no longer
   * exists, and applying it would discard whatever was typed in the meantime.
   */
  apply(from: string, to: string): boolean
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
 * The identifier the caret is in, or `null`.
 *
 * Find usages is the caller. ⌥F7 fires from `keys/dispatch.ts`, which has no `EditorView` and
 * cannot get one — the same predicament [`focusedCaret`] exists for — and without a name the popup
 * would be headed *"Usages of the symbol"* and the empty-result notice would say *"No usages
 * found"* about nothing in particular. That is the difference between a report and a shrug.
 *
 * `null` is a legitimate answer and every caller must render it: a detached window holds no editor,
 * and a caret on punctuation is in no word.
 */
export function focusedWord(): string | null {
  const top = claims.at(-1)
  if (top === undefined || top.word === null) return null
  return top.word()
}

/**
 * The folding commands of the editor the user is in, or `null`.
 *
 * `null` means "no editor holds the slot" — a detached terminal window, a Claude tab — and every
 * caller has to render it, exactly as they already do for [`focusedCaret`]. It is *also* null for
 * a claim made without actions, which is what a fixture or a check script produces.
 */
export function focusedFolds(): FoldActions | null {
  const top = claims.at(-1)
  return top === undefined ? null : top.folds
}

/**
 * The text actions of the editor the user is in, or `null`.
 *
 * `null` means "no editor holds the slot" — a detached terminal window, a Claude tab — exactly as
 * for [`focusedFolds`], and every caller has to render it. It is *also* null for a claim made
 * without actions, which is what a fixture or a check script produces.
 */
export function focusedFormat(): FormatActions | null {
  const top = claims.at(-1)
  return top === undefined ? null : top.format
}

/**
 * Claim the slot for an editor showing `path`.
 *
 * Mounting claims it, because a freshly opened file is the one being looked at — the same rule
 * `claimStatusReadout` follows, and the two are claimed and released together.
 *
 * `word`, `folds` and `format` are optional so a caller that has no cheap way to supply them — a
 * fixture, a check script — still gets a working claim; [`focusedWord`], [`focusedFolds`] and
 * [`focusedFormat`] then answer `null`, which every caller already has to handle.
 */
export function claimCaret(
  path: string,
  word?: WordReader,
  folds?: FoldActions,
  format?: FormatActions,
): CaretSlot {
  // `lines: 1` rather than 0, because "an empty document is one empty line" is CodeMirror's own
  // arithmetic and this value is read before the first `set` arrives. A 0 here would let Go to
  // line report a file with no lines in it for one frame after a split opens.
  const claim: Claim = {
    path,
    line: 1,
    column: 1,
    lines: 1,
    word: word ?? null,
    folds: folds ?? null,
    format: format ?? null,
  }
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
