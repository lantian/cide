/**
 * Checks the Claude launch configuration: which arguments and variables are refused, where the
 * user sees the refusal, and that the two implementations of the rule agree.
 *
 * # What this is guarding
 *
 * Settings gained a free-form argument list and a free-form environment editor. Both are
 * exactly the shape that breaks the two rules `CLAUDE.md` names as non-negotiable — never
 * inject `ANTHROPIC_API_KEY` (it outranks subscription OAuth and would bill a Console org for
 * a Max user), and never let an AppImage-internal path back into a child's environment (ADR
 * 0007, reported three processes away as `CONNECTION_CLOSED`) — plus a third: an argument that
 * duplicates one of cide's own breaks resume or the hooks *silently*.
 *
 * So there are three separate properties here, and a gate that proved only one would be
 * comfortable and useless:
 *
 *   1. **The rule exists and is right.** `cide_core::claude_cli`'s own tests. Not this file's
 *      job, and this file deliberately does not restate them.
 *   2. **The rule is reached from the spawn.** A refusal table nothing consults is this
 *      project's most-repeated defect wearing a security hat. Asserted here by reading
 *      comment-stripped `cmd/session.rs` and `cmd/settings.rs`.
 *   3. **The screen's copy of the rule agrees with Rust's.** `src/settings/claudeCli.ts` is a
 *      *port*, so a name added on one side only shows a wrong label — struck out in the UI and
 *      passed by the spawn, or worse, drawn as fine and then dropped. The tables are read out
 *      of the Rust and compared name for name.
 *
 * # Comment stripping is load-bearing
 *
 * Both sides explain these flags and variables *by name*, at length: `claude_cli.rs` is mostly
 * prose about `ANTHROPIC_API_KEY` and `PYTHONHOME`, and `session.rs` has three paragraphs about
 * ordering. A grep over raw source would therefore match the explanation of a feature that had
 * been deleted, which is precisely how a gate stays green over a dead rule. Every source
 * assertion below runs on comment-stripped text, and the mutation transcript for this file
 * includes the case where the code is removed and its paragraph left behind.
 *
 * # What this does NOT cover
 *
 *   - that a spawned child really receives the environment. That is `PtySession::spawn`'s and
 *     the Rust suite's; nothing in `ui/scripts/` forks a process.
 *   - the ADR 0007 *value* rule on the TypeScript side. It needs this process's `APPDIR`,
 *     which the webview does not have — the port says so and this file asserts that it says so,
 *     rather than pretending the two sides agree about something one of them cannot see.
 *
 * Run: `pnpm --dir ui run check:claude-cli`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-claude-cli-'))
let failed = 0

const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const repoFile = (rel) => readFileSync(fileURLToPath(new URL(`../../${rel}`, import.meta.url)), 'utf8')
const uiFile = (rel) => readFileSync(fileURLToPath(new URL(`../${rel}`, import.meta.url)), 'utf8')

/**
 * Remove comments before grepping. One stripper for Rust and TSX; see `check-claude-env.mjs`,
 * which makes the same argument at length. String literals are kept deliberately — the names
 * this file is entirely about live inside them on both sides.
 */
const strip = (src) => src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

