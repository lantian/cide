/**
 * The launch-configuration rules, mirrored from `cide_core::claude_cli` so the Settings screen
 * can strike a token out as it is typed.
 *
 * # A port, and therefore checked rather than trusted
 *
 * Rust is the authority: `session_spawn` runs `claude_cli::plan` at every spawn, so a
 * divergence here can only ever produce a **wrong label** — it can never hand a child an
 * argument Rust refuses. But the label is the entire reason the field is visible at all (a
 * refusal the user cannot see is a flag they will spend an afternoon on), so
 * `ui/scripts/check-claude-cli.mjs` reads the tables out of `crates/cide-core/src/claude_cli.rs`
 * and compares them with the ones below, name for name and value-taking flag for value-taking
 * flag. A name added on one side and not the other fails a gate.
 *
 * Same arrangement, same reason, as `./proxyEnv` next door — which the Rust proxy rule already
 * establishes as the shape for "a screen must show what the child gets".
 *
 * # Import-free, deliberately
 *
 * The check script compiles this module standalone with the TypeScript in `node_modules` and
 * runs it. An import of `@/ipc/client` — even a type-only one — makes that impossible, so the
 * three shapes it needs are declared here as structural types. They are the generated
 * `ClaudeCli`, `ClaudeEnvVar` and nothing else; a field added to either in Rust that this file
 * needs would fail to compile at the call site rather than silently.
 *
 * # The reasons are not duplicated
 *
 * Only the *names* live here, plus whether a flag takes a value. Every sentence is Rust's and
 * arrives on the wire in `ClaudeCliSupport` or is simply not shown — because a sentence written
 * twice is a sentence that will say two things, and this side has no way to notice.
 */

/** The stored shape, structurally. Mirrors `cide_ipc::ClaudeCli`. */
export interface CliConfig {
  binary: string
  args: string[]
  env: { name: string; value: string }[]
}

/** What cide will do with one row. Mirrors `cide_core::claude_cli::Verdict`. */
export type Fate = 'accepted' | 'warned' | 'refused'

/** One row, judged. Mirrors `cide_core::claude_cli::Judged` minus the prose. */
export interface Judged {
  index: number
  text: string
  fate: Fate
}

/**
 * Arguments cide passes itself. `value` is whether a following non-flag token belongs to this
 * one — which is what decides whether refusing it swallows one row or two.
 *
 * Spellings and aliases are `claude --help`'s, checked rather than remembered, and the check
 * script asserts this table is the same set as Rust's.
 */
export const REFUSED_ARGS: readonly { flag: string; aliases: readonly string[]; value: boolean }[] =
  [
    { flag: '--session-id', aliases: [], value: true },
    { flag: '--resume', aliases: ['-r'], value: true },
    { flag: '--fork-session', aliases: [], value: false },
    { flag: '--continue', aliases: ['-c'], value: false },
    { flag: '--settings', aliases: [], value: true },
    { flag: '--bare', aliases: [], value: false },
    { flag: '--print', aliases: ['-p'], value: false },
  ]

/** Arguments that cost something cide cannot repair and are allowed anyway. */
export const WARNED_ARGS: readonly { flag: string; aliases: readonly string[]; value: boolean }[] =
  [{ flag: '--safe-mode', aliases: [], value: false }]

/** Variables cide sets itself, or must never set. */
export const REFUSED_ENV: readonly string[] = [
  'ANTHROPIC_API_KEY',
  'ANTHROPIC_AUTH_TOKEN',
  'CLAUDE_CODE_SSE_PORT',
  'CIDE_HOOK_SOCK',
  'TERM',
  'COLUMNS',
  'LINES',
  'TMUX',
  'CLAUDE_CODE_SCROLL_SPEED',
  'CLAUDE_CODE_DISABLE_MOUSE',
  'CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT',
  'CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN',
  'HTTP_PROXY',
  'HTTPS_PROXY',
  'ALL_PROXY',
  'NO_PROXY',
  'CLAUDE_CONFIG_DIR',
]

