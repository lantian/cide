/**
 * Turning a rejected `invoke` into a sentence a person can read. (M24)
 *
 * # Why `String(e)` is not enough, and was wrong on screen
 *
 * `CoreError` is a **tagged** enum — `#[serde(tag = "kind", content = "detail")]` — so a rejected
 * command arrives as an object, `{ kind: 'serde', detail: '…' }`. `String()` of an object is
 * `"[object Object]"`, and that is what the Settings screen showed when an import failed: a note
 * headed *"Could not import that file"* whose body said nothing at all. The sentence Rust had
 * gone to the trouble of composing — which named the file, what was looked for inside it, and
 * what the file turned out to be — never reached anybody.
 *
 * It is the shape of mistake that survives review because the happy path is the one that gets
 * looked at, and the failing path is only ever seen by the person it is failing.
 *
 * # What this does not do
 *
 * It does not map `kind` to prose. `CoreError`'s `#[error(...)]` strings live in Rust and are not
 * serialised, and restating them here would be a second copy that drifts. The variants a user is
 * shown a message for (`Io`, `Serde`, `Invariant`) all carry their sentence in `detail`; the ones
 * that do not — `TabPinned`, `LastTab` — are branched on by the UI rather than printed, which is
 * the whole reason they are tags. So: the detail when there is one, the tag when there is not,
 * and `String()` for anything that is not a `CoreError` at all.
 *
 * Deliberately import-free, so `check:scheme` can compile it standalone.
 */

/** The sentence to show for a rejected `invoke`, whatever it rejected with. */
export function errorText(error: unknown): string {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  if (error !== null && typeof error === 'object') {
    const tagged = error as { kind?: unknown; detail?: unknown }
    if (typeof tagged.detail === 'string' && tagged.detail.length > 0) return tagged.detail
    // A tag with no payload. Better than `[object Object]`, and it is a name a bug report can
    // carry back to the variant that produced it.
    if (typeof tagged.kind === 'string' && tagged.kind.length > 0) return tagged.kind
  }
  return String(error)
}
