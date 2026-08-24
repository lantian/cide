/**
 * Turning "here is the whole formatted file" into the smallest edit that produces it. (M26)
 *
 * # Why this exists at all
 *
 * `format_document` answers with the entire buffer as it should now read, for the reasons
 * `cide_ipc::FormatAnswer` gives. Dispatching that as `{ from: 0, to: doc.length, insert: text }`
 * is one line and is wrong in four ways at once, none of which throws:
 *
 * * **The caret jumps.** CodeMirror maps every position through a change, and a change spanning
 *   the document maps every position in it to the same end. Reformatting while the caret is on
 *   line 400 puts it at the bottom of the file.
 * * **The scroll jumps**, for the same reason and at the same moment.
 * * **Undo becomes all-or-nothing.** One transaction replacing everything is one undo step that
 *   restores everything, so Ctrl+Z after a format-then-type loses the typing too.
 * * **Every decoration is rebuilt** — diagnostics, folds, blame, search matches — because none
 *   of their positions survived.
 *
 * Trimmed to what actually differs, a format that changed three lines *is* a three-line change:
 * positions outside it map to themselves, so the caret, the scroll and the folds stay where they
 * were, and the undo step is the size of the edit.
 *
 * # Why it imports nothing
 *
 * `ui/scripts/check-format.mjs` compiles this module alone with a bare `tsc` and runs it under
 * node. Nothing here may reach for CodeMirror, the store, or the IPC client — the arithmetic is
 * the part worth testing, and it is testable only while it stays arithmetic.
 */

/** A CodeMirror change spec, in the shape `EditorView.dispatch` takes. */
export interface MinimalChange {
  /** Offset of the first differing UTF-16 code unit. */
  readonly from: number
  /** Offset just past the last differing code unit, in the *old* text. */
  readonly to: number
  /** What replaces `[from, to)`. */
  readonly insert: string
}

/**
 * Is this code unit the first half of a surrogate pair?
 *
 * JavaScript strings are UTF-16, and a non-BMP character — an emoji, a rare CJK ideograph, a
 * mathematical symbol — is two code units. Cutting between them produces a lone surrogate, which
 * is not a character, renders as a replacement glyph, and would be inserted into the user's file
 * as one. Every boundary this module chooses is nudged off such a cut; see [`minimalChange`].
 */
function isHighSurrogate(text: string, at: number): boolean {
  const code = text.charCodeAt(at)
  return code >= 0xd800 && code <= 0xdbff
}

/**
 * The smallest single replacement that turns `before` into `after`, or `null` when they are
 * already equal.
 *
 * `null` is a real answer and the common one: a reflexive Ctrl+Alt+F on an already-formatted
 * file must dispatch **no transaction at all**, or it dirties the tab and pushes an undo step
 * for an edit that changed nothing.
 *
 * # What this is and is not
 *
 * One replacement spanning everything between the first and last difference — a common-prefix,
 * common-suffix trim and nothing cleverer. It is *not* a diff: a formatter that changes line 2
 * and line 900 produces one change covering both and the 898 lines between them, and positions
 * inside that span still move.
 *
 * That is deliberate. A real diff would keep more positions stable, and it would also have to be
 * a real diff — Myers, with its own tests and its own performance story — to buy a difference the
 * user notices only when a formatter rewrites two distant regions and leaves the middle alone,
 * which is not what formatters do. Formatters change indentation everywhere or a little
 * somewhere. The trim handles both ends of that range and costs two scans.
 */
export function minimalChange(before: string, after: string): MinimalChange | null {
  if (before === after) return null

  const max = Math.min(before.length, after.length)

  let prefix = 0
  while (prefix < max && before.charCodeAt(prefix) === after.charCodeAt(prefix)) prefix += 1
  // Never cut a surrogate pair in half. Backing off by one is always safe: the code unit at
  // `prefix - 1` matched, so stepping back onto it keeps the two texts equal up to the new
  // boundary and the pair is left whole on both sides.
  if (prefix > 0 && isHighSurrogate(before, prefix - 1)) prefix -= 1

  // The suffix may not overlap the prefix in *either* text, or the change would be inside out:
  // `to` would land before `from`, and `insert` would be a backwards slice (silently empty).
  // Reachable with an ordinary edit — deleting a repeated line, where the text after the
  // deletion matches the text before it.
  const room = max - prefix
  let suffix = 0
  while (
    suffix < room &&
    before.charCodeAt(before.length - 1 - suffix) === after.charCodeAt(after.length - 1 - suffix)
  ) {
    suffix += 1
  }
  // Same surrogate guard at the other end. Here the unit that matched is the *low* half, so the
  // cut to avoid is one whose left neighbour is a high surrogate.
  const cut = before.length - suffix
  if (suffix > 0 && cut > 0 && isHighSurrogate(before, cut - 1)) suffix -= 1

  return {
    from: prefix,
    to: before.length - suffix,
    insert: after.slice(prefix, after.length - suffix),
  }
}

/**
 * Should the answer still be applied?
 *
 * The round trip is asynchronous — a flush, an IPC hop, a language server or a child process,
 * and back — and the user's hands are on the keyboard the whole time. If a single character was
 * typed while the formatter ran, the returned text describes a buffer that no longer exists, and
 * applying it **silently discards that keystroke**: the edit would be computed against the old
 * text and land on the new one.
 *
 * Comparing the whole string rather than tracking a version counter, deliberately. It is one
 * comparison of two strings that are usually identical and share nothing else to get wrong —
 * where a counter would need to be incremented at every site that can change the document,
 * including the ones added later, and the failure of a missed increment is the corruption this
 * function exists to prevent.
 */
export function isStillCurrent(sent: string, live: string): boolean {
  return sent === live
}
