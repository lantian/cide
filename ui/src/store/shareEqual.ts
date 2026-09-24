/**
 * Structural sharing: `next`, except that every subtree deep-equal to the one at the same place
 * in `prev` is `prev`'s object. The shape react-query calls `replaceEqualDeep`.
 *
 * # Why it exists
 *
 * Rust owns the state and ships it whole: `cide://workspace-changed` carries the entire
 * `Workspace`, a git log refresh the entire page, a diagnostics fetch the entire snapshot. Parsed
 * off the wire, every object in each of them is new — including the ninety-nine percent that did
 * not change. React compares props and selector results with `Object.is`, so a fresh-but-equal
 * object is indistinguishable from a real change, and `memo` and every dependency array keyed on
 * a project, a tab, a pane or a row fired on every message. That was the whole-app re-render on a
 * focus click.
 *
 * Passed through this, an unchanged subtree keeps its identity and a changed one gets a new one
 * all the way up to the root, so identity means what a reader assumes it means: *this changed*.
 * Nothing is mutated, and the result is always deep-equal to `next` — the data is `next`'s; only
 * which object holds it is decided here.
 *
 * # What it walks
 *
 * Plain objects and arrays, the only shapes `JSON.parse` produces. Anything else — a class
 * instance, a `Map`, a function — is compared by identity and otherwise taken from `next`, which
 * is the conservative answer: sharing is an optimisation, and declining it is never wrong.
 * Arrays are matched by index; a caller whose items move (a list with a row prepended) matches
 * them by key first and calls this per item.
 *
 * Import-free on purpose: `ui/scripts/check-share.mjs` compiles this one file standalone and
 * executes it.
 */
export function shareEqual<T>(prev: unknown, next: T): T {
  if (Object.is(prev, next)) return next
  if (Array.isArray(prev) && Array.isArray(next)) {
    let same = prev.length === next.length
    const out = next.map((item, i) => {
      const shared = i < prev.length ? shareEqual(prev[i], item) : item
      if (shared !== prev[i]) same = false
      return shared
    })
    return (same ? prev : out) as T
  }
  if (isPlain(prev) && isPlain(next)) {
    const prevKeys = Object.keys(prev)
    const nextKeys = Object.keys(next)
    let same = prevKeys.length === nextKeys.length
    const out: Record<string, unknown> = {}
    for (const key of nextKeys) {
      const shared = Object.prototype.hasOwnProperty.call(prev, key)
        ? shareEqual(prev[key], next[key])
        : next[key]
      if (!Object.prototype.hasOwnProperty.call(prev, key) || shared !== prev[key]) same = false
      out[key] = shared
    }
    return (same ? prev : out) as T
  }
  return next
}

function isPlain(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== 'object') return false
  const proto = Object.getPrototypeOf(value)
  return proto === Object.prototype || proto === null
}
