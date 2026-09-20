/**
 * Checks `src/settings/llmPools.ts` and the Models screen around it. (M45)
 *
 * `check-claude-cli.mjs`'s recipe: compile the import-free rules module standalone with the
 * TypeScript in `node_modules`, import the emitted JavaScript, and drive it — plus a handful of
 * source assertions that pin the two sides of a contract together.
 *
 * What is worth pinning here, each naming a failure this feature can have silently:
 *
 *   - **The settings group is mirrored by a patch field.** A group on `Settings` with no field on
 *     `SettingsPatch` is a screen that saves nothing, and Rust's `apply_patch` destructure cannot
 *     catch it, because there is no field to destructure. This is the one genuinely new failure
 *     class this feature introduces.
 *   - **Every provider kind Rust can produce has an editor.** A kind with no `case` draws a card
 *     with nothing in it and the build says nothing — the `SettingKind`/`renderSection` failure
 *     one screen over, which used to compile because `undefined` is a `ReactNode`.
 *   - **cide never writes opencode's `plugin` key.** Listing a plugin makes opencode install it
 *     from npm, so writing that key would turn pressing Dispatch into a package fetch that stalls
 *     the first turn and fails outright with no network. It is a one-line edit to re-add and
 *     nothing else in the suite would notice.
 *   - **The key is never masked.** A patch is per top-level field, so the screen sends the whole
 *     `LlmSettings` back on every edit; a masked field would be sent back *as the mask* and
 *     overwrite the real key.
 *   - **One joiner, one splitter.** `provider/model` is assembled in exactly one function, so the
 *     readout on screen describes the run that happens — `cide_git::push::preview`'s rule. A model
 *     id contains slashes, so a second splitter would eventually disagree about what a provider is.
 *   - **Reordering cannot drop an entry.** A pool is ordered and the order is the whole feature;
 *     an off-by-one at either end loses a row silently rather than throwing.
 *
 * What this does NOT cover: that the section renders (there is no DOM in this process); that
 * opencode accepts the document (nothing here forks a process); or what cide actually emits —
 * `cargo test -p cide-agents` owns `provider_members`, and duplicating its rules here would create
 * a second producer that could disagree with it.
 *
 * Run: `pnpm --dir ui run check:pools`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-pools-'))
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

/** Comments stripped before grepping, so a gate cannot stay green over a rule's dead prose. */
const shipping = (source) =>
  source.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