/** Variables that work, mean something serious, and are allowed with a sentence. */
export const WARNED_ENV: readonly string[] = [
  'ANTHROPIC_BASE_URL',
  'CLAUDE_CODE_USE_BEDROCK',
  'CLAUDE_CODE_USE_VERTEX',
]

/**
 * Does this token name that flag?
 *
 * Whole token only, and the `=` form split off first. A `startsWith` here would refuse
 * `--printer` and — worse — `--append-system-prompt`, which contains `-p`.
 */
function names(entry: { flag: string; aliases: readonly string[] }, token: string): boolean {
  const at = token.indexOf('=')
  const head = at === -1 ? token : token.slice(0, at)
  return head === entry.flag || entry.aliases.includes(head)
}

/** The verdict on one argument token, ignoring its neighbours. */
export function argFate(token: string): Fate {
  if (REFUSED_ARGS.some((entry) => names(entry, token))) return 'refused'
  if (WARNED_ARGS.some((entry) => names(entry, token))) return 'warned'
  return 'accepted'
}

/**
 * The verdict on one variable name.
 *
 * Case-insensitive and trimmed, matching Rust — a refusal a trailing space defeats is not a
 * refusal. The **value** rule (a path inside the running AppImage, ADR 0007) is deliberately
 * *not* mirrored: it needs this process's `APPDIR`, which the webview has no access to and
 * should not be given. Such a value is refused at the spawn and reported by the Rust readout;
 * this side draws it as accepted. That is the one place the two sides knowingly disagree, and
 * it disagrees in the safe direction — cide refuses more than the screen predicts, never less.
 */
export function envFate(name: string): Fate {
  const key = name.trim().toUpperCase()
  if (REFUSED_ENV.includes(key)) return 'refused'
  if (WARNED_ENV.includes(key)) return 'warned'
  return 'accepted'
}

/**
 * Every argument row with its fate, including the values swallowed by a refused flag.
 *
 * The swallowing is the half a reader would not guess and the half that matters: dropping
 * `--resume` and keeping `abc` leaves a positional argument, and `claude`'s first positional
 * *is a prompt* — so the pane would start by asking the model something. Rust does the same and
 * its own note carries the argument.
 */
export function judgeArgs(cli: CliConfig): Judged[] {
  const out: Judged[] = []
  let swallowing = false

  cli.args.forEach((text, index) => {
    if (swallowing && !text.startsWith('-')) {
      swallowing = false
      out.push({ index, text, fate: 'refused' })
      return
    }
    swallowing = false
    const fate = argFate(text)
    if (fate === 'refused') {
      const entry = REFUSED_ARGS.find((candidate) => names(candidate, text))
      if (entry?.value === true && !text.includes('=')) swallowing = true
    }
    out.push({ index, text, fate })
  })

  return out
}

/** Every environment row with its fate. Blank names are skipped, as in Rust. */
export function judgeEnv(cli: CliConfig): Judged[] {
  const out: Judged[] = []
  cli.env.forEach((entry, index) => {
    const name = entry.name.trim()
    if (name === '') return
    out.push({ index, text: name, fate: envFate(name) })
  })
  return out
}

/**
 * The argv a Claude pane is actually spawned with, as tokens.
 *
 * This is the readout, and it is the honest half of the screen — a launch configuration that
 * does not show you the resulting command line is one you can only debug by reading `ps`. It
 * shows cide's own arguments too, because the user's flags are only half of an argv and the
 * *order* is what makes the refusals make sense: everything cide adds begins with `-`, which is
 * why a variadic user flag placed first can never swallow one of them.
 *
 * `--settings <json>` is drawn as an elided token rather than as several kilobytes of inline
 * hook payload — the readout is 620px wide and the JSON is not something a human reads.
 */
