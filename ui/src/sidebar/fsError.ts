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
  // word, which beats `[object Object]` by the whole distance that matters.
  if (typeof tagged.kind === 'string' && tagged.kind.length > 0) return tagged.kind
  return String(error)
}

function countOf(value: unknown): number | null {
  return Array.isArray(value) ? value.length : null
}
