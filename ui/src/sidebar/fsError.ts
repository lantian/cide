/**
 * A rejected `fs_*` command, as a sentence.
 *
 * `FsError` is `#[serde(tag = "kind", content = "detail")]`, so what arrives at a `.catch` is
 * `{ kind: "exists", detail: "/home/u/p/main.rs already exists" }` — an **object with no
 * `message` field**. Every panel that reported one with `String(error)` was therefore printing
 *
 *     Move to Trash failed: [object Object]
 *
 * which is worse than saying nothing: it looks like a bug in the reporting rather than a
 * refusal with a reason, and the reason it is hiding is one the user can act on ("that name is
 * taken", "that is a project root"). `chrome/Failures.tsx` has the same shape for the *other*
 * error convention in this app — `{ kind, message }` — and deliberately does not know about
 * this one, because it never sees an `FsError`: the tree catches its own rejections so it can
 * show them beside the rows they are about.
 *
 * Pure and import-free, so `check-fs-clipboard.mjs` can hold the shapes.
 */

/** The tagged-enum envelope `FsError` serialises to. Every field optional — this is the thing
 *  that crossed the IPC boundary, so nothing about it can be assumed. */
interface Tagged {
  kind?: unknown
  detail?: unknown
}

/**
 * Turn a rejection into something worth showing next to a file row.
 *
 * The `detail` is preferred over the `kind` because Rust already wrote the sentence: every
 * `FsError` variant carries `#[error(...)]` prose naming the path it is about, and the tag is
 * for code. A struct variant (`Io { path, message }`, `PartialPaste { pasted, error }`) has an
 * object there instead, so its fields are assembled in the same order the `Display` impl would.
 */
export function fsMessage(error: unknown): string {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  if (error === null || typeof error !== 'object') return String(error)

  const tagged = error as Tagged
  const detail = tagged.detail
  if (typeof detail === 'string' && detail.length > 0) return detail
  if (detail !== null && typeof detail === 'object') {
    const inner = detail as Record<string, unknown>
    const path = typeof inner['path'] === 'string' ? inner['path'] : null
    const message = typeof inner['message'] === 'string' ? inner['message'] : null
    if (path !== null && message !== null) return `${path}: ${message}`
    if (message !== null) return message
    // `PartialPaste` / `PartialDelete`: the half that failed, plus how much had already
    // happened — which is the whole reason those variants exist rather than a bare `Io`.
    const nested = typeof inner['error'] === 'string' ? inner['error'] : null
    const done = countOf(inner['pasted']) ?? countOf(inner['trashed'])
    if (nested !== null) {
      return done === null ? nested : `${nested} — ${done} path(s) had already been done`
    }
  }
  // A tag with nothing readable behind it (`noIndex` has no content at all). The tag is a
  // word, which beats `[object Object]` by the whole distance that matters — and for the one
  // unit variant a user meets on purpose there is a sentence instead, because "that gesture
  // found nothing" is a thing to read and `noClipboardImage` is a thing to grep.
  if (typeof tagged.kind === 'string' && tagged.kind.length > 0) {
    return tagged.kind === NO_CLIPBOARD_IMAGE ? 'the clipboard does not hold an image' : tagged.kind
  }
  return String(error)
}

/**
 * `FsError::NoClipboardImage`'s tag. See [`isNoClipboardImage`].
 */
export const NO_CLIPBOARD_IMAGE = 'noClipboardImage'

/**
 * Was this rejection *there is no image on the clipboard*?
 *
 * The one `FsError` a caller routinely wants to **swallow**. `fs_paste_image` is reached by a
 * bare Ctrl+V — in the tree when no file is on the in-app clipboard, in the editor when the
 * paste event says the clipboard holds no text — so a clipboard with text on it, or an empty
 * one, produces this several times a session and none of them is a thing that went wrong. A
 * caller that reached the same command from a menu item the user pointed at shows it instead,
 * which is why this is a predicate and not a silent `catch`.
 *
 * Matched on the tag rather than on the prose, for the reason this whole module exists: prose
 * is not an API and `fsMessage`'s output is for people.
 */
export function isNoClipboardImage(error: unknown): boolean {
  return (
    error !== null &&
    typeof error === 'object' &&
    (error as Tagged).kind === NO_CLIPBOARD_IMAGE
  )
}

function countOf(value: unknown): number | null {
  return Array.isArray(value) ? value.length : null
}
