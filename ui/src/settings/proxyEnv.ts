/**
 * The proxy environment a child would be spawned with — the TypeScript half of a mirror.
 *
 * The authority is Rust: `proxy_env`, `no_proxy_value`, `normalize_proxy_url` and
 * `redact_proxy_url` decide what a child actually gets, and this file only decides what the
 * Settings screen *says* it gets. A divergence therefore shows a wrong label and never hands
 * a child a wrong environment — but a wrong label on the one readout a user opens when a pane
 * cannot reach the network is its own bug, and "small enough to keep in step by hand" is what
 * every drifted mirror was described as on the day it was written.
 *
 * So it lives here rather than inside `ProxySection.tsx`, and it is **import-free on purpose**
 * — same reason as `theme.ts`. `ui/scripts/check-proxy.mjs` compiles this module on its own
 * with the TypeScript in `node_modules`, runs it against the cases the Rust tests assert, and
 * reads `LOOPBACK_EXEMPT` and `PROXY_URL_VARS` straight out of `cmd/session.rs` to prove the
 * two lists are still the same list. Importing `@/ipc/client` for `ProxySettings` would end
 * that, which is why the wire shape is restated structurally below.
 */

/** Mirrors `ProxyMode`. Checked against `crates/cide-ipc/bindings/ProxyMode.ts`. */
export type ProxyModeName = 'inherit' | 'manual' | 'direct'

/**
 * The fields of `ProxySettings`, restated so this module imports nothing.
 *
 * Not a copy that can rot unnoticed: `ProxySection` passes the real wire type into
 * [`childEnvironment`], so a field renamed or dropped in Rust fails to compile at that call
 * rather than here. A field *added* is accepted, which is the right asymmetry — a new
 * variable cide does not display yet is not a lie about the ones it does.
 */
export interface ProxyValues {
  mode: ProxyModeName
  http: string
  https: string
  all: string
  noProxy: string
}

/**
 * The loopback hosts cide puts in `NO_PROXY` and will not let you remove.
 *
 * Kept in step with `LOOPBACK_EXEMPT` in `crates/cide-app/src/cmd/session.rs`, by the check
 * script rather than by hope. The IDE integration is an MCP server on loopback; a proxy that
 * swallows it takes inline diffs and @-mentions with it, silently.
 */
export const LOOPBACK: readonly string[] = ['localhost', '127.0.0.1', '::1']

/** Kept in step with `PROXY_URL_VARS`. Only the upper-case name — see [`childEnvironment`]. */
export const PROXY_URL_VARS: readonly string[] = ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY']

/** One environment line in the readout, or one being removed. */
export interface EnvLine {
  name: string
  /** `null` when the variable is removed from the child rather than set. */
  value: string | null
}

/**
 * The variables a child would be spawned with, as a mirror of `proxy_env`.
 *
 * Only the upper-case name is listed, with the readout's footnote explaining that the
 * lower-case twin is set to the same value. Printing eight rows where four carry the
 * information reads as a bug in the readout rather than as the deliberate redundancy it is.
 *
 * The inherit case cannot be shown honestly from here — it depends on the environment *this
 * process* was launched with, which the webview cannot see — so it returns nothing and the
 * prose beside it says what happens instead.
 */
export function childEnvironment(proxy: ProxyValues): EnvLine[] {
  if (proxy.mode === 'inherit') return []
  if (proxy.mode === 'direct') {
    return [...PROXY_URL_VARS, 'NO_PROXY'].map((name) => ({ name, value: null }))
  }
  const http = normalizeProxyUrl(proxy.http)
  return [
    { name: 'HTTP_PROXY', value: http },
    { name: 'HTTPS_PROXY', value: normalizeProxyUrl(proxy.https) ?? http },
    { name: 'ALL_PROXY', value: normalizeProxyUrl(proxy.all) },
    { name: 'NO_PROXY', value: bypassList(proxy.noProxy) },
  ]
}

/** Mirrors `normalize_proxy_url`: blank is unset, and a bare host:port gains `http://`. */
export function normalizeProxyUrl(raw: string): string | null {
  const trimmed = raw.trim()
  if (trimmed === '') return null
  return trimmed.includes('://') ? trimmed : `http://${trimmed}`
}

/** Mirrors `no_proxy_value`: loopback first, then the user's entries, deduplicated. */
export function bypassList(extra: string): string {
  const entries: string[] = [...LOOPBACK]
  for (const raw of extra.split(',')) {
    const entry = raw.trim()
    if (entry === '') continue
    if (!entries.some((e) => e.toLowerCase() === entry.toLowerCase())) entries.push(entry)
  }
  return entries.join(',')
}

/**
 * Mirrors `redact_proxy_url`: the host survives, the credentials do not.
 *
 * Applied to the readout and not to the input field. The readout is the part of this screen
 * that ends up in a screenshot attached to a ticket; the field is the part you have to be
 * able to correct.
 */
export function redactProxyUrl(url: string): string {
  const at = url.indexOf('://')
  // Nothing to anchor on, so nothing can be shown to be safe.
  if (at < 0) return url.includes('@') ? '***' : url
  const scheme = url.slice(0, at)
  const rest = url.slice(at + 3)
  const slash = rest.indexOf('/')
  const authority = slash < 0 ? rest : rest.slice(0, slash)
  const tail = slash < 0 ? '' : rest.slice(slash)
  // `lastIndexOf`, matching Rust's `rsplit_once`: a password may itself contain an `@`, and
  // splitting on the first one would print the tail of it.
  const marker = authority.lastIndexOf('@')
  if (marker < 0) return url
  return `${scheme}://***@${authority.slice(marker + 1)}${tail}`
}

/** Whether a URL carries userinfo, i.e. whether it can be carrying a password. */
export function hasUserinfo(url: string): boolean {
  const rest = url.split('://')[1] ?? url
  const authority = rest.split('/')[0] ?? ''
  return authority.includes('@')
}