export function resolvedArgv(cli: CliConfig, shape: 'fresh' | 'resume' | 'fork'): string[] {
  const mine = judgeArgs(cli)
    .filter((row) => row.fate !== 'refused')
    .map((row) => row.text)
  // Mirrors `cide_claude::conversation`'s three shapes. A plain resume names no `--session-id`,
  // which is not a simplification: the CLI keeps the parent's id when it is not forking, and
  // 2.1.227 removed the combination outright.
  const conversation =
    shape === 'fresh'
      ? ['--session-id', '<uuid>']
      : shape === 'resume'
        ? ['--resume', '<uuid>']
        : ['--resume', '<uuid>', '--fork-session']
  return [cli.binary.trim() || 'claude', ...mine, ...conversation, '--settings', '<cide hooks>']
}

/**
 * The same argv, with each token marked as cide's or the user's.
 *
 * Exists because the renderer was deriving that split itself, as `tokens.length - 4`, and it was
 * wrong twice over. The tail is not a constant: `fork` adds `--fork-session`, so it is five
 * tokens rather than four and every mark shifts. And the arithmetic was off by one even in the
 * common case — `--session-id` came out in the user's colour, in a readout whose whole purpose is
 * to answer *which of these did I write*, about the one flag whose value cide owns absolutely
 * (a `SessionId` IS that uuid, which is what makes resume work).
 *
 * Returned from here rather than computed in the component so `check-claude-cli.mjs` can drive
 * it across all three shapes; a split derived from a length is exactly the kind of rule that
 * looks right and cannot be seen to be wrong.
 */
export function resolvedArgvParts(
  cli: CliConfig,
  shape: 'fresh' | 'resume' | 'fork',
): { text: string; ours: boolean }[] {
  const tokens = resolvedArgv(cli, shape)
  const mine = judgeArgs(cli).filter((row) => row.fate !== 'refused').length
  // Index 0 is the binary — cide's choice of program even when the user named it, because the
  // question the colour answers is "did I write this token", and the binary has its own field.
  return tokens.map((text, i) => ({ text, ours: i === 0 || i > mine }))
}

/**
 * Whether anything at all is configured, for the "nothing here yet" state.
 *
 * The binary is compared against the default rather than tested for emptiness: `claude` is what
 * the field holds when the user has never touched it, and a screen that called that
 * "configured" would have no empty state at all.
 */
export function isDefaultConfig(cli: CliConfig): boolean {
  return cli.binary.trim() === 'claude' && cli.args.length === 0 && cli.env.length === 0
}

/**
 * A short label for the count of refusals, or null when there are none.
 *
 * A rule and not a template, because the plural and the zero case are exactly the shape that
 * ships as `1 arguments refused`.
 */
export function refusalSummary(cli: CliConfig): string | null {
  const n = [...judgeArgs(cli), ...judgeEnv(cli)].filter((row) => row.fate === 'refused').length
  if (n === 0) return null
  return n === 1 ? '1 entry is refused' : `${n} entries are refused`
}


/**
 * The sentence for one row, looked up in the table Rust shipped.
 *
 * `judgeArgs` and `judgeEnv` already decided the row's fate here, with no round trip, by matching
 * the NAMES that live in this file. What they cannot supply is the prose, which is deliberately
 * single-sourced in `cide_core::claude_cli` — so it arrives once in `ClaudeCliSupport` and is
 * matched back to a row here.
 *
 * Canonicalises before looking up: the table is keyed by long-form flag, and a user may have
 * written an alias (`-r`) or an inline value (`--resume=x`). Without that a struck-out `-r` would
 * be struck out with no explanation, which is the state this whole function exists to end.
 *
 * `null` when there is nothing to say — an accepted row, or a table that did not arrive because
 * the command was unavailable. The caller draws nothing rather than an empty line.
 */
export function reasonFor(
  text: string,
  table: readonly { name: string; reason: string }[],
): string | null {
  const token = text.trim()
  if (token === '') return null
  const bare = token.split('=')[0] ?? token
  const canonical =
    REFUSED_ARGS.find((a) => a.flag === bare || a.aliases.includes(bare))?.flag ?? bare
  return table.find((entry) => entry.name === canonical)?.reason ?? null
}
