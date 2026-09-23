/**
 * The provider and pool rules behind the Models screen. (M45)
 *
 * # Import-free, deliberately
 *
 * `ui/scripts/check-pools.mjs` compiles this module standalone with the TypeScript in
 * `node_modules` and runs it. An import of `@/ipc/client` — even a type-only one — makes that
 * impossible, so the shapes it needs are declared here as structural types mirroring
 * `cide_ipc::llm`. That is `claudeCli.ts` and `agentsDraft.ts`'s arrangement next door, for the
 * same reason: a rule inside a component is a rule no check script can drive, and this project
 * has shipped bugs that lived exactly there.
 *
 * # What is here and what is not
 *
 * Here: which kinds exist, what a pool entry becomes on the argv, what is wrong with a row, and
 * the pure list edits. Not here: any sentence Rust owns, and any decision about what cide
 * *writes* — `cide_agents::harness::opencode::provider_members` is the only producer of the
 * document, and duplicating its rules here would create a second one that could disagree.
 *
 * # The one splitter, and why it takes the first slash
 *
 * `opencode models` prints `providerID/modelID`, and a model id may itself contain `/` and `:`
 * (`lmstudio/openai/gpt-oss-20b`, `unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M`). So the split is *first
 * slash wins*, it happens exactly once — here — and everything downstream only ever joins the
 * halves back with `entryFlags`. Two splitters would eventually disagree about what a provider is.
 */

/** Which shape a provider is. Mirrors `cide_ipc::LlmProvider`'s serde tag. */
export type ProviderKind = 'catalog' | 'custom' | 'external'

/** Every kind, in the order the Add menu offers them. */
export const PROVIDER_KINDS: readonly ProviderKind[] = ['catalog', 'custom', 'external']

/** One model under a custom endpoint. Mirrors `cide_ipc::LlmModel`. */
export interface Model {
  id: string
  label: string
  context: number
  output: number
}

/** A provider row. A structural mirror of `cide_ipc::LlmProvider`'s three variants. */
export type Provider =
  | { kind: 'catalog'; id: string; label: string; enabled: boolean; apiKey: string }
  | {
      kind: 'custom'
      id: string
      label: string
      enabled: boolean
      npm: string
      baseUrl: string
      apiKey: string
      models: Model[]
    }
  | { kind: 'external'; id: string; label: string; setup: string; expect: string[] }

/** One entry in a pool. Mirrors `cide_ipc::PoolEntry`. */
export interface Entry {
  provider: string
  model: string
  variant: string
  /**
   * How many runs may be on this entry at once, machine-wide. **Absent** means no limit — never
   * `null` and never `0`, the rule `check:pools` holds every optional field on this wire to.
   */
  maxRunning?: number
}

/** A pool. Mirrors `cide_ipc::ModelPool`. */
export interface Pool {
  name: string
  description: string
  entries: Entry[]
}

/** Mirrors `cide_ipc::LlmSettings`. */
export interface Llm {
  providers: Provider[]
  pools: Pool[]
}

/**
 * Provider ids worth suggesting, as a `<datalist>`.
 *
 * **A suggestion and never a rule.** opencode ships a catalog of 213 providers cached at
 * `$XDG_CACHE_HOME/opencode/models.json`, and this list is a hand-written handful of it — the
 * cache is a *cache path*, absent on a fresh install and moveable by any release, so nothing
 * stored may depend on reading it. A stale entry here costs nothing, because in `catalog` mode
 * cide writes no npm package and no base URL: the only thing a suggestion can be wrong about is
 * whether the id is worth suggesting. Free text is always accepted.
 */
export const KNOWN_CATALOG_IDS: readonly string[] = [
  'openrouter',
  'deepseek',
  'openai',
  'anthropic',
  'google',
  'groq',
  'mistral',
  'lmstudio',
  'xai',
  'cerebras',
]

/**
 * What the npm field starts at for a custom endpoint.
 *
 * The SDK an OpenAI-compatible endpoint wants. Prefilled rather than assumed, because an endpoint
 * wanting a different one is exactly the case that variant exists for.
 */
