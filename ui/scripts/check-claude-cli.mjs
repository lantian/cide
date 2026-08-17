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
  } = cli

  const config = (args = [], env = []) => ({ binary: 'claude', args, env })

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
    const end = rust.indexOf('\n];', start)
    if (end < 0) return null
    const body = rust.slice(start, end)
    return [...body.matchAll(/flag:\s*"([^"]+)",\s*aliases:\s*&\[([^\]]*)\],\s*takes_value:\s*(true|false)/g)].map(
      (m) => ({
        flag: m[1],
        aliases: [...m[2].matchAll(/"([^"]+)"/g)].map((a) => a[1]),
        value: m[3] === 'true',
      }),
    )
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
    (rustRefusedArgs ?? []).map((e) => [e.flag, e.aliases, e.value]),
    REFUSED_ARGS.map((e) => [e.flag, [...e.aliases], e.value]),
    'the arguments the screen strikes out are exactly the ones the spawn refuses, alias for '
      + 'alias and value-taking flag for value-taking flag',
  )
  eq(
    (rustWarnedArgs ?? []).map((e) => [e.flag, e.aliases, e.value]),
    WARNED_ARGS.map((e) => [e.flag, [...e.aliases], e.value]),
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
  for (const flag of ['--session-id', '--resume', '--fork-session', '--settings']) {
    ok(
      REFUSED_ARGS.some((e) => e.flag === flag),
      `${flag} is refused — it duplicates an argument cide passes itself, and the duplicate `
        + 'breaks resume or the hooks in silence',
    )
  }

  // =====================================================================================
  // 2. The port behaves like the rule.
  // =====================================================================================

  eq(argFate('--resume'), 'refused', 'a long flag is refused')
  eq(argFate('-r'), 'refused', 'and so is its short alias')
  eq(argFate('--resume=abc'), 'refused', 'and the `=` form, which commander accepts')
  eq(argFate('--safe-mode'), 'warned', 'a flag that costs the hooks is warned, not refused')
  eq(argFate('--model'), 'accepted', 'an ordinary flag passes')

  // Whole tokens only. `--append-system-prompt` contains `-p`; a `startsWith`/`includes`
  // implementation refuses it, and the user's system prompt silently disappears.
  for (const token of ['--append-system-prompt', '--resumed-thing', '--no-resume', '--printer']) {
    eq(argFate(token), 'accepted', `${token} merely contains a refused flag and is not one`)
  }

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
    for (const flag of ['--session-id', '--resume', '--fork-session']) {
      ok(!theirs.includes(flag), `${shape}: ${flag} is never attributed to the user`)
    }
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

  eq(refusalSummary(config()), null, 'nothing refused, nothing said')
  eq(refusalSummary(config(['--bare'])), '1 entry is refused', 'and the singular is singular')
  eq(refusalSummary(config(['--bare', '--print'])), '2 entries are refused', 'and the plural is not')

  eq(isDefaultConfig(config()), true, 'the untouched configuration is the default')
  eq(isDefaultConfig(config(['--model'])), false, 'and one argument is enough to be configured')

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
    const decides = session.indexOf('let is_claude = program_is_claude')
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
    const userArgs = session.indexOf('for a in plan.args')
    const conversation = session.indexOf('cide_claude::conversation')
    const settingsArg = session.indexOf('"--settings"')
    ok(userArgs >= 0 && conversation >= 0 && settingsArg >= 0, 'the three argv folds are readable')
    ok(
      userArgs < conversation && userArgs < settingsArg,
      'the user’s arguments are folded before cide’s conversation arguments and before '
        + '--settings. See the readout assertion above for why the order is the whole '
        + 'protection against a variadic flag',
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
    !/disabled/.test(section) || !/refused/.test(section.slice(section.indexOf('disabled'))),
    'a refused row is not disabled — it stays selectable and editable, because a value you '
      + 'cannot correct is worse than one that is merely wrong',
  )

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
