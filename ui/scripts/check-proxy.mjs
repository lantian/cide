/**
 * Checks `src/settings/proxyEnv.ts` — the Settings screen's account of the proxy environment
 * a child is spawned with — against the Rust that actually spawns it.
 *
 * This exists because that module is a *mirror*, and a mirror is the one kind of code that
 * rots without anything failing. `proxy_env` in `crates/cide-app/src/cmd/session.rs` decides
 * what a child gets; `childEnvironment` here decides what the screen says a child gets. The
 * Rust half has eleven tests over it. The TypeScript half had none, and the readout it feeds
 * is the thing a user opens precisely when a pane cannot reach the network — the moment when
 * being told a confident wrong answer is worse than being told nothing.
 *
 * Two kinds of proof, because the failure has two shapes:
 *
 *   - The cases. The same inputs the Rust tests assert on, asserted here, so the two
 *     implementations are shown to agree on the answers rather than on the prose.
 *   - The constants. `LOOPBACK` and `PROXY_URL_VARS` are read straight out of `session.rs`
 *     and compared, and the mode names out of the generated `ProxyMode.ts`. A list that
 *     gains an entry on one side only is exactly the drift the comments promise cannot
 *     happen, and it is invisible to the cases above.
 *
 * Same shape as `check-theme.mjs` and `check-exit-marker.mjs`: `proxyEnv.ts` is import-free
 * on purpose, so the TypeScript in `node_modules` can compile it on its own.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that the environment reaches a child. That is `PtySession::spawn` and the Rust suite's.
 *   - that a request actually traverses a proxy. Nothing in this repo talks to one.
 *   - anything about `ProxySection.tsx`'s markup beyond its still importing this module.
 *
 * Run: `pnpm --dir ui run check:proxy`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-proxy-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const rustSource = (rel) => readFileSync(fileURLToPath(new URL(`../../${rel}`, import.meta.url)), 'utf8')

/** The string literals of a `const NAME: [&str; N] = [...]` in a Rust source. */
function rustStrArray(source, name) {
  const decl = new RegExp(`const ${name}: \\[&str; \\d+\\] = \\[([^\\]]*)\\];`).exec(source)
  if (decl === null) return null
  return [...decl[1].matchAll(/"([^"]*)"/g)].map((m) => m[1])
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/settings/proxyEnv.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The project sets it, and this module is written for it.
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const {
    LOOPBACK,
    PROXY_URL_VARS,
    childEnvironment,
    normalizeProxyUrl,
    bypassList,
    redactProxyUrl,
    hasUserinfo,
  } = await import(`file://${join(out, 'proxyEnv.js')}`)

  const manual = (http = '', https = '', all = '', noProxy = '') => ({
    mode: 'manual',
    http,
    https,
    all,
    noProxy,
  })
  const values = (proxy) =>
    Object.fromEntries(childEnvironment(proxy).map((line) => [line.name, line.value]))

  // --- the constants are the same constants ---------------------------------------------
  //
  // Read out of the Rust rather than restated here, so this check cannot drift in the same
  // edit that makes the mirror drift.

  const session = rustSource('crates/cide-app/src/cmd/session.rs')
  eq(rustStrArray(session, 'LOOPBACK_EXEMPT'), [...LOOPBACK], 'LOOPBACK matches LOOPBACK_EXEMPT')
  eq(
    rustStrArray(session, 'PROXY_URL_VARS'),
    [...PROXY_URL_VARS],
    'PROXY_URL_VARS matches the Rust list',
  )

  // The wire enum, so a fourth mode added in Rust cannot leave this module silently
  // returning the `manual` branch for it.
  const modeBinding = rustSource('crates/cide-ipc/bindings/ProxyMode.ts')
  eq(
    [...modeBinding.matchAll(/"([a-z]+)"/g)].map((m) => m[1]),
    ['inherit', 'manual', 'direct'],
    'ProxyMode is the three modes this module branches on',
  )

  // --- normalisation ---------------------------------------------------------------------
  //
  // Mirrors `a_bare_host_and_port_gains_the_scheme_node_insists_on`. curl accepts a bare
  // host:port and Node's `new URL()` throws on it, and the Claude CLI is Node.

  eq(normalizeProxyUrl(' proxy.corp:3128 '), 'http://proxy.corp:3128', 'a bare host:port gains http://')
  eq(normalizeProxyUrl('socks5h://localhost:9050'), 'socks5h://localhost:9050', 'a scheme is left alone')
  eq(normalizeProxyUrl('   '), null, 'blank is unset')

  // --- the bypass list ---------------------------------------------------------------------
  //
  // Mirrors `loopback_is_exempt_and_cannot_be_configured_away` and
  // `the_exemption_list_does_not_repeat_what_the_user_already_wrote`.

  eq(bypassList(''), 'localhost,127.0.0.1,::1', 'loopback alone when nothing was added')
  eq(
    bypassList('corp.internal, .example.com'),
    'localhost,127.0.0.1,::1,corp.internal,.example.com',
    'loopback first, then the user’s entries',
  )
  eq(bypassList('LocalHost,corp'), 'localhost,127.0.0.1,::1,corp', 'deduplicated case-insensitively')
  eq(bypassList(' , ,corp, '), 'localhost,127.0.0.1,::1,corp', 'empty entries are dropped')

  // --- what the readout says a child gets --------------------------------------------------

  eq(
    values(manual('http://proxy.corp:3128', 'http://tls.corp:3129', 'socks5://socks.corp:1080')),
    {
      HTTP_PROXY: 'http://proxy.corp:3128',
      HTTPS_PROXY: 'http://tls.corp:3129',
      ALL_PROXY: 'socks5://socks.corp:1080',
      NO_PROXY: 'localhost,127.0.0.1,::1',
    },
    'manual: every variable from one configuration',
  )

  // Mirrors `https_defaults_to_the_http_proxy_and_all_proxy_does_not`: one proxy typed once
  // reaches HTTPS too, and ALL_PROXY is *not* filled in behind the user's back.
  eq(
    values(manual('proxy.corp:3128')),
    {
      HTTP_PROXY: 'http://proxy.corp:3128',
      HTTPS_PROXY: 'http://proxy.corp:3128',
      ALL_PROXY: null,
      NO_PROXY: 'localhost,127.0.0.1,::1',
    },
    'manual: https falls back to http, all_proxy does not',
  )

  // Mirrors `direct_scrubs_every_spelling`. `null` is the readout's "removed from the child",
  // and every name has to carry it — a variable missing from this list reads as untouched.
  eq(
    values({ mode: 'direct', http: 'http://ignored:1', https: '', all: '', noProxy: 'corp' }),
    { HTTP_PROXY: null, HTTPS_PROXY: null, ALL_PROXY: null, NO_PROXY: null },
    'direct: all four removed, and the stored values ignored',
  )

  // Mirrors `the_no_proxy_default_touches_nothing`: inherit cannot be shown honestly from a
  // webview that cannot read this process's environment, so it shows nothing at all.
  eq(
    childEnvironment({ mode: 'inherit', http: 'http://x:1', https: '', all: '', noProxy: 'corp' }),
    [],
    'inherit: no rows, because the answer is not knowable here',
  )

  // --- redaction ---------------------------------------------------------------------------
  //
  // Mirrors `redaction_keeps_the_host_and_drops_the_userinfo`. The host survives because it
  // is the one question the readout is there to answer.

  eq(redactProxyUrl('http://u:p@proxy.corp:3128'), 'http://***@proxy.corp:3128', 'userinfo dropped')
  eq(
    redactProxyUrl('http://u:p@ss@proxy.corp:3128'),
    'http://***@proxy.corp:3128',
    'an @ inside the password does not put part of it on screen',
  )
  eq(redactProxyUrl('http://proxy.corp:3128'), 'http://proxy.corp:3128', 'nothing to hide')
  eq(redactProxyUrl(''), '', 'empty')
  eq(redactProxyUrl('u:p@proxy.corp:3128'), '***', 'unparseable and carrying an @: redacted whole')
  eq(
    redactProxyUrl('http://u:p@proxy.corp:3128/pac'),
    'http://***@proxy.corp:3128/pac',
    'a path after the authority is kept and is not mistaken for userinfo',
  )

  // The readout runs redaction over the *normalised* value, which is what makes a
  // schemeless credentialed URL survive as a usable line rather than collapsing to `***`.
  eq(
    redactProxyUrl(normalizeProxyUrl('u:p@proxy.corp:3128')),
    'http://***@proxy.corp:3128',
    'normalise-then-redact keeps the host of a schemeless credentialed URL',
  )

  eq(hasUserinfo('http://u:p@proxy.corp:3128'), true, 'userinfo detected')
  eq(hasUserinfo('u:p@proxy.corp:3128'), true, 'userinfo detected without a scheme')
  eq(hasUserinfo('http://proxy.corp:3128'), false, 'no userinfo')
  eq(hasUserinfo('http://proxy.corp:3128/a@b'), false, 'an @ in the path is not a credential')

  // --- the screen still uses this module ---------------------------------------------------
  //
  // The extraction is the whole point: helpers copied back into the component would be
  // unreachable from here and this file would keep passing while checking nothing that ships.

  const section = readFileSync(fileURLToPath(new URL('../src/settings/ProxySection.tsx', import.meta.url)), 'utf8')
  eq(/from '\.\/proxyEnv'/.test(section), true, 'ProxySection imports the checked module')
  eq(
    /\n(function|const) (childEnvironment|bypassList|redact|normalize)\b/.test(section),
    false,
    'ProxySection has not grown its own copy of the mirror again',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `proxy: ok (${LOOPBACK.length} loopback hosts, ${PROXY_URL_VARS.length} URL variables, ` +
      `mirrored against cmd/session.rs)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