export const DEFAULT_CUSTOM_NPM = '@ai-sdk/openai-compatible'

/**
 * Effort variants worth offering beside a pool entry.
 *
 * Open, exactly as `agentsDraft.ts`'s `EFFORT_SUGGESTIONS` is and for the same reason: the set is
 * per-provider and per-release and updates underneath a running cide, so a value a release added
 * last week must stay typable. `gpt-5.2` offers all five; `gpt-5.1-codex-mini` offers two.
 */
export const VARIANT_SUGGESTIONS: readonly string[] = ['none', 'low', 'medium', 'high', 'xhigh']

/** The npm package that exposes a ChatGPT/codex subscription to opencode. */
export const CODEX_PLUGIN = 'opencode-openai-codex-auth'

/** The provider id that plugin registers, and the left half of every model it offers. */
export const CODEX_PROVIDER_ID = 'openai'

/** The models it offers, as the presence test for "did the setup work". */
export const CODEX_EXPECT: readonly string[] = [
  'openai/gpt-5.2',
  'openai/gpt-5.2-codex',
  'openai/gpt-5.1-codex-max',
  'openai/gpt-5.1-codex',
  'openai/gpt-5.1',
]

/** What the setup box starts at for the codex subscription. */
export const CODEX_PLUGIN_SETUP =
  'Add "opencode-openai-codex-auth" to the `plugin` array in your own opencode.json, then run ' +
  '`opencode auth login`. cide does not write either — listing a plugin makes opencode install ' +
  'it from npm, and an IDE should not fetch a package because you pressed Dispatch.'

/** A blank row of each kind, which is what the Add menu produces. */
export function blankProvider(kind: ProviderKind): Provider {
  switch (kind) {
    // `enabled: true` mirrors `LlmProvider::default`'s hand-written arm, whose doc explains why a
    // provider that arrives disabled is the dangerous default.
    case 'catalog':
      return { kind: 'catalog', id: '', label: '', enabled: true, apiKey: '' }
    case 'custom':
      return {
        kind: 'custom',
        id: '',
        label: '',
        enabled: true,
        npm: DEFAULT_CUSTOM_NPM,
        baseUrl: '',
        apiKey: '',
        models: [],
      }
    // Pre-filled for the codex subscription rather than blank. It is the only plugin-backed
    // provider anyone has asked for, a generic "plugin-backed provider" button asked the user to
    // know things cide could fill in, and cide keeps no table of plugins to offer a second one
    // from — so this variant is that one row, and the fields stay editable for anybody who has
    // another.
    case 'external':
      return {
        kind: 'external',
        id: CODEX_PROVIDER_ID,
        label: 'ChatGPT subscription',
        setup: CODEX_PLUGIN_SETUP,
        expect: [...CODEX_EXPECT],
      }
    default: {
      const unreachable: never = kind
      return unreachable
    }
  }
}

/** Whether cide writes anything for this provider. Mirrors `LlmProvider::enabled`. */
export function isEnabled(provider: Provider): boolean {
  // An external provider is always "on": there is nothing for a switch to gate, because cide
  // writes nothing for it either way. Answering true keeps every caller from needing a fourth
  // state.
  return provider.kind === 'external' ? true : provider.enabled
}

/** The credential, or `''` for a kind that has none. */
export function apiKeyOf(provider: Provider): string {
  return provider.kind === 'external' ? '' : provider.apiKey
}

/**
 * The flags `opencode run` gets for this entry.
 *
 * The **only** place the two halves are joined, mirroring `cide_ipc::PoolEntry::model_flag`, so
 * the readout on screen describes the run that happens. A second joiner is how a preview starts
 * describing a different run from the one it previews — `cide_git::push::preview`'s rule.
 */
export function entryFlags(entry: Entry): string[] {
  const flags = ['--model', joinModelId(entry.provider, entry.model)]
  if (entry.variant !== '') {
    flags.push('--variant', entry.variant)
  }
  return flags
}