/** Drop everything from the first `#[cfg(test)]`. A gate that reads a fixture measures it. */
const shipping = (src) => strip(src.split(/#\[cfg\(test\)\]/)[0])

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/settings/claudeCli.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const cli = await import(`file://${join(out, 'claudeCli.js')}`)
  const {
    REFUSED_ARGS,
    WARNED_ARGS,
    REFUSED_ENV,
    WARNED_ENV,
    argFate,
    envFate,
    judgeArgs,
    judgeEnv,
    reasonFor,
    resolvedArgv,
    resolvedArgvParts,
    isDefaultConfig,
    refusalSummary,
    INJECTIONS,
    effectiveFlags,
    flagFor,
  } = cli

  /**
   * A stored configuration. `inject` defaults to the shipped one — every argument cide adds,
   * at its own spelling — because that is what every assertion about today's behaviour is
   * about, and because a fixture that quietly injected nothing would make half this file pass
   * for the wrong reason.
   */
  const config = (args = [], env = [], inject = {}) => ({
    binary: 'claude',
    args,
    env,
    inject: {
      sessionId: { enabled: true, flag: '' },
      resume: { enabled: true, flag: '' },
      forkSession: { enabled: true, flag: '' },
      settings: { enabled: true, flag: '' },
      mcpConfig: { enabled: true, flag: '' },
      ...inject,
    },
  })

  /** The same, with one injection switched off. */
  const without = (key) => config([], [], { [key]: { enabled: false, flag: '' } })

  /** The same, with one injection renamed. */
  const spelled = (key, flag) => config([], [], { [key]: { enabled: true, flag } })

  // =====================================================================================
  // 1. The two tables are the same tables.
  // =====================================================================================
  //
  // Read out of the Rust rather than restated here, so this check cannot drift in the same
  // edit that makes the mirror drift.

  const rust = shipping(repoFile('crates/cide-core/src/claude_cli.rs'))

  /**
   * Every `RefusedArg { flag: "…", aliases: &["…"], takes_value: …, … }` in one table.
   *
   * The table is sliced by name first, so `REFUSED_ARGS` and `WARNED_ARGS` cannot be read as
   * one list — which would make a flag moved from refuse to warn invisible here, and that is
   * the single most consequential edit anybody will make to this file.
   */
  function rustArgTable(name) {
    const start = rust.indexOf(`pub const ${name}: &[RefusedArg] = &[`)
    if (start < 0) return null
    // The first `];` and not the first `\n];`: `WARNED_ARGS` is a one-entry table written as
    // `&[RefusedArg { … }];`, so a newline-anchored end ran past it and swallowed the `struct
    // RefusedArg` declaration below — whose `RefusedArg {` the splitter then read as two more
    // entries. `aliases: &[]` ends with `],`, never `];`, so this cannot stop early.
    const end = rust.indexOf('];', start)
    if (end < 0) return null
    // Split per entry rather than matching the whole row in one regex: `reason` is a multi-line
    // string literal sitting between `takes_value` and `because`, and a single pattern spanning
    // it would have to be non-greedy across entries — which silently pairs one row's flag with
    // the next row's `because` the first time somebody reorders a field.
    return rust
      .slice(start, end)
      .split('RefusedArg {')
      .slice(1)
      .map((entry) => {
        const flag = /flag:\s*"([^"]+)"/.exec(entry)
        const aliases = /aliases:\s*&\[([^\]]*)\]/.exec(entry)
        const value = /takes_value:\s*(true|false)/.exec(entry)
        const because = /because:\s*(None|Some\(Injection::(\w+)\))/.exec(entry)
        return {
          flag: flag?.[1] ?? null,
          aliases: [...(aliases?.[1] ?? '').matchAll(/"([^"]+)"/g)].map((a) => a[1]),
          value: value?.[1] === 'true',
          // `Injection::SessionId` on this side, `sessionId` on the other: compared under the
          // wire name, which is what `ClaudeInjections`' fields are called and what the screen
          // keys its rows by.
          because:
            because?.[2] === undefined
              ? null
              : because[2][0].toLowerCase() + because[2].slice(1),
        }
      })
      // A chunk with no `flag:` is not an entry — belt and braces beside the `end` rule above,
      // because a splitter that silently invents rows is how a table comparison passes for the
      // wrong reason.
      .filter((entry) => entry.flag !== null)
  }

  /**
   * The injection table: `key` and `default_flag` per row, read out of the same file.
   *
   * This is the half that makes the conditional refusals safe. If the screen thought cide
   * injected a different set from the one Rust injects, it would strike out a flag the user is
   * entitled to pass, or draw as fine one the spawn will refuse.
   */
  function rustInjections() {
    const start = rust.indexOf('pub const INJECTIONS: &[InjectionSpec] = &[')
    if (start < 0) return null
    const end = rust.indexOf('];', start)
    if (end < 0) return null
    return rust
      .slice(start, end)
      .split('InjectionSpec {')
      .slice(1)
      .map((entry) => ({
        key: /key:\s*"([^"]+)"/.exec(entry)?.[1] ?? null,
        defaultFlag: /default_flag:\s*"([^"]+)"/.exec(entry)?.[1] ?? null,
      }))
      .filter((entry) => entry.key !== null)
  }

  /** The names of a `pub const NAME: &[(&str, &str)] = &[("A", "…"), …];` table. */
  function rustEnvTable(name) {
    const start = rust.indexOf(`pub const ${name}: &[(&str, &str)] = &[`)
    if (start < 0) return null
    const end = rust.indexOf('\n];', start)
    if (end < 0) return null
    return [...rust.slice(start, end).matchAll(/\(\s*\n?\s*"([A-Z_][A-Z0-9_]*)",/g)].map((m) => m[1])
  }

  const rustRefusedArgs = rustArgTable('REFUSED_ARGS')
  const rustWarnedArgs = rustArgTable('WARNED_ARGS')
  const rustRefusedEnv = rustEnvTable('REFUSED_ENV')
  const rustWarnedEnv = rustEnvTable('WARNED_ENV')

  ok(rustRefusedArgs != null && rustRefusedArgs.length >= 7, 'REFUSED_ARGS is readable out of claude_cli.rs')
  ok(rustWarnedArgs != null && rustWarnedArgs.length >= 1, 'WARNED_ARGS is readable out of claude_cli.rs')
  ok(rustRefusedEnv != null && rustRefusedEnv.length >= 10, 'REFUSED_ENV is readable out of claude_cli.rs')
  ok(rustWarnedEnv != null && rustWarnedEnv.length >= 3, 'WARNED_ENV is readable out of claude_cli.rs')

  // Not merely the flag names: the aliases and `takes_value` too. A `takes_value` that
  // disagreed would make the screen draw a surviving argument the spawn actually swallows —
  // the readout would promise the child a token it never gets.
  eq(
    (rustRefusedArgs ?? []).map((e) => [e.flag, e.aliases, e.value, e.because]),
    REFUSED_ARGS.map((e) => [e.flag, [...e.aliases], e.value, e.because]),
    'the arguments the screen strikes out are exactly the ones the spawn refuses, alias for '
      + 'alias, value-taking flag for value-taking flag, and — since the injection switches — '
      + 'reason-for-existing for reason-for-existing. A `because` that disagreed would relax a '
      + 'refusal on one side only: the screen would draw a flag as fine and the spawn would '
      + 'drop it, or the screen would strike out one the child actually gets',
  )
  eq(
    (rustWarnedArgs ?? []).map((e) => [e.flag, e.aliases, e.value, e.because]),
    WARNED_ARGS.map((e) => [e.flag, [...e.aliases], e.value, e.because]),
    'and the warned arguments agree too — a flag moved between the two tables changes what '
      + 'reaches the child, not merely how it is drawn',
  )
  eq(rustRefusedEnv, [...REFUSED_ENV], 'the refused variable names match the Rust table')
  eq(rustWarnedEnv, [...WARNED_ENV], 'the warned variable names match the Rust table')

  // The two names the whole feature is dangerous without. Asserted separately from the set
  // equality above, so that deleting them from *both* sides — which the comparison happily
  // accepts — still fails.
  for (const name of ['ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN']) {
    ok(
      REFUSED_ENV.includes(name) && (rustRefusedEnv ?? []).includes(name),
      `${name} is refused on both sides. A key outranks subscription OAuth in the CLI's `
        + 'credential order, so setting one from Settings would silently bill a Console '
        + 'organisation for a Claude Max user — and the sentence already on this screen '
        + 'promising cide never sets one would become a lie',
    )
  }
  for (const name of ['CLAUDE_CODE_SSE_PORT', 'CIDE_HOOK_SOCK']) {
    ok(
      REFUSED_ENV.includes(name) && (rustRefusedEnv ?? []).includes(name),
      `${name} is refused on both sides — it is cide's own, and a wrong value points a pane's `
        + 'claude at another editor or at no hook socket at all, with nothing on screen saying so',
    )
  }
  for (const flag of ['--session-id', '--resume', '--fork-session', '--settings', '--mcp-config']) {
    ok(
      REFUSED_ARGS.some((e) => e.flag === flag),
      `${flag} is refused — it duplicates an argument cide passes itself, and the duplicate `
        + 'breaks resume or the hooks in silence',
    )
  }

  // =====================================================================================
  // 2. The port behaves like the rule.
  // =====================================================================================

  eq(argFate('--resume', config()), 'refused', 'a long flag is refused')
  eq(argFate('-r', config()), 'refused', 'and so is its short alias')
  eq(argFate('--resume=abc', config()), 'refused', 'and the `=` form, which commander accepts')
  eq(argFate('--safe-mode', config()), 'warned', 'a flag that costs the hooks is warned, not refused')
  eq(argFate('--model', config()), 'accepted', 'an ordinary flag passes')

  // Whole tokens only. `--append-system-prompt` contains `-p`; a `startsWith`/`includes`
  // implementation refuses it, and the user's system prompt silently disappears.
  for (const token of ['--append-system-prompt', '--resumed-thing', '--no-resume', '--printer']) {
    eq(argFate(token, config()), 'accepted', `${token} merely contains a refused flag and is not one`)
  }

  // =====================================================================================
  // 1b. The injections: the relationship the conditional refusals hang on.
  // =====================================================================================

  const rustInject = rustInjections()
  ok(rustInject != null && rustInject.length === 5, 'INJECTIONS is readable out of claude_cli.rs')
  eq(
    (rustInject ?? []).map((e) => [e.key, e.defaultFlag]),
    INJECTIONS.map((e) => [e.key, e.defaultFlag]),
    'the screen and the spawn inject the same arguments, spelled the same way. Disagree '
      + 'here and the screen strikes out a flag the user is now entitled to pass, or draws as '
      + 'fine one the spawn will drop',
  )
  eq(
    (rustInject ?? []).map((e) => e.defaultFlag),
    ['--session-id', '--resume', '--fork-session', '--settings', '--mcp-config'],
    'and the shipped spellings are still what cide has always passed. The default is the whole '
      + 'safety property of this feature: a user who never opens the screen sees no change',
  )

  // **The upgrade trap, from this side.** `ClaudeInjection`'s `Default` is hand-written in
  // `cide-ipc` because `bool::default()` is `false` and `ClaudeCli` carries a container-level
  // `#[serde(default)]`: derive it and every `workspace.json` already on disk — which is every
  // one of them — loads with every injection OFF. No hooks and no resume, for every user,
  // on the launch after an upgrade, from a screen they never opened. `cide-ipc`'s own test is
  // the guard; this is the second pair of eyes, because the failure has no runtime symptom.
  {
    const ipc = shipping(repoFile('crates/cide-ipc/src/settings.rs'))
    const impl = ipc.slice(ipc.indexOf('impl Default for ClaudeInjection {'))
    ok(
      /enabled:\s*true/.test(impl.slice(0, impl.indexOf('}'))),
      'every injection is ON by default, so a user who never opens this screen sees no change '
        + 'whatsoever. Derived, this is `false` and their hooks and resume die on upgrade',
    )
    ok(
      !/#\[derive\([^)]*Default[^)]*\)\]\s*(#\[[^\]]*\]\s*)*pub struct ClaudeInjection\b/.test(ipc),
      'and it is not derived',
    )
  }

  // The relationship, from this side. Every `because` names an injection that exists, and every
  // injection has a refusal that names it — two lists would be two chances to disable a feature
  // and then forbid the replacement.
  const becauses = [...new Set(REFUSED_ARGS.map((e) => e.because).filter((k) => k !== null))]
  eq(
    [...becauses].sort(),
    INJECTIONS.map((e) => e.key).sort(),
    'every injection is refused for the user, and every conditional refusal names an injection '
      + 'that cide actually adds',
  )
  for (const { key, defaultFlag } of INJECTIONS) {
    ok(
      REFUSED_ARGS.some((e) => e.flag === defaultFlag && e.because === key),
      `${defaultFlag} is refused because cide injects it, and the two facts are the same row`,
    )
  }
  for (const flag of ['--bare', '--print']) {
    ok(
      REFUSED_ARGS.find((e) => e.flag === flag)?.because === null,
      `${flag} breaks a pane whatever cide passes — an authentication failure and a one-shot `
        + 'pane are not a spelling question, so they must not become switchable from this screen',
    )
  }

  // Switching one off relaxes exactly its own refusals.
  eq(argFate('--session-id', without('sessionId')), 'accepted', 'cide no longer passes it')
  eq(
    argFate('--continue', without('sessionId')),
    'accepted',
    '--continue was only illegal beside an injected session id, and there is no longer one',
  )
  eq(argFate('-c', without('sessionId')), 'accepted', 'alias and all')
  eq(argFate('--settings', without('sessionId')), 'refused', 'and nothing else moved')
  eq(argFate('--resume', without('sessionId')), 'refused')
  eq(argFate('--bare', without('sessionId')), 'refused')
  eq(argFate('--settings', without('settings')), 'accepted', 'the hooks payload is the user’s now')
  eq(argFate('--mcp-config', config()), 'refused', 'cide attaches its own MCP server')
  eq(
    argFate('--mcp-config', without('mcpConfig')),
    'accepted',
    'and with cide’s own switched off the flag is the user’s to pass — a switch that took the '
      + 'task tools away and then forbade the replacement would be the worst of both',
  )
  eq(argFate('--settings', without('mcpConfig')), 'refused', 'and only that one moved')
  eq(argFate('--session-id', without('settings')), 'refused', 'and only that one moved')

  // A rename moves the refusal with it, in both directions.
  eq(argFate('--sid', spelled('sessionId', '--sid')), 'refused', 'the flag cide now writes')
  eq(argFate('--sid=abc', spelled('sessionId', '--sid')), 'refused', 'and the `=` form of it')
  eq(
    argFate('--session-id', spelled('sessionId', '--sid')),
    'accepted',
    'and the spelling cide has stopped writing is the user’s to pass',
  )
  eq(
    argFate('--continue', spelled('sessionId', '--sid')),
    'refused',
    '--continue is the CLI’s own flag; cide renaming its own does not move it',
  )

  // The two discard rules, which the screen has to draw as it is typed.
  eq(flagFor(spelled('sessionId', 'sid'), 'sessionId'), '--session-id',
    'a bare token is a POSITIONAL and claude’s first positional is a PROMPT — every pane would '
      + 'start by asking the model something, so the override is discarded')
  ok(
    effectiveFlags(spelled('sessionId', 'sid')).find((r) => r.key === 'sessionId')?.discarded,
    'and the discard is visible, because a field that silently ignores what was typed into it '
      + 'is the state this whole screen exists to avoid',
  )
  eq(flagFor(spelled('sessionId', '--resume'), 'sessionId'), '--session-id',
    'two injections cannot be spelled the same: a command line carrying one flag twice is the '
      + 'silent breakage the refusal table was written for')
  eq(flagFor(spelled('sessionId', '  --sid  '), 'sessionId'), '--sid', 'and an override is trimmed')
  eq(flagFor(without('settings'), 'settings'), null, 'a switched-off injection writes nothing')
  eq(
    effectiveFlags(config()).map((r) => [r.key, r.enabled, r.flag, r.discarded]),
    INJECTIONS.map((e) => [e.key, true, e.defaultFlag, false]),
    'and the shipped configuration resolves to exactly what cide always passed',
  )

  eq(envFate('ANTHROPIC_API_KEY'), 'refused', 'the credential name is refused')
  eq(envFate('anthropic_api_key'), 'refused', 'in any case')
  eq(envFate('  TERM  '), 'refused', 'and a trailing space does not smuggle a name through')
  eq(envFate('ANTHROPIC_BASE_URL'), 'warned', 'a gateway is the user’s decision, with a sentence')
  eq(envFate('MY_MCP_TOKEN'), 'accepted', 'an ordinary variable passes')

  // The orphan-positional trap, which is the one behaviour a reader would not guess: dropping
  // `--resume` and keeping `abc` leaves a positional argument, and `claude`'s first positional
  // *is a prompt* — so every pane would start by asking the model something.
  eq(
    judgeArgs(config(['--model', 'opus', '--resume', 'abc'])).map((r) => r.fate),
    ['accepted', 'accepted', 'refused', 'refused'],
    'a refused flag takes its value with it, or the orphan becomes a prompt',
  )
  eq(
    judgeArgs(config(['--fork-session', 'hello'])).map((r) => r.fate),
    ['refused', 'accepted'],
    'a value-less refused flag swallows nothing',
  )
  eq(
    judgeArgs(config(['--resume', '--model'])).map((r) => r.fate),
    ['refused', 'accepted'],
    'nor does one followed by another flag',
  )
  eq(
    judgeArgs(config(['--resume=abc', '--model'])).map((r) => r.fate),
    ['refused', 'accepted'],
    'and an inline value carries itself, so the next token is not ours to take',
  )

  eq(
    judgeEnv(config([], [{ name: '', value: 'x' }, { name: 'A', value: '1' }])).map((r) => r.text),
    ['A'],
    'a blank row is a row the user has not typed into yet, not a refusal',
  )
  eq(
    judgeEnv(config([], [{ name: 'A', value: '1' }, { name: 'A', value: '2' }])).length,
    2,
    'duplicates both survive — the child gets the last one, and collapsing them here would '
      + 'make the readout disagree with the child',
  )

  // The one place the two sides knowingly disagree, asserted rather than left to be
  // discovered: the ADR 0007 value rule needs `APPDIR`, which the webview does not have.
  eq(
    envFate('LD_LIBRARY_PATH'),
    'accepted',
    'the bundle *value* rule is not mirrored — it needs this process’s APPDIR. The port says '
      + 'so, and it errs safe: Rust refuses more than the screen predicts, never less',
  )
  ok(
    /APPDIR/.test(uiFile('src/settings/claudeCli.ts')),
    'and the port states that gap in its own header rather than leaving it to be found',
  )

  // =====================================================================================
  // 3. The readout, and the ordering it depends on.
  // =====================================================================================

  /* ----------------------------------------------- the sentences actually reach a screen */
  //
  // `RefusedArg::reason`'s own doc says it is "printed beside the struck-out token. Prose,
  // because the screen prints it." It was not printed: `Verdict::note()` had exactly one caller
  // and it was a `tracing` line, so a user who typed a refused flag got a struck-out row and no
  // explanation, while the sentence existed and was asserted non-empty by a Rust test.
  //
  // Two halves, and the second is the one that was missing: the prose has to CROSS THE WIRE, and
  // the component has to DRAW it. A test of the lookup alone would have passed the whole time.
  const table = [
    { name: '--resume', reason: 'cide owns the conversation id.' },
    { name: 'ANTHROPIC_API_KEY', reason: 'Bills a Console org rather than your subscription.' },
  ]
  eq(reasonFor('--resume', table), 'cide owns the conversation id.', 'a refused flag has its say')
  eq(reasonFor('-r', table), 'cide owns the conversation id.', '...and so does its alias')
  eq(reasonFor('--resume=abc', table), 'cide owns the conversation id.', '...and an inline value')
  eq(
    reasonFor('ANTHROPIC_API_KEY', table),
    'Bills a Console org rather than your subscription.',
    'an environment variable is looked up by name',
  )
  eq(reasonFor('--model', table), null, 'an accepted row says nothing rather than an empty line')
  eq(reasonFor('', table), null, 'and neither does a blank row')
  eq(reasonFor('--resume', []), null, 'a table that never arrived degrades to silence, not a crash')

  /* ------------------------------------------- which tokens are cide's, across all three shapes */
  //
  // The component used to derive this as `tokens.length - 4`. That is wrong for `fork`, whose
  // tail is five tokens, and off by one for the other two — so `--session-id`, the flag whose
  // value cide owns absolutely, was painted as if the user had written it, in the readout whose
  // whole purpose is to answer "which of these did I write".
  for (const shape of ['fresh', 'resume', 'fork']) {
    const parts = resolvedArgvParts(config(['--model', 'opus']), shape)
    const ours = parts.filter((p) => p.ours).map((p) => p.text)
    const theirs = parts.filter((p) => !p.ours).map((p) => p.text)
    eq(theirs, ['--model', 'opus'], `${shape}: exactly the user's two tokens are theirs`)
    ok(
      ours.includes('--settings') && !theirs.includes('--settings'),
      `${shape}: the hook settings are cide's`,
    )
    for (const flag of ['--session-id', '--resume', '--fork-session', '--mcp-config']) {
      ok(!theirs.includes(flag), `${shape}: ${flag} is never attributed to the user`)
    }
    const tokens = resolvedArgv(config(['--model', 'opus']), shape)
    eq(
      tokens[tokens.length - 2],
      '--mcp-config',
      `${shape}: the task tools are written LAST. --mcp-config <configs...> is variadic and `
        + 'collects every following non-flag token, so anywhere but the end it swallows a token '
        + 'cide wrote — and this readout is the only place a user can see that it did not',
    )
  }
  // ...and the binary is the program the user configured, not the literal word.
  eq(
    resolvedArgvParts({ binary: '/opt/claude-2.1', args: [], env: [] }, 'fresh')[0].text,
    '/opt/claude-2.1',
    'the readout names the configured binary — it promises the real command line',
  )

  const argv = resolvedArgv(config(['--model', 'opus', '--resume', 'x']), 'fresh')
  eq(argv[0], 'claude', 'the readout is a command line and starts with the program')
  ok(!argv.includes('x'), 'a refused flag’s value is absent from the resolved argv too')
  eq(
    argv.indexOf('--model') < argv.indexOf('--session-id'),
    true,
    'the user’s arguments come BEFORE every token cide adds. That is not cosmetic: --add-dir, '
      + '--mcp-config and --tools are variadic and collect every following non-flag token, and '
      + 'every argument cide appends begins with `-` — so a user flag placed first can never '
      + 'swallow cide’s session id, and one placed last would swallow whatever cide wrote',
  )
  eq(
    resolvedArgv(config(), 'resume').includes('--session-id'),
    false,
    'a plain resume names no --session-id: the CLI keeps the parent’s id when it is not '
      + 'forking, and 2.1.227 removed the combination outright',
  )

  /* ------------------------------------------- the readout under every toggle and rename */
  //
  // This readout is the ONLY surface a user has for the injection switches, so a wrong readout
  // is the whole feature wrong. Driven across all three conversation shapes for each toggle.

  for (const shape of ['fresh', 'resume', 'fork']) {
    ok(
      !resolvedArgv(without('settings'), shape).includes('--settings'),
      `${shape}: --settings is absent once the hook injection is off — which is the pane that `
        + 'reports no tokens, cannot tell busy from idle, and reloads buffers on a poll',
    )
    ok(
      resolvedArgv(without('sessionId'), shape).every((t) => t !== '--session-id'),
      `${shape}: no session id is named once that injection is off`,
    )
    ok(
      resolvedArgv(spelled('settings', '--config'), shape).includes('--config'),
      `${shape}: a renamed injection is written under its new spelling`,
    )
    ok(
      !resolvedArgv(spelled('settings', '--config'), shape).includes('--settings'),
      `${shape}: and not under its old one as well`,
    )
    ok(
      !resolvedArgv(without('mcpConfig'), shape).includes('--mcp-config'),
      `${shape}: no --mcp-config once the task tools are switched off — the pane keeps every `
        + 'MCP server the USER configured (cide never passes --strict-mcp-config) and loses '
        + 'cide’s own, so no task tracker and, on the console pane, no subagents at all',
    )
    ok(
      resolvedArgv(without('mcpConfig'), shape).includes('--settings'),
      `${shape}: and the hooks are untouched by it — two switches, two costs`,
    )
  }

  // **The shape 2.1.227 rejects, reached through the new door.** A fork with `--fork-session`
  // switched off must not become `--resume <uuid> --session-id <uuid>`: the CLI answers
  // "--session-id can only be used with --continue or --resume if --fork-session is also
  // specified" and the pane never appears.
  {
    const argv = resolvedArgv(without('forkSession'), 'fork')
    ok(argv.includes('--resume'), 'a fork with no fork flag is still a resume')
    ok(
      !argv.includes('--session-id'),
      'and it names no session id, because that pair is what the CLI rejects outright — every '
        + 'Resume click failing before a pane appears',
    )
  }
  eq(
    resolvedArgv(without('resume'), 'resume').includes('--resume'),
    false,
    'with the resume injection off a restored pane starts fresh, rather than emitting a lone '
      + '--fork-session for the CLI to reject',
  )
  ok(
    resolvedArgv(without('resume'), 'resume').includes('--session-id'),
    'and it is a genuine fresh session, named as one',
  )
  eq(
    resolvedArgv(config([], [], {
      sessionId: { enabled: false, flag: '' },
      resume: { enabled: false, flag: '' },
      forkSession: { enabled: false, flag: '' },
      settings: { enabled: false, flag: '' },
      mcpConfig: { enabled: false, flag: '' },
    }), 'fork'),
    ['claude'],
    'everything off is the bare binary: what a harness that is not Claude Code gets, which is '
      + 'the whole point of the switches',
  )

  eq(refusalSummary(config()), null, 'nothing refused, nothing said')
  eq(refusalSummary(config(['--bare'])), '1 entry is refused', 'and the singular is singular')
  eq(refusalSummary(config(['--bare', '--print'])), '2 entries are refused', 'and the plural is not')

  eq(isDefaultConfig(config()), true, 'the untouched configuration is the default')
  eq(isDefaultConfig(config(['--model'])), false, 'and one argument is enough to be configured')
  eq(
    isDefaultConfig(without('settings')),
    false,
    'and so is a switched-off injection: a pane spawned with no hooks is the most configured '
      + 'this screen gets, and calling it “nothing here yet” would be a lie in the one place a '
      + 'user goes to find out why their status bar is empty',
  )
  eq(isDefaultConfig(spelled('sessionId', '--sid')), false, 'a rename is a configuration too')

  // =====================================================================================
  // 4. The rule is reached from the spawn — over comment-stripped source.
  // =====================================================================================
  //
  // Everything above is satisfied by a `claude_cli` module nobody calls, which is this
  // project's most-repeated defect and the reason these four assertions exist.

  const session = shipping(repoFile('crates/cide-app/src/cmd/session.rs'))

  ok(
    /claude_cli::plan_here\s*\(/.test(session),
    '`cmd/session.rs` runs the launch-configuration plan at the spawn. Without this call the '
      + 'whole feature is a screen that stores text, and every assertion above still passes',
  )
  ok(
    /claude_cli::resolve\s*\(/.test(session),
    'and it checks the configured binary before forking, so a typo is a sentence in the pane’s '
      + 'own transcript rather than portable-pty’s ENOENT naming a file the user never typed',
  )
  ok(
    /spec\.program\s*=/.test(session),
    'and it actually substitutes the configured binary into the spec — a plan computed and '
      + 'discarded would leave every pane on the bare `claude` with no symptom until somebody '
      + 'configured one',
  )

  {
    // The ordering, in the code as well as in the readout. `is_claude` decides whether hooks
    // are attached and it is matched on a *file name*; decided after substituting, an absolute
    // path from Settings answers false and every session runs with no hooks at all — no token
    // figures, no fast buffer reload, no busy-versus-idle close confirm. Nothing fails.
    // Since M93 the decision is "which console did the caller ask for", by the same file-name
    // match (`console_program` calls `program_is_claude`) and under the same ordering rule.
    const decides = session.indexOf('let asked = console_program(&spec.program)')
    const substitutes = session.indexOf('spec.program =')
    ok(decides >= 0 && substitutes >= 0, 'both halves of the substitution are readable')
    ok(
      decides < substitutes,
      'the hooks decision is made BEFORE the configured binary is substituted. Reversed, an '
        + 'absolute path in Settings silently turns every session’s hooks off — which is the '
        + 'exact failure `program_is_claude`’s own note predicted for this feature',
    )
  }

  {
    // The argv ordering, at the one place it is real.
    //
    // The `--settings` literal used to be right here in `session.rs`; it now lives in
    // `claude_cli.rs`'s INJECTIONS table and reaches the spawn as `plan.inject`, so the fold is
    // found by the injection it is gated on rather than by the string. The spelling itself is
    // asserted against the Rust table above.
    const userArgs = session.indexOf('for a in plan.args')
    const conversation = session.indexOf('cide_claude::conversation')
    const settingsArg = session.indexOf('Injection::Settings')
    ok(userArgs >= 0 && conversation >= 0 && settingsArg >= 0, 'the three argv folds are readable')
    ok(
      userArgs < conversation && userArgs < settingsArg,
      'the user’s arguments are folded before cide’s conversation arguments and before '
        + '--settings. See the readout assertion above for why the order is the whole '
        + 'protection against a variadic flag',
    )
    ok(
      !/spec\.arg\("--settings"\)/.test(session),
      'and the hook payload is no longer attached under a hardcoded flag. A literal here would '
        + 'be a `--settings` the switches cannot turn off and the rename cannot move, while the '
        + 'screen drew both',
    )
    ok(
      /plan\.inject/.test(session),
      'the spawn folds the SAME resolved injection set the verdicts were computed against. '
        + 'Resolved twice, cide could refuse the user’s --session-id while passing none of its '
        + 'own — a switch that turns a feature off and then forbids the replacement',
    )
    ok(
      /conversation\(\s*minted,\s*resume,\s*wants_fork\(fork\),\s*&plan\.inject/.test(session),
      'and the conversation arguments are built from it too, not from three literals',
    )

    // The fifth injection, from the same side and for the same reason. It was appended under a
    // hardcoded `--mcp-config` with no switch at all until the row existed.
    ok(
      /Injection::McpConfig/.test(session),
      'the task tools are gated on their injection. A literal here would be an --mcp-config the '
        + 'switch cannot turn off and the rename cannot move, while the screen drew both',
    )
    ok(
      !/spec\.arg\("--mcp-config"\)/.test(session),
      'and not attached under a hardcoded flag either',
    )
    const mcpArg = session.indexOf('with_task_tools(spec')
    ok(mcpArg >= 0, 'the fold is readable')
    ok(
      userArgs < mcpArg && settingsArg < mcpArg,
      'and it is folded LAST, after the user’s arguments and after --settings. --mcp-config '
        + '<configs...> is variadic: anywhere else in the argv it swallows the next token cide '
        + 'wrote, which for the hook payload is several kilobytes of JSON silently becoming an '
        + 'MCP configuration',
    )
  }

  {
    // The honest half of the `--resume` switch. With it off cide passes no `--resume`, so a
    // pane planned as `Resumable` — or a dead pane offered *Resume this conversation* — would
    // silently start a NEW conversation under a heading promising the old one.
    const lifecycle = shipping(repoFile('crates/cide-app/src/lifecycle.rs'))
    ok(
      /inject\.resume\.enabled/.test(lifecycle),
      '`lifecycle.rs` reads the resume injection when it plans a restore',
    )
    ok(
      /resume_enabled/.test(lifecycle) && /if !resume_enabled/.test(lifecycle),
      'and answers Fresh when cide will not pass --resume, rather than offering a button that '
        + 'starts a new session',
    )
    ok(
      /inject\.resume\.enabled/.test(session),
      'and `session_resumable` — what a pane whose child just died asks — makes the same '
        + 'decision, or the launch and the dead-pane bar would disagree',
    )
  }

  ok(
    /if is_claude|is_claude\s*\{/.test(session),
    'and the plan is gated on the pane being a Claude one: a shell pane is the user’s shell, '
      + 'and an arbitrary NODE_OPTIONS or PATH from this list is not inert to it the way the '
      + 'CLAUDE_CODE_* switches are',
  )

  const settings = shipping(repoFile('crates/cide-app/src/cmd/settings.rs'))
  ok(
    /fn claude_cli\s*\(/.test(settings) && /claude_cli\(&state\)/.test(settings),
    'the headless one-shot lane reads the configured binary too. It used to spawn a `claude` '
      + 'constant, and a user who pinned a binary would have had Generate commit message run a '
      + 'different one — or none',
  )
  ok(
    !/const CLAUDE_PROGRAM/.test(settings),
    'and the hardcoded program constant is gone rather than left beside the setting, where it '
      + 'would be the value somebody reaches for next',
  )
  ok(
    /claude_cli::resolve\s*\(/.test(settings),
    'the Settings verdict resolves the configured binary, which is what makes a bad path fail '
      + 'at save with a sentence rather than at spawn with every pane at once',
  )
  ok(
    !/version::check_once/.test(shipping(repoFile('crates/cide-app/src/cmd/settings.rs')).slice(
      shipping(repoFile('crates/cide-app/src/cmd/settings.rs')).indexOf('fn cli_support'),
    )),
    'and the screen’s verdict is NOT the latched probe. `check_once` describes whichever '
      + 'binary was probed first in this process; with a configurable binary that means a user '
      + 'who corrects a typo reads back the verdict for the binary that answered ten minutes ago',
  )

  // =====================================================================================
  // 5. The refusal is where the user can see it.
  // =====================================================================================

  const section = strip(uiFile('src/settings/ClaudeCliSection.tsx'))
  ok(
    /judgeArgs\(/.test(section) && /judgeEnv\(/.test(section),
    'the section asks the checked module rather than re-deciding. A rule inside a component '
      + 'is a rule no check script can run, which is where six of this project’s shipped bugs '
      + 'lived',
  )
  ok(
    /reasonFor\(/.test(section) && /argReasons/.test(section) && /envReasons/.test(section),
    'the section looks the sentence up and is handed BOTH tables — a refusal with no reason is '
      + 'what shipped, and the prose existed in Rust the entire time',
  )
  ok(
    /resolvedArgvParts\(/.test(section),
    'and it draws the resolved argv through `resolvedArgvParts`, which owns the cide-versus-user '
      + 'split too. The component derived that split itself as `tokens.length - 4`: wrong for the '
      + '`fork` shape, whose tail is five tokens, and off by one even for the others, so cide’s '
      + 'own `--session-id` was drawn as if the user had typed it',
  )
  ok(
    /support\.problem|current\?\.problem/.test(section),
    'and it renders Rust’s binary verdict, which is the only side that can stat a path',
  )
  ok(
    !/disabled/.test(section),
    'no input on this screen is disabled — every row stays selectable and editable, including '
      + 'the rename beside a switched-off injection, because a value you cannot correct is '
      + 'worse than one that is merely wrong',
  )

  /* ------------------------------------------------ the injections, and what they cost */
  //
  // Each of these switches silently disables a feature the user will report later without
  // connecting it to this screen — "the status bar shows nothing", "Resume does nothing". The
  // copy is the only thing standing between the switch and that report, so it is asserted
  // word by word rather than left to be tidied into vagueness.
  ok(
    /effectiveFlags\(/.test(section),
    'the section resolves the injections through the checked module rather than re-deciding '
      + 'which flag it is about — a rule inside a component is a rule no check script can run',
  )
  ok(
    /injectReasons/.test(section),
    'and it draws Rust’s sentence for a discarded rename, which is the only place that '
      + 'refusal can be seen at all',
  )
  {
    const copy = section.slice(section.indexOf('INJECTION_COPY'))
    const sessionCopy = copy.slice(copy.indexOf('sessionId:'), copy.indexOf('resume:'))
    ok(
      /Resume/.test(sessionCopy),
      'the session-id row says that Resume stops working. That is the casualty, and a user who '
        + 'is not told names the wrong switch — turning the hooks off as well, chasing a status '
        + 'bar that was never affected',
    )
    ok(
      /CIDE_SESSION/.test(sessionCopy),
      '...and that the busy/idle chrome is NOT the casualty, because it routes on CIDE_SESSION',
    )
    // `mcpConfig:` sits between `forkSession:` and `settings:` in the table, so this slice is
    // bounded on both sides and the assertions below cannot be satisfied by a neighbour's prose.
    const mcpCopy = copy.slice(copy.indexOf('mcpConfig:'), copy.indexOf('settings:'))
    ok(mcpCopy.length > 200, 'the task-tools row has copy at all')
    for (const word of ['task', 'subagent', 'dispatch']) {
      ok(
        new RegExp(word, 'i').test(mcpCopy),
        `the --mcp-config row names what it costs: ${word}. Off, this pane cannot read or write `
          + '.cide/tasks.json and a console pane cannot dispatch anything — two features that '
          + 'simply stop, with nothing on screen connecting them to this switch',
      )
    }
    ok(
      /strict-mcp-config/.test(mcpCopy),
      '...and it says what it does NOT cost: your own MCP servers are still loaded, because cide '
        + 'has never passed --strict-mcp-config. Without that sentence the switch reads as “turn '
        + 'off MCP”, which is the one thing it does not do',
    )
    const settingsCopy = copy.slice(copy.indexOf('settings:'))
    for (const word of ['token', 'close', 'reload', 'notification', 'theme']) {
      ok(
        new RegExp(word, 'i').test(settingsCopy),
        `the --settings row names what it costs: ${word}. Off, this pane has no hooks AT ALL, `
          + 'and every one of those features stops with nothing on screen connecting it to '
          + 'this switch',
      )
    }
  }

  // …and the sentence has to be *fetched again* when the configuration changes, or the whole
  // `inject_reasons` path is dead in the running app.
  //
  // `ClaudeCliSupport` stopped being a fact about a binary the moment the injections became
  // configurable: `argReasons` is keyed by the flag cide will actually write, and
  // `injectReasons` exists only for the configuration that produced it. The effect that fetches
  // it was keyed on the binary alone, so typing `sid` into a flag field struck the input through
  // with NO sentence under it until the user edited the binary or closed the tab — the exact
  // state `CliReason` was added to end.
  {
    const tab = strip(uiFile('src/settings/SettingsTab.tsx'))
    const deps = /\}, \[([^\]]*)\]\)/g
    const support = tab.slice(tab.indexOf('claudeTasks.cliSupport('))
    const dep = deps.exec(support)?.[1] ?? ''
    ok(
      /binary/i.test(dep) && /inject/i.test(dep),
      'the cliSupport probe re-runs on the injection configuration as well as on the binary, '
        + 'or a discarded rename is struck through with no explanation and a refusal names a '
        + `flag nobody passes. Its dependencies are: ${dep || '(none found)'}`,
    )
  }

  const css = uiFile('src/settings/ClaudeCliSection.module.css')
  ok(
    /\.refused\b[\s\S]*?line-through/.test(css),
    'a refused row is struck through, not merely dimmed. Colour alone is a distinction nobody '
      + 'makes at a glance and one that vanishes entirely for a colour-blind reader',
  )

  const sections = strip(uiFile('src/settings/sections.tsx'))
  ok(
    /<ClaudeCliSection/.test(sections),
    'and the Settings screen actually mounts it. Complete, correct and reachable from nothing '
      + 'is this project’s most-repeated defect',
  )
  ok(
    /claude\.cli/.test(sections),
    'bound to `claude.cli`, which is also what `check-claude-env.mjs`’s field scan looks for',
  )
  // The neighbouring gate's trap, asserted from this side so the failure names the cause.
  // `check-claude-env.mjs` compares the CLAUDE_CODE_* names in `child_env.rs` with the ones in
  // `sections.tsx` and calls a mismatch "a switch wired to nothing". The refusal list names
  // four of those variables for the opposite reason.
  ok(
    !/CLAUDE_CODE_SSE_PORT/.test(uiFile('src/settings/sections.tsx')),
    'the refused variable names stay OUT of sections.tsx. check-claude-env.mjs asserts set '
      + 'equality between the CLAUDE_CODE_* names there and the ones a spawn actually sets, so '
      + 'naming a refused variable in that file fails it with a message about a defect that is '
      + 'not there',
  )

  if (failed > 0) {
    console.error(`\ncheck-claude-cli: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-claude-cli: ok (${REFUSED_ARGS.length} refused args, ${REFUSED_ENV.length} refused vars,`
      + ' mirrored against cide-core/src/claude_cli.rs)',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