try {
  // The compile IS an assertion: if this module ever needs a tsconfig, an import has appeared and
  // the node-testability the whole file rests on is gone.
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/settings/llmPools.ts',
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

  const pools = await import(`file://${join(out, 'llmPools.js')}`)
  const {
    PROVIDER_KINDS,
    ENTRY_VERDICTS,
    KNOWN_CATALOG_IDS,
    VARIANT_SUGGESTIONS,
    DEFAULT_CUSTOM_NPM,
    blankProvider,
    isEnabled,
    isStruck,
    joinModelId,
    modelsFor,
    CODEX_PLUGIN,
    CODEX_PROVIDER_ID,
    apiKeyOf,
    entryFlags,
    splitModelId,
    judgeEntry,
    localProblems,
    poolNames,
    moveUp,
    moveDown,
    removeAt,
  } = pools

  const llmRs = repoFile('crates/cide-ipc/src/llm.rs')
  const opencodeRs = repoFile('crates/cide-agents/src/harness/opencode.rs')
  const section = uiFile('src/settings/ModelsSection.tsx')
  const sections = uiFile('src/settings/sections.tsx')
  const generated = uiFile('src/ipc/generated.ts')

  // ---------------------------------------------------------------- the group is mirrored
  /**
   * The body of one generated type. The closing `};` sits on the *same line* as the last field,
   * so this walks to the terminator rather than anchoring on a newline before it — a regex that
   * assumed the layout would silently match nothing and pass every assertion below.
   */
  const typeBody = (name) => {
    const opens = `export type ${name} = {`
    const at = generated.indexOf(opens)
    if (at < 0) return ''
    const end = generated.indexOf('};', at)
    return end < 0 ? '' : generated.slice(at + opens.length, end)
  }
  const settingsType = typeBody('Settings')
  const patchType = typeBody('SettingsPatch')
  ok(settingsType !== '' && patchType !== '', 'both generated settings types were found')
  ok(/\bllm: LlmSettings,/.test(settingsType), '`Settings` carries the `llm` group')
  ok(
    /\bllm\?: LlmSettings,/.test(patchType),
    'and `SettingsPatch` carries it too — a group with no patch field is a screen that saves ' +
      'nothing, and `apply_patch`’s destructure cannot catch it because there is no field ' +
      'to destructure',
  )

  // ------------------------------------------------- an Add button can actually add a row
  //
  // ADR 0002: the webview owns no copy of the settings — it sends the whole group to Rust and
  // redraws from the broadcast that comes back. So a row Rust refuses to *store* is a row that can
  // never appear, and a row the user has just added is blank by definition. `cleaned()` dropping
  // an incomplete row made every Add button on this screen silently inert: nothing threw, nothing
  // logged, the list just redrew unchanged. Refusing an incomplete row belongs where the value is
  // used — `provider_members` skips a blank id — not where it is stored.
  const cleanedBody = (() => {
    const at = llmRs.indexOf('pub fn cleaned(')
    return at < 0 ? '' : shipping(llmRs.slice(at, llmRs.indexOf('\n    }', at)))
  })()
  ok(cleanedBody !== '', '`LlmSettings::cleaned` was found')
  ok(
    !/\.retain\(/.test(cleanedBody),
    '`LlmSettings::cleaned` drops no row — the webview redraws from what Rust stored, so a ' +
      'dropped blank row is an Add button that does nothing at all',
  )

  // ---------------------------------------------------------------- the kinds are Rust's
  const rustKinds = [
    ...(/pub enum LlmProvider \{([\s\S]*?)\n\}/.exec(shipping(llmRs))?.[1] ?? '').matchAll(
      /^\s{4}([A-Z][A-Za-z0-9]*)\s*\{/gm,
    ),
  ].map((m) => m[1].charAt(0).toLowerCase() + m[1].slice(1))
  ok(rustKinds.length === 3, `read ${rustKinds.length} LlmProvider variants — the scan still matches`)
  eq([...PROVIDER_KINDS].sort(), [...rustKinds].sort(), 'PROVIDER_KINDS is exactly `pub enum LlmProvider`')
  for (const kind of rustKinds) {
    ok(
      section.includes(`case '${kind}':`),
      `\`${kind}\` has an editor — a kind with no case draws an empty card and nothing notices`,
    )
  }
  ok(
    /const unreachable: never = provider/.test(section),
    'and the kind switch is exhaustive by a `never`, so a fourth kind is a compile error',
  )

  // ---------------------------------------------------------------- reachability
  ok(/id: 'models'/.test(sections), "SECTIONS carries a 'models' entry, so the nav draws it")
  ok(/case 'models':/.test(sections), 'renderSection has a case for it, so the pane draws it')

  // ---------------------------------------------------------------- cide writes no plugin key
  const emitted = shipping(opencodeRs)
  ok(
    !/insert\(\s*"plugin"/.test(emitted) && !/"plugin"\s*:/.test(emitted),
    'cide never writes opencode’s `plugin` key — listing a plugin makes opencode install it ' +
      'from npm, so writing it would turn pressing Dispatch into a package fetch that stalls the ' +
      'first turn and fails outright with no network',
  )

  // ---------------------------------------------------------------- the key is never masked
  ok(
    !/type="password"/.test(section),
    'the API key is never drawn masked — a patch is per top-level field, so the screen sends the ' +
      'whole group back on every edit and a mask would overwrite the real key',
  )
  for (const word of ['workspace.json', '0600', 'plain text', 'environment']) {
    ok(
      section.includes(word),
      `the credential note still says "${word}" — copy that must not be tidied into vagueness`,
    )
  }
  ok(
    /trim=\{false\}/.test(section),
    'the key field does not trim — whitespace in a credential is the user’s to see, and ' +
      'silently editing one is how a key that works in a terminal stops working in cide',
  )

  // ---------------------------------------------------------------- one joiner, one splitter
  eq(
    entryFlags({ provider: 'openrouter', model: 'deepseek/deepseek-chat', variant: '' }),
    ['--model', 'openrouter/deepseek/deepseek-chat'],
    'entryFlags joins the two halves and omits a blank variant',
  )
  eq(
    entryFlags({ provider: 'openai', model: 'gpt-5.1-codex-max', variant: 'xhigh' }),
    ['--model', 'openai/gpt-5.1-codex-max', '--variant', 'xhigh'],
    'and spells the variant as its own flag',
  )
  for (const flag of ['"--model"', '"--variant"']) {
    ok(emitted.includes(flag), `${flag} is the spelling the harness uses`)
  }
  eq(
    joinModelId('openrouter', 'deepseek/deepseek-chat'),
    'openrouter/deepseek/deepseek-chat',
    'joinModelId is the one join, and it does not care how many slashes the model has',
  )
  ok(
    !/\$\{[^}]*provider[^}]*\}\//i.test(shipping(section)),
    'the section joins no provider and model of its own — a second joiner is how a readout starts ' +
      'describing a different run from the one that happens',
  )
  ok(
    !/\.split\('\/'\)/.test(shipping(section)),
    'and splits none either; `splitModelId` is the only splitter',
  )
  // First slash wins, over an id that contains several and one that contains a colon.
  eq(splitModelId('lmstudio/openai/gpt-oss-20b'), { provider: 'lmstudio', model: 'openai/gpt-oss-20b' }, 'the split takes the FIRST slash')
  eq(
    splitModelId('unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M'),
    { provider: 'unsloth', model: 'Qwen3.8-27B-GGUF:UD-Q4_K_M' },
    'and a colon is part of the model id',
  )
  eq(splitModelId('no-slash-here'), null, 'a line that is not an id is not one')
  eq(splitModelId('/leading'), null, 'nor is a leading slash')
  eq(splitModelId('trailing/'), null, 'nor a trailing one')

  // ---------------------------------------------------------------- ordering is safe
  const three = ['a', 'b', 'c']
  eq(moveUp(three, 0), three, 'moveUp at the front is a no-op that keeps every item')
  eq(moveDown(three, 2), three, 'moveDown at the back is a no-op that keeps every item')
  eq(moveUp(three, -1), three, 'and an out-of-range index changes nothing')
  eq(moveDown(three, 99), three, 'at either end')
  eq(moveUp(three, 1), ['b', 'a', 'c'], 'moveUp swaps with the item before')
  eq(moveDown(three, 0), ['b', 'a', 'c'], 'moveDown swaps with the item after')
  for (const [label, moved] of [['up', moveUp(three, 2)], ['down', moveDown(three, 1)]]) {
    eq(moved.length, three.length, `a move ${label} preserves length`)
    eq([...moved].sort(), [...three].sort(), `a move ${label} preserves the multiset`)
  }
  eq(removeAt(three, 1), ['a', 'c'], 'a remove preserves the order of the rest')
  eq(removeAt(three, 9), three, 'and an out-of-range remove drops nothing')

  // ---------------------------------------------------------------- verdicts are all drawn
  const providers = [
    { kind: 'catalog', id: 'openrouter', label: '', enabled: true, apiKey: 'k' },
    { kind: 'catalog', id: 'groq', label: '', enabled: false, apiKey: 'k' },
    { kind: 'external', id: 'openai', label: '', setup: 's', expect: [] },
  ]
  eq(judgeEntry({ provider: 'openrouter', model: 'm', variant: '' }, providers), 'ok', 'a configured, enabled provider is ok')
  eq(judgeEntry({ provider: 'groq', model: 'm', variant: '' }, providers), 'disabledProvider', 'a switched-off provider is reported')
  eq(judgeEntry({ provider: 'nope', model: 'm', variant: '' }, providers), 'unknownProvider', 'an unconfigured provider is reported, not deleted')
  eq(
    judgeEntry({ provider: 'openrouter', model: '  ', variant: '' }, providers),
    'incomplete',
    'a half-typed row is `incomplete`, not an error',
  )
  eq(
    judgeEntry({ provider: '', model: '', variant: '' }, providers),
    'incomplete',
    'and so is the blank row an Add button just produced',
  )
  // The distinction that stops a brand-new pool looking broken: only a row naming something
  // WRONG is struck through.
  ok(!isStruck('incomplete'), 'an incomplete row is not struck through')
  ok(!isStruck('ok'), 'nor is a good one')
  ok(isStruck('unknownProvider') && isStruck('disabledProvider'), 'a wrong row is')
  eq(
    judgeEntry({ provider: 'openai', model: 'gpt-5.2', variant: '' }, providers),
    'ok',
    'an external provider is poolable — there is nothing for a switch to gate, so it is always on',
  )
  // `ok` needs no branch — it is the absence of a complaint — so what must hold is that every
  // verdict that *says* something has somewhere to say it, and that the strike-through decision is
  // taken in one place rather than re-derived at each call site.
  for (const verdict of ENTRY_VERDICTS.filter((v) => v !== 'ok')) {
    ok(section.includes(`'${verdict}'`), `the screen draws the \`${verdict}\` verdict`)
  }
  ok(
    /isStruck\(/.test(section),
    'and the strike-through comes from `isStruck`, not a `!== ok` the next verdict would join',
  )

  // ---------------------------------------------------------------- local refusals
  const bareCustom = blankProvider('custom')
  const problems = localProblems({ ...bareCustom, id: 'ollama', baseUrl: 'http://x/v1' })
  ok(problems.length === 1, `a custom endpoint with no models is refused (saw ${problems.length})`)
  ok(
    problems[0].length > 20 && /model/i.test(problems[0]),
    'and the refusal is a sentence naming what is missing',
  )
  eq(
    localProblems({ ...bareCustom, id: 'ollama', baseUrl: 'http://x/v1', models: [{ id: 'm', label: '', context: 0, output: 0 }] }),
    [],
    'and a complete one is refused for nothing else — this must not grow into a second validator',
  )
  eq(
    localProblems({ kind: 'catalog', id: 'openrouter', label: '', enabled: true, apiKey: '' }),
    [],
    'a catalogued provider with no key is NOT a problem: the key may already be in the environment',
  )

  // Measured against opencode 1.18.29: `limit` is all-or-nothing, and a document carrying one key
  // without the other is REFUSED — which makes the run silently opencode's default agent.
  const withModels = (models) => ({ ...bareCustom, id: 'ollama', baseUrl: 'http://x/v1', models })
  eq(
    localProblems(withModels([{ id: 'm', label: '', context: 32768, output: 0 }])).length,
    1,
    'half a limit pair is reported before the save',
  )
  eq(
    localProblems(withModels([{ id: 'm', label: '', context: 0, output: 4096 }])).length,
    1,
    'in either direction',
  )
  eq(
    localProblems(withModels([{ id: 'm', label: '', context: 32768, output: 4096 }])),
    [],
    'and a complete pair is fine',
  )
  eq(
    localProblems(withModels([{ id: 'm', label: '', context: 0, output: 0 }])),
    [],
    'as is neither, which is what "the user gave no number" looks like',
  )

  // ------------------------------------------- the same four rules, twice, on purpose (M71)
  //
  // `LlmProvider::problems` in `cide-ipc` states these rules a second time, and the duplication is
  // deliberate rather than an oversight: the two answer for callers that cannot be treated alike.
  // `localProblems` is a **hint beside a form somebody is still typing into** and must not block
  // the save, because `LlmSettings::cleaned`'s whole argument is that a half-filled row has to
  // survive storage or the Add button is inert. The Rust one answers `cide_llm_provider`, whose
  // caller is a model handing over a whole row at once with no next keystroke — so there an
  // incomplete row is a refusal, made before anything is written.
  //
  // What must not happen is one of them losing a rule. Neither side fails to compile for it: the
  // screen would simply stop warning, or the tool would store a provider that resolves
  // `--model <id>/<model>` to nothing and leave the run silently opencode's default agent. So the
  // four are named here and asserted on both sides.
  const RULES = [
    { what: 'a provider with no id', rust: 'needs an id', ts: 'needs an id' },
    { what: 'a custom endpoint with no base URL', rust: 'base URL', ts: 'base URL' },
    { what: 'half a limit pair', rust: 'only one of its two limits', ts: 'only one of its two limits' },
    { what: 'a custom endpoint declaring no models', rust: 'must declare its models', ts: 'must declare its models' },
  ]
  const rustProblems =
    /pub fn problems\(&self\) -> Vec<String> \{([\s\S]*?)\n    \}/.exec(
      shipping(repoFile('crates/cide-ipc/src/llm.rs')),
    )?.[1] ?? ''
  ok(rustProblems.length > 0, 'found `LlmProvider::problems` in cide-ipc')
  const tsProblems =
    /export function localProblems\(provider: Provider\): string\[\] \{([\s\S]*?)\n\}/.exec(
      shipping(uiFile('src/settings/llmPools.ts')),
    )?.[1] ?? ''
  ok(tsProblems.length > 0, 'found `localProblems` in llmPools.ts')
  for (const rule of RULES) {
    ok(rustProblems.includes(rule.rust), `Rust refuses ${rule.what}`)
    ok(tsProblems.includes(rule.ts), `the screen warns about ${rule.what}`)
  }
  // And neither has grown a rule the other has never heard of, which is the drift in the
  // direction a phrase list cannot see.
  eq(
    (rustProblems.match(/problems\.push/g) ?? []).length,
    RULES.length,
    'Rust states exactly the rules named above — a new one belongs in RULES and on both sides',
  )
  eq(
    (tsProblems.match(/problems\.push/g) ?? []).length,
    RULES.length,
    'and so does the screen',
  )

  // ---------------------------------------------------------------- blanks, defaults, helpers
  eq(blankProvider('catalog').enabled, true, 'a new provider is enabled — the `LlmProvider::default` rule')
  eq(blankProvider('custom').npm, DEFAULT_CUSTOM_NPM, 'a custom endpoint is prefilled with the OpenAI-compatible SDK')
  eq(isEnabled(blankProvider('external')), true, 'an external provider is always on')
  eq(apiKeyOf(blankProvider('external')), '', 'and carries no credential at all')
  ok(KNOWN_CATALOG_IDS.length >= 5, 'there are provider ids worth suggesting')
  ok(KNOWN_CATALOG_IDS.includes('openrouter') && KNOWN_CATALOG_IDS.includes('deepseek'), 'including the two asked for')
  ok(VARIANT_SUGGESTIONS.includes('xhigh'), 'the codex models’ top variant is offered')
  eq(
    poolNames({ providers: [], pools: [{ name: 'a', description: '', entries: [] }, { name: '', description: '', entries: [] }] }),
    ['a'],
    'poolNames skips a pool that has not been named yet',
  )
  ok(!PROVIDER_KINDS.includes('constructor'), 'the kind list is data, not a prototype walk')

  // ------------------------------------------------- the codex row is pre-filled, not blank
  //
  // The button says "Add codex subscription", so the row it produces must already be one. A blank
  // `external` row would ask the user to know a package name and a provider id that cide has
  // right here — and cide keeps no table of plugins to offer a second one from, which is why
  // there is no generic "plugin-backed provider" button any more.
  const codex = blankProvider('external')
  eq(codex.id, CODEX_PROVIDER_ID, 'the codex row names the provider its plugin registers')
  ok(codex.setup.includes(CODEX_PLUGIN), 'and its setup names the package to install')
  ok(codex.setup.includes('opencode auth login'), 'and the login step')
  ok(codex.expect.length > 0, 'and the ids whose presence proves it worked')
  ok(
    codex.expect.every((id) => id.startsWith(`${CODEX_PROVIDER_ID}/`)),
    'which are all under that provider',
  )
  ok(
    !section.includes("'Add a known provider'") || section.includes('cannot be created here'),
    'the catalogue button is gone, or the comment says why the kind survives without one',
  )

  // ------------------------------------------------- a model can be picked, not only typed
  const lines = [
    'openrouter/deepseek/deepseek-chat',
    'openrouter/anthropic/claude-sonnet-4-5',
    'lmstudio/qwen/qwen3-coder-30b',
    'not-an-id',
  ]
  eq(
    modelsFor('openrouter', lines),
    ['deepseek/deepseek-chat', 'anthropic/claude-sonnet-4-5'],
    'modelsFor returns the right halves under one provider, slashes and all',
  )
  eq(modelsFor('nobody', lines), [], 'and nothing for a provider the probe never saw')
  ok(
    /aria-label="Pick a model"/.test(section),
    'a pool entry offers a picker beside the box — the ids the probe reported, with the box still ' +
      'typable for one it has not',
  )
  ok(
    /aria-label="Display name"/.test(section) || /Display name/.test(section),
    'the second model column says what it is; "(optional)" said only that it may be blank',
  )

  // ------------------------------------------------- collapsed by default
  ok(
    /useState<ReadonlySet<number>>\(\(\) => new Set\(\)\)/.test(section),
    'provider cards start collapsed — the screen opens as a list of headings, not a wall of forms',
  )
  ok(/aria-expanded=/.test(section), 'and the disclosure says so to a screen reader')

  // ------------------------------------------------- local overrides (M45)
  const agentsSection = uiFile('src/settings/AgentsSection.tsx')
  const overridesRs = repoFile('crates/cide-agents/src/overrides.rs')
  const overrideFields = [
    ...(/pub struct AgentOverride \{([\s\S]*?)\n\}/.exec(shipping(repoFile('crates/cide-ipc/src/overrides.rs')))?.[1] ?? '').matchAll(
      /pub ([a-z_]+):/g,
    ),
  ].map((m) => m[1].replace(/_([a-z])/g, (_, c) => c.toUpperCase()))
  ok(overrideFields.length === 5, `read ${overrideFields.length} AgentOverride fields`)
  for (const field of overrideFields) {
    ok(
      agentsSection.includes(`'${field}'`),
      `the Agents screen edits \`${field}\` — a field Rust reads and no control writes is a ` +
        'redirection nobody can make',
    )
  }

  // An unset override field must be **absent** on the wire, not `null`.
  //
  // `#[ts(optional)]` types the field optional and does nothing to serde, which writes `None` as
  // `null`. The screen derives each row's mode from whether a field is set, and `null !==
  // undefined` — so a null coming back is a control the user can move away from "leave as
  // committed" and never move back. Invisible from either side alone: Rust looks right, the
  // TypeScript type looks right, and only the round trip is wrong.
  const overridesRsSrc = repoFile('crates/cide-ipc/src/overrides.rs')
  const optionalCount = (shipping(overridesRsSrc).match(/#\[ts\(optional\)\]/g) ?? []).length
  const skipCount = (shipping(overridesRsSrc).match(/skip_serializing_if = "Option::is_none"/g) ?? [])
    .length
  ok(optionalCount > 0, `AgentOverride has optional fields (saw ${optionalCount})`)
  eq(
    skipCount,
    optionalCount,
    'every `#[ts(optional)]` override field also skips serialising `None` — otherwise a cleared ' +
      'field comes back as `null`, and the control can never be returned to "leave as committed"',
  )
  // And the screen reads them null-tolerantly anyway, for a file written before that annotation.
  ok(
    /value\.pool != null/.test(agentsSection),
    'the mode test uses `!= null`, so a stored null is still read as unset',
  )

  // A key omitted and a key set to `undefined` are different types under
  // `exactOptionalPropertyTypes`, and only the first reads back as `None`. Assigning `undefined`
  // compiles under a laxer config and sends a key the backend cannot parse.
  ok(
    /function withField</.test(agentsSection),
    'an override field is cleared by omitting the key, through `withField`',
  )
  ok(
    !/harness: undefined|pool: undefined|model: undefined/.test(shipping(agentsSection)),
    'and never by assigning `undefined` to it',
  )

  // The rule `defs` states as "a Claude Code subagent runs under Claude Code; there is no second
  // answer". Offering the override would promise something the fork reverses in silence.
  ok(
    /claudeProject/.test(agentsSection) && /claudeGlobal/.test(agentsSection),
    'the screen does not offer to override a Claude Code role',
  )
  ok(
    /is_claude_code\(\)/.test(shipping(overridesRs)),
    'and `overrides::resolve` refuses it too, so the screen is not the only guard',
  )

  // The one refusal, and the reason it is a refusal rather than a fallback: people build a pool
  // to cap spend, so falling back would answer with a model nobody chose and bill for it.
  ok(
    /refusal = Some\(format!\(/.test(shipping(overridesRs)),
    'a pool an override names and settings does not have refuses the dispatch',
  )
  ok(
    /not committed/i.test(agentsSection),
    'and the override block says it is not committed — it means the opposite of the ' +
      '`.cide/config.json` row beside it',
  )

  console.log(
    `check-pools: ${PROVIDER_KINDS.length} provider kinds, ${ENTRY_VERDICTS.length} entry verdicts, ` +
      'one joiner and one splitter',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`${failed} failure(s)`)
  process.exit(1)
}
