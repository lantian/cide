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
 *   - The constants. `LOOPBACK` and `PROXY_URL_VARS` are read straight out of `proxy.rs`
 *     and compared, and the mode and target names out of the generated `ProxyMode.ts` and
 *     `ProxyTarget.ts`. A list that gains an entry on one side only is exactly the drift the
 *     comments promise cannot happen, and it is invisible to the cases above.
 *
 * # The scope half
 *
 * `ProxyScope` gives every child one of three answers, and two of them produce an *empty*
 * readout for completely different reasons — `untouched` means cide touches nothing, so an
 * inherited proxy survives, while `inherit` means cide defers but still rescues loopback.
 * The screen has to say different sentences for those, so `readoutKind` is a function in the
 * checked module rather than a ternary in the component, and its three answers are asserted
 * below. A build that collapsed them would tell a corporate user that cide had taken their
 * `git push` off the proxy when it had done nothing of the kind.
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
    TARGETS,
    childEnvironment,
    readoutKind,
    claudeOnlyNote,
    isClaudeOnly,
    normalizeProxyUrl,
    bypassList,
    redactProxyUrl,
    hasUserinfo,
  } = await import(`file://${join(out, 'proxyEnv.js')}`)

  /** The default scope: today's behaviour, which is what every case below assumes. */
  const SCOPE = { claude: 'configured', shells: 'configured', git: 'untouched' }

  const manual = (http = '', https = '', all = '', noProxy = '') => ({
    mode: 'manual',
    scope: SCOPE,
    http,
    https,
    all,
    noProxy,
  })
  // Every case that predates `ProxyScope` asked "what does a child get"; a child is now
  // always some *particular* child, and the pane columns are the ones those cases were about.
  const values = (proxy, target = 'configured') =>
    Object.fromEntries(childEnvironment(proxy, target).map((line) => [line.name, line.value]))

  // --- the constants are the same constants ---------------------------------------------
  //
  // Read out of the Rust rather than restated here, so this check cannot drift in the same
  // edit that makes the mirror drift.

  // `cide_core::proxy`, not `cmd/session.rs`: the rule moved there when three spawn sites in
  // three crates started needing the same answer, and this check has to follow it or it goes
  // on comparing a list nothing reads.
  const core = rustSource('crates/cide-core/src/proxy.rs')
  eq(rustStrArray(core, 'LOOPBACK_EXEMPT'), [...LOOPBACK], 'LOOPBACK matches LOOPBACK_EXEMPT')
  eq(
    rustStrArray(core, 'PROXY_URL_VARS'),
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

  // Same argument one axis over. A fourth target state added in Rust would reach
  // `childEnvironment`'s final `else` and be drawn as `configured` — a readout claiming a
  // proxy for a child that is not getting one.
  const targetBinding = rustSource('crates/cide-ipc/bindings/ProxyTarget.ts')
  eq(
    [...targetBinding.matchAll(/"([a-z]+)"/g)].map((m) => m[1]),
    ['configured', 'untouched', 'direct'],
    'ProxyTarget is the three states this module branches on',
  )

  // And the fields of `ProxyScope`, so a fourth kind of child added in Rust cannot be one the
  // screen never draws a row for — which is the shape of this project's recurring defect: a
  // setting that exists, is honoured by the backend, and is reachable from nothing.
  const scopeBinding = rustSource('crates/cide-ipc/bindings/ProxyScope.ts')
  eq(
    [...scopeBinding.matchAll(/^(\w+): ProxyTarget,/gm)].map((m) => m[1]),
    TARGETS.map((t) => t.key),
    'every field of ProxyScope has a row on the screen',
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
    values({ mode: 'direct', scope: SCOPE, http: 'http://ignored:1', https: '', all: '', noProxy: 'corp' }),
    { HTTP_PROXY: null, HTTPS_PROXY: null, ALL_PROXY: null, NO_PROXY: null },
    'direct: all four removed, and the stored values ignored',
  )

  // Mirrors `the_no_proxy_default_touches_nothing`: inherit cannot be shown honestly from a
  // webview that cannot read this process's environment, so it shows nothing at all.
  eq(
    childEnvironment(
      { mode: 'inherit', scope: SCOPE, http: 'http://x:1', https: '', all: '', noProxy: 'corp' },
      'configured',
    ),
    [],
    'inherit: no rows, because the answer is not knowable here',
  )

  // --- the scope -----------------------------------------------------------------------------
  //
  // Mirrors `untouched_is_not_direct_and_the_difference_is_the_whole_point`,
  // `direct_beats_a_configured_address` and `untouched_ignores_even_direct_mode` in
  // `crates/cide-core/src/proxy.rs`.

  const corporate = manual('http://proxy.corp:3128', '', '', '')

  // `Untouched` draws nothing at all — not even the loopback rescue that `inherit` gets.
  eq(childEnvironment(corporate, 'untouched'), [], 'untouched: cide writes nothing')

  // `Direct` draws four removals, and this is the pair that matters: the two answers above
  // and below produce opposite environments from the same settings, and a screen that
  // confused them would tell a user their git was direct while it sat on an inherited proxy.
  eq(
    values(corporate, 'direct'),
    { HTTP_PROXY: null, HTTPS_PROXY: null, ALL_PROXY: null, NO_PROXY: null },
    'direct: four removals, whatever the mode says',
  )

  // And the target outranks the mode in both directions.
  eq(
    childEnvironment({ ...corporate, mode: 'direct' }, 'untouched'),
    [],
    'a child out of scope is out of scope for `No proxy` too',
  )
  eq(
    values({ ...corporate, mode: 'inherit' }, 'direct'),
    { HTTP_PROXY: null, HTTPS_PROXY: null, ALL_PROXY: null, NO_PROXY: null },
    'and `No proxy` on one target does not need the mode to agree',
  )

  // --- why a readout is empty ------------------------------------------------------------------
  //
  // The rule that would have lived in a ternary inside the component. Two empties, two
  // meanings, and the user needs them told apart: `inherited` still rescues loopback and is
  // in scope; `untouched` is cide keeping its hands off entirely, inherited proxy and all.

  eq(readoutKind(corporate, 'untouched'), 'untouched', 'out of scope reads as out of scope')
  eq(
    readoutKind({ ...corporate, mode: 'inherit' }, 'configured'),
    'inherited',
    'in scope and deferring is a different sentence from out of scope',
  )
  eq(readoutKind(corporate, 'configured'), 'lines', 'a configured manual proxy draws rows')
  eq(
    readoutKind({ ...corporate, mode: 'inherit' }, 'untouched'),
    'untouched',
    'the target is asked first: an inherit-mode child that is out of scope is not merely deferring',
  )
  eq(readoutKind({ ...corporate, mode: 'direct' }, 'configured'), 'lines', 'direct mode draws removals')

  // --- the shape the request behind this feature asked for --------------------------------------

  eq(
    isClaudeOnly({ ...corporate, scope: { claude: 'configured', shells: 'direct', git: 'direct' } }),
    true,
    'a proxy for claude and nothing else is recognised',
  )
  eq(
    isClaudeOnly({ ...corporate, scope: { claude: 'configured', shells: 'untouched', git: 'untouched' } }),
    true,
    'and “leave the rest alone” counts too — it is still only claude that cide proxies',
  )

  // ...and because BOTH of those satisfy `isClaudeOnly` while meaning opposite things, the one
  // sentence the screen prints underneath must not describe only one of them. It used to: the
  // note read "on whatever network cide itself is on" — true of `untouched`, and the exact
  // reverse of `direct`, which strips every proxy variable so the child goes direct even when
  // cide was started with one exported. That is the distinction `ProxyTargetName` has three
  // values to preserve, collapsed in the only place a user reads it in prose.
  const noteFor = (shells, git) =>
    claudeOnlyNote({ ...corporate, scope: { claude: 'configured', shells, git } })

  eq(
    /network cide itself is on/.test(noteFor('untouched', 'untouched'))
      && !/removed/.test(noteFor('untouched', 'untouched')),
    true,
    "left-alone children are described as sharing cide's own network",
  )
  eq(
    /removed/.test(noteFor('direct', 'direct'))
      && !/network cide itself is on/.test(noteFor('direct', 'direct')),
    true,
    "stripped children are described as having the proxy removed, not as sharing cide's network",
  )
  eq(
    noteFor('direct', 'direct') !== noteFor('untouched', 'untouched'),
    true,
    'and the two opposite meanings do not print the same sentence',
  )
  for (const [shells, git] of [['direct', 'untouched'], ['untouched', 'direct']]) {
    const mixed = noteFor(shells, git)
    eq(
      /removed/.test(mixed) && /network cide itself is on/.test(mixed),
      true,
      `a mixed scope (shells ${shells}, git ${git}) describes each child separately rather than `
        + 'picking one and speaking for both',
    )
  }
  eq(
    isClaudeOnly(corporate),
    false,
    'the default is not claude-only: it proxies both pane kinds, which is what it has always done',
  )
  eq(
    isClaudeOnly({ ...corporate, mode: 'inherit', scope: { claude: 'configured', shells: 'direct', git: 'direct' } }),
    false,
    'there is no proxy to be claude-only *with* unless one is configured',
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
    /\n(function|const) (childEnvironment|bypassList|redact|normalize|readoutKind|isClaudeOnly)\b/.test(section),
    false,
    'ProxySection has not grown its own copy of the mirror again',
  )

  // The one rule most likely to be inlined back into the component, because in JSX it looks
  // like two words: a `target === 'untouched' ? … : …` here is a decision no check script can
  // reach, and the last time a rule lived in a React hook it shipped a bug all 29 gates
  // missed.
  eq(
    /readoutKind\(/.test(section),
    true,
    'the two-kinds-of-empty rule is called rather than re-decided in JSX',
  )
  // Branching on `kind` — the answer the module gives back — is the correct shape and is
  // what the component does. Branching on `target` is re-deciding the rule, and that is the
  // exact line this assertion exists to stop being written.
  eq(
    /\btarget === '/.test(section),
    false,
    'ProxySection does not re-decide what a target means; that is `readoutKind`\'s job',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `proxy: ok (${LOOPBACK.length} loopback hosts, ${PROXY_URL_VARS.length} URL variables, ` +
      `${TARGETS.length} scope targets, mirrored against cide-core/src/proxy.rs)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
