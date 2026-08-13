/**
 * The proxy environment a child would be spawned with — the TypeScript half of a mirror.
 *
 * The authority is Rust: `cide_core::proxy::ProxyEnv::resolve`, `no_proxy_value`,
 * `normalize_proxy_url` and `redact_proxy_url` decide what a child actually gets, and this
 * file only decides what the Settings screen *says* it gets. A divergence therefore shows a wrong label and never hands
 * a child a wrong environment — but a wrong label on the one readout a user opens when a pane
 * cannot reach the network is its own bug, and "small enough to keep in step by hand" is what
 * every drifted mirror was described as on the day it was written.
 *
 * So it lives here rather than inside `ProxySection.tsx`, and it is **import-free on purpose**
 * — same reason as `theme.ts`. `ui/scripts/check-proxy.mjs` compiles this module on its own
 * with the TypeScript in `node_modules`, runs it against the cases the Rust tests assert, and
 * reads `LOOPBACK_EXEMPT` and `PROXY_URL_VARS` straight out of `cide-core/src/proxy.rs` — and
 * the target names out of the generated `ProxyTarget.ts` — to prove the lists are still the
 * same lists. Importing `@/ipc/client` for `ProxySettings` would end that, which is why the
 * wire shape is restated structurally below.
 */

/** Mirrors `ProxyMode`. Checked against `crates/cide-ipc/bindings/ProxyMode.ts`. */
export type ProxyModeName = 'inherit' | 'manual' | 'direct'

/**
 * Mirrors `ProxyTarget`. Checked against `crates/cide-ipc/bindings/ProxyTarget.ts`.
 *
 * The three answers to "what does cide do to *this* kind of child's proxy environment", and
 * the middle one is the one worth reading twice. `untouched` is not `direct`: it means cide
 * sets nothing and removes nothing, so the child inherits whatever cide itself was launched
 * with — proxy included. Only `direct` removes.
 */
export type ProxyTargetName = 'configured' | 'untouched' | 'direct'

/** Mirrors `ProxyScope`: which children the settings above reach. */
export interface ProxyScopeValues {
  claude: ProxyTargetName
  shells: ProxyTargetName
  git: ProxyTargetName
}

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
  scope: ProxyScopeValues
  http: string
  https: string
  all: string
  noProxy: string
}

/** The three children the scope answers for, in the order the screen draws them. */
export const TARGETS: readonly { key: keyof ProxyScopeValues; label: string }[] = [
  { key: 'claude', label: 'claude' },
  { key: 'shells', label: 'Shell panes' },
  { key: 'git', label: 'cide’s own Git' },
]

/**
 * The loopback hosts cide puts in `NO_PROXY` and will not let you remove.
 *
 * Kept in step with `LOOPBACK_EXEMPT` in `crates/cide-core/src/proxy.rs`, by the check
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
 * The variables one kind of child would be spawned with, as a mirror of
 * `cide_core::proxy::ProxyEnv::resolve`.
 *
 * **`target` is not optional and that is the point.** The readout used to answer for "a
 * child", back when every child got the same answer. With a scope in play there is no such
 * thing: the same settings give `claude` a proxy and cide's own `git` nothing at all, and a
 * readout that kept answering in the singular would be confidently wrong for two of the
 * three columns. So the caller says which child it is asking about, and the screen draws one
 * readout per target.
 *
 * Only the upper-case name is listed, with the readout's footnote explaining that the
 * lower-case twin is set to the same value. Printing eight rows where four carry the
 * information reads as a bug in the readout rather than as the deliberate redundancy it is.
 *
 * The inherit case cannot be shown honestly from here — it depends on the environment *this
 * process* was launched with, which the webview cannot see — so it returns nothing and the
 * prose beside it says what happens instead. `untouched` returns nothing for a different
 * reason, and the two are told apart by [`readoutKind`] rather than by the caller guessing
 * from an empty array.
 */
export function childEnvironment(proxy: ProxyValues, target: ProxyTargetName): EnvLine[] {
  // Nothing set and nothing removed. Deliberately identical in *shape* to the inherit case
  // and different in meaning; see `readoutKind`.
  if (target === 'untouched') return []
  if (target === 'direct') {
    return [...PROXY_URL_VARS, 'NO_PROXY'].map((name) => ({ name, value: null }))
  }
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

/**
 * Why a readout is empty — the distinction the whole scope feature turns on.
 *
 * `childEnvironment` returns `[]` for two completely different reasons, and a screen that
 * drew the same sentence for both would be telling a corporate user that cide had taken their
 * `git push` off the proxy when it had done nothing of the kind:
 *
 * * `'inherited'` — cide is deferring, but it *is* in scope, so the loopback bypass is still
 *   merged in when something was inherited. The IDE integration keeps working.
 * * `'untouched'` — this child is out of scope entirely. cide adds nothing, removes nothing,
 *   and does not even rescue loopback. Whatever cide itself was launched with, including a
 *   proxy from a login profile, reaches it unchanged.
 *
 * A rule, in a pure module, rather than a ternary inside the component: it is exactly the
 * kind of two-branch sentence that reads as obvious and ships inverted.
 */
export function readoutKind(
  proxy: ProxyValues,
  target: ProxyTargetName,
): 'lines' | 'inherited' | 'untouched' {
  if (target === 'untouched') return 'untouched'
  if (target === 'configured' && proxy.mode === 'inherit') return 'inherited'
  return 'lines'
}

/**
 * Whether this configuration is the one the request behind the scope asked for: a proxy for
 * `claude`, and cide's own Git left on the network it was already on.
 *
 * Used only to confirm the shape on screen, so a user who has assembled it can see that they
 * have. It is deliberately *not* a preset button: three independent controls that also have a
 * fourth control setting them is a UI where the fourth one silently disagrees with the three.
 */
export function isClaudeOnly(proxy: ProxyValues): boolean {
  return (
    proxy.mode === 'manual' &&
    proxy.scope.claude === 'configured' &&
    proxy.scope.shells !== 'configured' &&
    proxy.scope.git !== 'configured'
  )
}

/**
 * What the "claude only" confirmation says about the other two children.
 *
 * A sentence rather than a constant, because {@link isClaudeOnly} is satisfied by `untouched`
 * *and* by `direct`, and those are opposites. The note used to read "Your shell panes and the
 * Git tool window are on whatever network cide itself is on" in both cases — true of
 * `untouched`, and the exact reverse of `direct`, which strips every proxy variable so a child
 * goes direct *even when cide was started with one exported*. Keeping that distinction is the
 * entire reason `ProxyTargetName` has three values instead of a boolean, so the one summary
 * line on the screen must not be the place it collapses.
 *
 * Returned as text rather than rendered here so `check-proxy.mjs` can run it: this is the kind
 * of sentence that goes quietly wrong when a fourth target state is added.
 */
export function claudeOnlyNote(proxy: ProxyValues): string {
  const untouched = 'left on whatever network cide itself is on'
  const stripped = 'given no proxy at all — every proxy variable is removed, even one cide '
    + 'itself was started with'
  const shells = proxy.scope.shells === 'direct' ? stripped : untouched
  const git = proxy.scope.git === 'direct' ? stripped : untouched
  if (shells === git) return `Your shell panes and the Git tool window are ${shells}.`
  return `Your shell panes are ${shells}. The Git tool window is ${git}.`
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