/**
 * The two halves of a model id, joined.
 *
 * The **only** join in this half of the codebase, mirroring `cide_ipc::PoolEntry::model_flag` on
 * the Rust side. Everything needing a `provider/model` calls this — including the test button,
 * which is where a second, inline `${provider}/${model}` first crept in and was caught by
 * `check:pools`. Two joiners is how a readout starts describing a different run from the one that
 * happens, which is `cide_git::push::preview`'s rule.
 */
export function joinModelId(provider: string, model: string): string {
  return `${provider}/${model}`
}

/**
 * A `provider/model` line from `opencode models`, split into its halves.
 *
 * First slash wins — see the module header. Returns `null` for a line that is not one, which is
 * every banner and warning the CLI might print above its list.
 */
export function splitModelId(line: string): { provider: string; model: string } | null {
  const at = line.indexOf('/')
  if (at <= 0 || at === line.length - 1) {
    return null
  }
  return { provider: line.slice(0, at), model: line.slice(at + 1) }
}

/**
 * What is wrong with a pool entry, as a closed set the screen must draw every member of.
 *
 * `incomplete` is deliberately separate from the two `…Provider` failures, and the distinction is
 * the whole point of the set: a row the user has *just added* names nothing yet, and drawing it
 * struck through as if it were broken makes every new pool look like an error. Only a row that
 * names something **wrong** is struck.
 */
export type EntryVerdict = 'ok' | 'incomplete' | 'unknownProvider' | 'disabledProvider'

/** Every verdict, so a check script can assert the screen branches on all of them. */
export const ENTRY_VERDICTS: readonly EntryVerdict[] = [
  'ok',
  'incomplete',
  'unknownProvider',
  'disabledProvider',
]

/**
 * Judge one entry against the configured providers.
 *
 * An entry naming a provider that does not exist is **kept and struck through**, never deleted:
 * the provider may be the next thing the user creates, which is `LlmSettings::cleaned`'s rule on
 * the other side of the wire.
 */
export function judgeEntry(entry: Entry, providers: readonly Provider[]): EntryVerdict {
  // Nothing named yet: a row that was added a second ago, not a row that is wrong. Answered
  // before the provider lookup so a half-filled row never reads as a broken one.
  if (entry.provider.trim() === '' || entry.model.trim() === '') {
    return 'incomplete'
  }
  const provider = providers.find((candidate) => candidate.id === entry.provider)
  if (provider === undefined) {
    return 'unknownProvider'
  }
  return isEnabled(provider) ? 'ok' : 'disabledProvider'
}

/**
 * Should this row be drawn struck through?
 *
 * Only a row naming something wrong. An `incomplete` row is one the user is still filling in, and
 * striking it is how a freshly added entry looks like a failure.
 */
export function isStruck(verdict: EntryVerdict): boolean {
  return verdict === 'unknownProvider' || verdict === 'disabledProvider'
}

/**
 * What is wrong with a provider row *before* anything is sent.
 *
 * Local shape problems only — an id nobody typed, a custom endpoint with no models. Whether the
 * provider actually *works* is a question only `opencode models` can answer, and that verdict
 * comes from the probe rather than from here. Keeping the two apart is what stops this growing
 * into a second validator that disagrees with the CLI.
 */
export function localProblems(provider: Provider): string[] {
  const problems: string[] = []
  if (provider.id.trim() === '') {
    problems.push('This provider needs an id — it is the left half of every provider/model.')
  }
  if (provider.kind === 'custom') {
    if (provider.baseUrl.trim() === '') {
      problems.push('A custom endpoint needs a base URL, or opencode has nowhere to send a request.')
    }
    for (const model of provider.models) {
      // Measured against opencode 1.18.29: a `limit` carrying one key and not the other is
      // refused outright — "Missing key provider.<id>.models.<model>.limit.output" — and a
      // refused document makes the run silently opencode's *default agent*. Rust writes no
      // `limit` for a half-filled pair rather than an invalid one; this says so before the save,
      // because the number the user half-typed is one they meant to have an effect.
      if ((model.context > 0) !== (model.output > 0)) {
        problems.push(
          `"${model.id === '' ? 'a model' : model.id}" gives only one of its two limits. ` +
            'opencode needs both a context and an output limit or neither, so cide will write ' +
            'neither until the pair is complete.',
        )
      }
    }
    if (provider.models.filter((model) => model.id.trim() !== '').length === 0) {
      problems.push(
        'A custom endpoint must declare its models. opencode has no catalog for an endpoint it ' +
          'has never heard of, so without a model list `--model ' +
          (provider.id.trim() === '' ? '<id>' : provider.id) +
          '/…` resolves to nothing.',
      )
    }
  }
  return problems
}

