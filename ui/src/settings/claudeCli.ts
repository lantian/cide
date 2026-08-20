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

/** One injection's stored configuration. Mirrors `cide_ipc::ClaudeInjection`. */
export interface Inj {
  enabled: boolean
  flag: string
}

/** The arguments cide adds. Mirrors `cide_ipc::ClaudeInjections`' field names. */
export type InjectionKey = 'sessionId' | 'resume' | 'forkSession' | 'settings' | 'mcpConfig'

/** The stored shape, structurally. Mirrors `cide_ipc::ClaudeCli`. */
export interface CliConfig {
  binary: string
  args: string[]
  env: { name: string; value: string }[]
  inject: Record<InjectionKey, Inj>
}

/**
 * What cide adds to a pane's command line, and how it spells it by default.
 *
 * Mirrors `cide_core::claude_cli::INJECTIONS`, key for key and spelling for spelling, and
 * `check-claude-cli.mjs` reads that table out of the Rust and compares. The order is the order
 * the rows are drawn in and the order `resolvedArgv` writes them, which is not cosmetic:
 * `--resume <parent>` and `--session-id <ours>` both take a uuid, so a swap is still a valid
 * command line that resumes the wrong conversation.
 */
export const INJECTIONS: readonly { key: InjectionKey; defaultFlag: string }[] = [
  { key: 'sessionId', defaultFlag: '--session-id' },
  { key: 'resume', defaultFlag: '--resume' },
  { key: 'forkSession', defaultFlag: '--fork-session' },
  { key: 'settings', defaultFlag: '--settings' },
  // Last here and last in the argv, and that is the one ordering fact in this table that is
  // load-bearing rather than cosmetic: `--mcp-config <configs...>` is variadic and collects
  // every following token that does not begin with `-`, so the only token allowed after it is
  // its own JSON. `cmd/session.rs` writes it last for the same reason.
  { key: 'mcpConfig', defaultFlag: '--mcp-config' },
]

/** One injection, resolved: what will be written, and whether a rename was thrown away. */
export interface Effective {
  key: InjectionKey
  /** Whether cide will write this argument at all. */
  enabled: boolean
  /** The spelling it would be written with — the default one when no override survived. */
  flag: string
  /** Whether the stored override was discarded. The row says why, from Rust's sentence. */
  discarded: boolean
}

/**
 * One injection's stored value, defaulting to *today's behaviour* when it is missing.
 *
 * Errs safe in the same direction the module header describes for `APPDIR`: a fixture, an
 * older window's snapshot or a hand-edited file with no `inject` key draws as "cide passes
 * everything", which is what such a configuration actually does — `ClaudeInjection`'s
 * hand-written `Default` in Rust is `enabled: true` for exactly this reason. Reading it as
 * `false` would paint every injection off on a screen where nothing is.
 */
function setting(cli: CliConfig, key: InjectionKey): Inj {
  const table = cli.inject as Partial<Record<InjectionKey, Inj>> | undefined
  return table?.[key] ?? { enabled: true, flag: '' }
}

/**
 * Resolve the injection settings: what cide will write, spelled how.
 *
 * A port of `cide_core::claude_cli::injected`, including both discard rules, because the
 * screen has to strike a bad rename out as it is typed:
 *
 *  - **an override must begin with `-`** — `claude`'s first positional argument is a *prompt*,
 *    so a bare `sid` would not rename a flag, it would start every pane by asking the model
 *    something;
 *  - **it must not be another injection's spelling**, its default included, and a clash
 *    discards both sides rather than picking a winner nobody can see.
 *
 * A discarded override falls back to the default spelling, which is always safe: the defaults
 * are distinct from one another and they are what cide passed before this setting existed.
 */
