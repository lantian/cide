/**
 * The hyperlink a rendered JSON log line carries, and what it means.
 *
 * # Why a URI at all
 *
 * `cide_core::jsonlog` replaces a structured log line with a one-line summary, upstream of the
 * screen mirror — so the original object is in no buffer anywhere, and the summary is lossy in
 * the direction people care about: a nested value arrives as compact JSON. `cide-app` keeps the
 * raw line in a bounded per-session ring and marks the rendering with an OSC 8 hyperlink whose
 * URI is the handle that finds it again. This module is the parser for that URI, and the only
 * place in the frontend that knows its shape.
 *
 * OSC 8 rather than a link provider of cide's own, for three reasons that are each a bug
 * avoided: the marker occupies no cells, so a copied line does not contain it; xterm reassembles
 * it across a wrapped line without help; and the handle is an *identity*, so two events that
 * render to identical characters — a `tick` a second apart — do not have to be told apart by
 * their text, which is the only thing a provider matching on the buffer could have done.
 *
 * # Import-free on purpose
 *
 * `ui/scripts/check-json-log.mjs` compiles this module standalone with the TypeScript in
 * `node_modules` and drives it. An import here would end that, and the parsing below is
 * exactly the kind of string handling that is wrong in a way nobody sees: a URI that fails to
 * parse is a click that does nothing at all.
 */

/**
 * The scheme, spelled here and in `cide_app::lifecycle::LOG_LINK_SCHEME`.
 *
 * Two spellings because the two sides cannot import one another, and `check:json-log` greps
 * the Rust constant to hold them together. It is not a registered scheme and never reaches a
 * browser: xterm's OSC link provider hands every URI to cide's own `linkHandler`, which opens
 * nothing it does not recognise.
 */
export const LOG_LINK_SCHEME = 'cide-log'

export interface LogLinkTarget {
  /** The session whose ring holds the line — the pane's, but carried so no lookup is needed. */
  readonly session: string
  /** Which line. A sequence number from that ring, never reused. */
  readonly handle: number
}

/**
 * Read a `cide-log:<session>:<handle>` URI, or `null` for anything else.
 *
 * Everything is checked rather than assumed, because this runs on whatever a *program's
 * output* put on the screen: a child that emits its own OSC 8 sequence can name any URI it
 * likes, including one wearing this scheme. The worst a forged one can do is name a handle
 * that is not in the ring — the answer is then "no longer kept" — but it must not be able to
 * make this return a `NaN` handle or a session id that is really a path.
 */
export function parseLogLink(uri: string): LogLinkTarget | null {
  const prefix = `${LOG_LINK_SCHEME}:`
  if (!uri.startsWith(prefix)) return null
  const rest = uri.slice(prefix.length)
  const split = rest.lastIndexOf(':')
  if (split <= 0 || split === rest.length - 1) return null
  const session = rest.slice(0, split)
  const digits = rest.slice(split + 1)
  // `Number` alone would take `' 3'`, `'0x2'`, `'1e3'` and `''`. A handle is decimal digits.
  if (!/^\d+$/.test(digits)) return null
  // The session is a uuid and nothing else may pass: this string is sent straight to a command.
  if (!/^[0-9a-fA-F-]{36}$/.test(session)) return null
  const handle = Number(digits)
  if (!Number.isSafeInteger(handle)) return null
  return { session, handle }
}

/** The URI Rust writes, spelled once here so the check can drive both directions. */
export function formatLogLink(session: string, handle: number): string {
  return `${LOG_LINK_SCHEME}:${session}:${handle}`
}

/** Whether a URI is one of ours at all — the branch `xterm.ts`'s `linkHandler` takes. */
export function isLogLink(uri: string): boolean {
  return parseLogLink(uri) !== null
}