/**
 * The entry with its running limit set from what the box holds. Blank, `0`, a fraction or junk
 * is "no limit", and "no limit" is the key **deleted** rather than set to `undefined` — the
 * object goes to Rust whole, and an unset field must be absent on the wire.
 */
export function withMaxRunning(entry: Entry, text: string): Entry {
  const next: Entry = { ...entry }
  const n = Number(text.trim())
  if (text.trim() !== '' && Number.isInteger(n) && n >= 1 && n <= 65535) {
    next.maxRunning = n
  } else {
    delete next.maxRunning
  }
  return next
}

/**
 * How many runs a pool can hold at once: the sum of its complete entries' limits, or `null`
 * when any complete entry is unlimited (or none has a limit). Mirrors
 * `cide_ipc::pool_capacity`, entry completeness included — `check:pools` holds the two to the
 * same cases.
 */
export function poolCapacity(entries: readonly Entry[]): number | null {
  let total = 0
  let any = false
  for (const entry of entries) {
    if (entry.provider === '' || entry.model === '') continue
    if (entry.maxRunning === undefined) return null
    total += entry.maxRunning
    any = true
  }
  return any ? total : null
}

/** Every pool name, for the override picker. */
export function poolNames(llm: Llm): string[] {
  return llm.pools.map((pool) => pool.name).filter((name) => name !== '')
}

/**
 * Move the item at `index` one place towards the front.
 *
 * Returns a **new** array, and returns the same contents unchanged when the move is impossible.
 * The off-by-one this guards is the one that silently drops an entry rather than throwing, which
 * in an ordered pool means a run reaching a model the user removed from the list.
 */
export function moveUp<T>(items: readonly T[], index: number): T[] {
  const next = [...items]
  if (index <= 0 || index >= next.length) {
    return next
  }
  const item = next[index] as T
  next[index] = next[index - 1] as T
  next[index - 1] = item
  return next
}

/** Move the item at `index` one place towards the back. See [`moveUp`]. */
export function moveDown<T>(items: readonly T[], index: number): T[] {
  const next = [...items]
  if (index < 0 || index >= next.length - 1) {
    return next
  }
  const item = next[index] as T
  next[index] = next[index + 1] as T
  next[index + 1] = item
  return next
}

/** Drop the item at `index`, preserving the order of the rest. */
export function removeAt<T>(items: readonly T[], index: number): T[] {
  const next = [...items]
  if (index < 0 || index >= next.length) {
    return next
  }
  next.splice(index, 1)
  return next
}

/**
 * The model ids `opencode models` reported under one provider, for a dropdown.
 *
 * Takes the raw `provider/model` lines and returns only the right halves, so the picker offers
 * what a run can actually reach on this machine rather than what cide has stored. `splitModelId`
 * is the only splitter — see the module header.
 */
export function modelsFor(provider: string, lines: readonly string[]): string[] {
  const found: string[] = []
  for (const line of lines) {
    const split = splitModelId(line)
    if (split !== null && split.provider === provider && !found.includes(split.model)) {
      found.push(split.model)
    }
  }
  return found
}

/** Every `provider/model` line, deduplicated, for a picker that spans providers. */
export function allModelIds(lines: readonly string[]): string[] {
  const found: string[] = []
  for (const line of lines) {
    if (splitModelId(line) !== null && !found.includes(line)) {
      found.push(line)
    }
  }
  return found
}