export function effectiveFlags(cli: CliConfig): Effective[] {
  // Pass one: the shape rule. `null` means "no usable override; use the default".
  const proposed = INJECTIONS.map(({ key }) => {
    const over = setting(cli, key).flag.trim()
    if (over === '' || !over.startsWith('-')) return null
    return over
  })

  return INJECTIONS.map(({ key, defaultFlag }, index) => {
    const over = proposed[index] ?? null
    const enabled = setting(cli, key).enabled
    const typed = setting(cli, key).flag.trim()
    if (over === null) {
      return { key, enabled, flag: defaultFlag, discarded: typed !== '' }
    }
    // Pass two: the collision rule, decided against the snapshot rather than against a list
    // being rewritten as it is read.
    const clashes = INJECTIONS.some(
      (other, i) => i !== index && (other.defaultFlag === over || proposed[i] === over),
    )
    return { key, enabled, flag: clashes ? defaultFlag : over, discarded: clashes }
  })
}

/** The spelling this injection will be written with, or null when cide will not write it. */
export function flagFor(cli: CliConfig, key: InjectionKey): string | null {
  const row = effectiveFlags(cli).find((entry) => entry.key === key)
  if (row === undefined || !row.enabled) return null
  return row.flag
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
export const REFUSED_ARGS: readonly {
  flag: string
  aliases: readonly string[]
  value: boolean
  /**
   * The injection this refusal exists *because of*, or null for one that stands whatever cide
   * passes. Mirrors `cide_core::claude_cli::RefusedArg::because`, which carries the argument
   * at length: a flag cide has stopped passing must stop being refused, or the screen disables
   * a feature and then forbids the replacement.
   *
   * `--continue` carries `sessionId` and not a key of its own — it is illegal *beside* an
   * injected session id, so it is that injection's refusal rather than one about itself.
   */
  because: InjectionKey | null
}[] = [
  { flag: '--session-id', aliases: [], value: true, because: 'sessionId' },
  { flag: '--resume', aliases: ['-r'], value: true, because: 'resume' },
  { flag: '--fork-session', aliases: [], value: false, because: 'forkSession' },
  { flag: '--continue', aliases: ['-c'], value: false, because: 'sessionId' },
  { flag: '--settings', aliases: [], value: true, because: 'settings' },
  { flag: '--mcp-config', aliases: [], value: true, because: 'mcpConfig' },
  { flag: '--bare', aliases: [], value: false, because: null },
  { flag: '--print', aliases: ['-p'], value: false, because: null },
]

/**
 * Arguments that cost something cide cannot repair and are allowed anyway.
 *
 * `--append-system-prompt-file` is here rather than in `REFUSED_ARGS` because what it collides
 * with is a paragraph *cide* adds — the CLI refuses it beside an `--append-system-prompt` — so
 * cide's own addition is what gives way, not the user's field. Rust's row carries the argument
 * and the sentence; only the name lives here.
 */
export const WARNED_ARGS: readonly {
  flag: string
  aliases: readonly string[]
  value: boolean
  because: InjectionKey | null
}[] = [
  { flag: '--safe-mode', aliases: [], value: false, because: null },
  { flag: '--append-system-prompt-file', aliases: [], value: true, because: null },
]

/** Variables cide sets itself, or must never set. */
export const REFUSED_ENV: readonly string[] = [
  'ANTHROPIC_API_KEY',
  'ANTHROPIC_AUTH_TOKEN',
  'CLAUDE_CODE_SSE_PORT',
  'CIDE_HOOK_SOCK',
  'CIDE_AGENT_SOCK',
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

/**
 * The refusal that covers this token, given what cide is going to inject. A port of
 * `cide_core::claude_cli::refusal_for`, whose doc carries the three matching rules:
 *
 *  - `because: null` — matched on its own name and aliases, always;
 *  - the entry that *is* an injected flag — matched on the **effective** spelling, and not at
 *    all when that injection is off. Rename the session id to `--sid` and `--sid` becomes the
 *    refused token while `--session-id` becomes the user's to pass;
 *  - an entry that is a *different* flag, illegal beside an injected one (`--continue`) —
 *    matched on its own name, and merely gated on that injection being on. It is the CLI's
 *    flag, not cide's, so cide renaming its own does not move it.
 */
function refusalFor(
  token: string,
  cli: CliConfig,
): { flag: string; aliases: readonly string[]; value: boolean } | null {
  for (const entry of REFUSED_ARGS) {
    if (entry.because === null) {
      if (names(entry, token)) return entry
      continue
    }
    const effective = flagFor(cli, entry.because)
    if (effective === null) continue
    const spec = INJECTIONS.find((injection) => injection.key === entry.because)
    if (spec !== undefined && entry.flag === spec.defaultFlag && effective !== spec.defaultFlag) {
      const at = token.indexOf('=')
      const head = at === -1 ? token : token.slice(0, at)
      if (head === effective) return { ...entry, flag: effective, aliases: [] }
      continue
    }
    if (names(entry, token)) return entry
  }
  return null
}

/**
 * The verdict on one argument token, ignoring its neighbours.
 *
 * Takes the whole configuration rather than the token alone, because since the injection
 * switches the answer depends on what cide is still going to pass: a `--session-id` is the
 * user's to write once cide has stopped writing one, and a renamed `--sid` becomes cide's.
 */
export function argFate(token: string, cli: CliConfig): Fate {
  if (refusalFor(token, cli) !== null) return 'refused'
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
    const fate = argFate(text, cli)
    if (fate === 'refused') {
      const entry = refusalFor(text, cli)
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
 * `--settings <json>` and `--mcp-config <json>` are drawn as elided tokens rather than as
 * several kilobytes of inline hook payload and a config naming an absolute path to `cide-hook`
 * — the readout is 620px wide and neither is something a human reads. The *position* is the
 * part that has to be right, and `--mcp-config` is last because it is variadic.
 */
export function resolvedArgv(cli: CliConfig, shape: 'fresh' | 'resume' | 'fork'): string[] {
  const mine = judgeArgs(cli)
    .filter((row) => row.fate !== 'refused')
    .map((row) => row.text)
  const sessionId = flagFor(cli, 'sessionId')
  const resume = flagFor(cli, 'resume')
  const fork = flagFor(cli, 'forkSession')
  const settings = flagFor(cli, 'settings')
  const mcp = flagFor(cli, 'mcpConfig')

  // Mirrors `cide_claude::conversation`, degraded shapes included, because those are exactly
  // what a user who has switched something off needs to be able to read.
  //
  // A plain resume names no session id, which is not a simplification: the CLI keeps the
  // parent's id when it is not forking, and 2.1.227 rejects the combination outright. The same
  // rule is why a fork whose `--fork-session` is switched off falls back to the plain resume
  // rather than to `--resume <uuid> --session-id <uuid>`, which is the pair that fails.
  const fresh = sessionId === null ? [] : [sessionId, '<uuid>']
  const conversation =
    shape === 'fresh' || resume === null
      ? fresh
      : shape === 'resume' || fork === null
        ? [resume, '<uuid>']
        : [resume, '<uuid>', fork, ...fresh]

  return [
    cli.binary.trim() || 'claude',
    ...mine,
    ...conversation,
    ...(settings === null ? [] : [settings, '<cide hooks>']),
    // Last, and after `--settings`, exactly as `cmd/session.rs` writes it. A readout that put
    // the variadic flag anywhere else would be promising a command line nothing spawns.
    ...(mcp === null ? [] : [mcp, '<cide tools>']),
  ]
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
  return (
    cli.binary.trim() === 'claude' &&
    cli.args.length === 0 &&
    cli.env.length === 0 &&
    // …and cide still adds what it always did. A pane spawned with no `--settings` is the most
    // configured this screen gets, and calling that "nothing here yet" would be a lie in the
    // one place a user goes to find out why their status bar is empty.
    INJECTIONS.every(({ key }) => setting(cli, key).enabled && setting(cli, key).flag.trim() === '')
  )
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
