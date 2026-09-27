/**
 * Settings ▸ Models: the providers cide configures, and the ordered pools a run falls down. (M45)
 *
 * # What this screen is responsible for, and what it deliberately is not
 *
 * It edits `Settings.llm` and nothing else. It does **not** decide what cide writes into an
 * opencode configuration — `cide_agents::harness::opencode::provider_members` is the only producer
 * of that document, and the model probe below is fed the very same one, so what this screen lists
 * is what a run can actually reach. A second set of emission rules here would be a screen that
 * could describe a different run from the one that happens, which is the failure
 * `cide_git::push::preview` names.
 *
 * Every rule it *does* own lives in `./llmPools`, which is import-free so `check:pools` can
 * compile and drive it under node. A rule inside a component is a rule no check script can drive.
 *
 * # Three kinds of provider, and only two of them are cide's to write
 *
 * `catalog` is a provider opencode's own 213-entry catalog already describes: cide stores a
 * credential and nothing else. `custom` is an endpoint the catalog has never heard of, which must
 * declare its models or `--model id/model` resolves to nothing. `external` is a provider somebody
 * else installs — the ChatGPT/codex plugin — for which cide writes not one byte, and this screen's
 * whole job is to explain the steps and then say whether they worked.
 *
 * # The key is shown in the clear, and that is the considered answer
 *
 * A masked field cannot work in this shape: a patch is per top-level field, so this screen sends
 * the whole `LlmSettings` back on every edit, and a mask would be sent back *as the mask* and
 * overwrite the real key. What survives is `ClaudeCliSection`'s rule — a value you cannot see is a
 * value you cannot correct — and a `Note` saying plainly where the bytes end up.
 */
import { Select } from '@/kit/components/Select'
import { IconButton } from '@/kit/components/Button'
import { Badge } from '@/kit/components/Status'
import { useCallback, useRef, useState } from 'react'

import { Icon } from '@/icons'
import type {
  AgentModels,
  LlmLimitsProbe,
  LlmModelTest,
  LlmSettings,
  SettingsPatch,
} from '@/ipc/generated'

import { ActionButton, Group, Note, TextField, ToggleRow } from './controls'
import controls from './controls.module.css'
import {
  CODEX_PLUGIN_SETUP,
  DEFAULT_CUSTOM_NPM,
  KNOWN_CATALOG_IDS,
  VARIANT_SUGGESTIONS,
  blankProvider,
  entryFlags,
  entryModelChoices,
  isEnabled,
  isStruck,
  joinModelId,
  judgeEntry,
  localProblems,
  moveDown,
  moveUp,
  poolCapacity,
  removeAt,
  withMaxRunning,
  type Entry,
  type Model,
  type Pool,
  type Provider,
  type ProviderKind,
} from './llmPools'
import styles from './ModelsSection.module.css'

/** Datalist ids, so the suggestion lists are declared once and referenced by name. */
const CATALOG_LIST = 'cide-llm-catalog-ids'
const VARIANT_LIST = 'cide-llm-variants'

export interface ModelsSectionProps {
  settings: LlmSettings
  patch: (patch: SettingsPatch) => void
  /** What `opencode models` reported, with cide's own document injected. `null` before it lands. */
  models: AgentModels | null
  /** Run the probe again, after a key or an endpoint changed. */
  /**
   * Try one `provider/model` for real. Spends a very small amount of quota, so it is only ever
   * wired to a button — never to a keystroke or a mount.
   */
  testModel: (model: string, variant?: string) => Promise<LlmModelTest | null>
  /** A custom model's limits from its server. Free; see `ModelRows`. (M88) */
  probeLimits: (provider: string, model: string) => Promise<LlmLimitsProbe | null>
}

export function ModelsSection({
  settings,
  patch,
  models,
  testModel,
  probeLimits,
}: ModelsSectionProps) {
  const write = (next: LlmSettings) => patch({ llm: next })
  const providers = settings.providers as Provider[]
  const pools = settings.pools as Pool[]

  /**
   * Which provider cards are expanded, by index.
   *
   * Empty to begin with, so the screen opens as a list of headings rather than a wall of forms —
   * a person arriving here is usually looking for one provider, not reading all of them. Held in
   * the component and not in `Settings`, because which card somebody has open is transient
   * gesture state and the store's own rule (ADR 0002) is that the webview owns exactly that.
   */
  const [openProviders, setOpenProviders] = useState<ReadonlySet<number>>(() => new Set())
  const toggleProvider = useCallback((index: number) => {
    setOpenProviders((open) => {
      const next = new Set(open)
      if (!next.delete(index)) next.add(index)
      return next
    })
  }, [])

  const setProvider = (index: number, next: Provider) =>
    write({ ...settings, providers: providers.map((p, i) => (i === index ? next : p)) })
  const setPool = (index: number, next: Pool) =>
    write({ ...settings, pools: pools.map((p, i) => (i === index ? next : p)) })

  return (
    <>
      {/*
       * Declared once, at the top, rather than per input: a `<datalist>` is referenced by id and
       * repeating it per row would put N copies of the same ten options in the document.
       */}
      <datalist id={CATALOG_LIST}>
        {KNOWN_CATALOG_IDS.map((id) => (
          <option key={id} value={id} />
        ))}
      </datalist>
      <datalist id={VARIANT_LIST}>
        {VARIANT_SUGGESTIONS.map((v) => (
          <option key={v} value={v} />
        ))}
      </datalist>

      <Group title="Providers">
        <Note title="Where a key is stored" tone="warn">
          A key you type here is stored in plain text in <code>workspace.json</code>, which cide
          writes <code>0600</code>, and it is passed to every opencode child in its environment.
          cide has no keyring. <strong>Leave the box empty if the key is already in your
          environment</strong> — a child inherits this process&rsquo;s environment wholesale, and
          opencode reads each provider&rsquo;s own variable at highest precedence, so nothing needs
          to be stored at all. The check below says which way worked.
        </Note>
        {providers.map((provider, index) => (
          <ProviderCard
            key={`${provider.kind}-${index}`}
            provider={provider}
            open={openProviders.has(index)}
            onToggle={() => toggleProvider(index)}
            onChange={(next) => setProvider(index, next)}
            onRemove={() => write({ ...settings, providers: removeAt(providers, index) })}
            testModel={testModel}
            probeLimits={probeLimits}
          />
        ))}
        <div className={styles.actions}>
          {/*
           * Two buttons, not three. "Add a known provider" is gone because opencode already reads
           * a catalogued provider's key straight from the environment or from the user's own
           * config, so cide storing a copy of it earned nothing; a `catalog` row that already
           * exists still loads and still works, it simply cannot be created here any more. And a
           * generic "plugin-backed provider" is gone because cide keeps no table of plugins —
           * the button asked the user to know a package name cide could just fill in. What is
           * left is the one anybody actually wants, pre-filled.
           */}
          {(['external', 'custom'] as ProviderKind[]).map((kind) => (
            <ActionButton
              key={kind}
              label={addLabel(kind)}
              onClick={() => {
                write({ ...settings, providers: [...providers, blankProvider(kind)] })
                // Opened, because a row somebody just asked for is a row they want to fill in —
                // collapsed-by-default is about arriving at a screenful of existing providers.
                setOpenProviders((open) => new Set(open).add(providers.length))
              }}
            />
          ))}
        </div>
      </Group>

      <Group title="Pools">
        <Note title="What a pool is">
          An <strong>ordered</strong> list of models a run falls down. A run takes the first entry
          and, if that provider rate-limits, cannot be reached, or refuses the credential, retries
          on the next — and then stays there for the rest of the run. Only opencode runs use a
          pool; a pool is chosen for a role in the Agents screen, as a local override that is never
          committed.
        </Note>
        {pools.map((pool, index) => (
          <PoolCard
            key={index}
            pool={pool}
            providers={providers}
            offered={models?.models ?? []}
            variantInModel={models?.variantInModel ?? false}
            testModel={testModel}
            onChange={(next) => setPool(index, next)}
            onRemove={() => write({ ...settings, pools: removeAt(pools, index) })}
          />
        ))}
        <div className={styles.actions}>
          <ActionButton
            label="Add pool"
            onClick={() =>
              write({
                ...settings,
                pools: [...pools, { name: '', description: '', entries: [] }],
              })
            }
          />
        </div>
      </Group>

    </>
  )
}

function addLabel(kind: ProviderKind): string {
  switch (kind) {
    // Reachable only for a row already stored: there is no button for this kind any more.
    case 'catalog':
      return 'Add a known provider'
    case 'custom':
      return 'Add a custom endpoint'
    case 'external':
      return 'Add codex subscription'
    default: {
      const unreachable: never = kind
      return unreachable
    }
  }
}

/**
 * One provider row.
 *
 * The `switch` has an arm per kind and a `never` on the end, so a fourth kind is a compile error
 * rather than a card that draws nothing — the `SettingKind`/`renderSection` failure one screen
 * over, which used to compile because `undefined` is a `ReactNode`.
 */
function ProviderCard({
  provider,
  open,
  onToggle,
  onChange,
  onRemove,
  testModel,
  probeLimits,
}: {
  provider: Provider
  open: boolean
  onToggle: () => void
  onChange: (next: Provider) => void
  onRemove: () => void
  testModel: (model: string, variant?: string) => Promise<LlmModelTest | null>
  probeLimits: (provider: string, model: string) => Promise<LlmLimitsProbe | null>
}) {
  const problems = localProblems(provider)
  return (
    <div className={styles.card}>
      <div className={styles.cardHead}>
        {/*
         * The heading is the disclosure. A separate chevron button would give the row two hit
         * targets that do the same thing, and the title is the larger and more obvious one.
         */}
        <button
          type="button"
          className={styles.disclosure}
          aria-expanded={open}
          onClick={onToggle}
        >
          <Icon name={open ? 'chevron-down' : 'chevron-right'} size={1} className={styles.chevron ?? ''} />
          <span className={styles.cardTitle}>
            {provider.label === '' ? provider.id || 'New provider' : provider.label}
          </span>
        </button>
        <Badge tone="neutral" soft squared>
          {provider.kind}
        </Badge>
        <IconButton
              icon="trash-2"
              label="Remove provider"
          onClick={onRemove}
            />
      </div>

      {!open ? null : (
      <>
      {provider.kind !== 'external' && (
        <ToggleRow
          label="Enabled"
          hint="When off, cide writes nothing for this provider and a pool entry naming it is skipped."
          checked={provider.enabled}
          onChange={(enabled) => onChange({ ...provider, enabled })}
        />
      )}

      <TextField
        label="Provider id"
        hint="The left half of every provider/model, and the key opencode files it under."
        value={provider.id}
        placeholder="openrouter"
        onCommit={(id) => onChange({ ...provider, id })}
      />
      <TextField
        label="Label"
        hint="What this screen calls it. Blank means the id speaks for itself."
        value={provider.label}
        placeholder={provider.id}
        onCommit={(label) => onChange({ ...provider, label })}
      />

      {providerBody(provider, onChange, probeLimits)}

      {problems.map((problem) => (
        <p key={problem} className={styles.problem}>
          {problem}
        </p>
      ))}
      {/*
       * A per-model test over the models **this card declares** — cide's providers are the whole
       * configuration a run gets, so what opencode would list from the user's own opencode.json
       * is not this screen's business (t-1090: the user asked for "opencode offers N models" and
       * "What opencode sees" to go, for exactly that reason). A declared id proves nothing on its
       * own: a wrong key, an endpoint that will not answer a completion and a retired model each
       * fail on the first token, which is what the button finds out.
       */}
      {declaredModels(provider).length > 0 && (
        <TestableModels
          provider={provider.id}
          models={declaredModels(provider)}
          testModel={testModel}
        />
      )}
      </>
      )}
    </div>
  )
}

/**
 * The models a provider offers, each with a button that tries it for real.
 *
 * One request at a time and one result per model, keyed by the model id — a late answer must not
 * be drawn against a different row, which is the same reason `LlmModelTest` echoes the model it
 * tried.
 */
function TestableModels({
  provider,
  models,
  testModel,
}: {
  provider: string
  models: readonly string[]
  testModel: (model: string, variant?: string) => Promise<LlmModelTest | null>
}) {
  const [results, setResults] = useState<Record<string, LlmModelTest | 'running'>>({})
  const run = (model: string) => {
    const id = joinModelId(provider, model)
    setResults((prev) => ({ ...prev, [model]: 'running' }))
    void testModel(id).then((answer) => {
      setResults((prev) => ({
        ...prev,
        [model]:
          answer ??
          // A build with no such handler. Said plainly rather than drawn as a failed model.
          { model: id, ok: false, detail: 'This build has no model test.' },
      }))
    })
  }
  return (
    <div>
      <span className={controls.label}>Test a model</span>
      <p className={styles.modelHead}>
        Sends one very small prompt and waits for an answer, which spends a little of that
        provider&rsquo;s quota.
      </p>
      {models.map((model) => {
        const result = results[model]
        return (
          <div key={model} className={styles.modelRow}>
            <input className={styles.entryField} readOnly value={model} aria-label="Model" />
            <ActionButton
              label={result === 'running' ? 'Testing…' : 'Test'}
              disabled={result === 'running'}
              onClick={() => run(model)}
            />
            {result !== undefined && result !== 'running' && (
              <span className={result.ok ? styles.testResultOk : styles.testResultBad}>
                {result.detail}
              </span>
            )}
          </div>
        )
      })}
    </div>
  )
}

/** The fields that differ per kind. Exhaustive, with a `never` on the end. */
function providerBody(
  provider: Provider,
  onChange: (next: Provider) => void,
  probeLimits: (provider: string, model: string) => Promise<LlmLimitsProbe | null>,
) {
  switch (provider.kind) {
    case 'catalog':
      return (
        <>
          <TextField
            label="API key"
            hint={
              'Blank is a real answer: if this key is already exported in the environment cide ' +
              'was launched from, opencode reads it there and nothing needs storing here.'
            }
            value={provider.apiKey}
            placeholder="leave blank to use the environment"
            // Never trimmed. Whitespace in a credential is the user's to see, and silently
            // editing one is how a key that works in a terminal stops working in cide.
            trim={false}
            onCommit={(apiKey) => onChange({ ...provider, apiKey })}
          />
          <p className={styles.problem} hidden>
            {/* Suggestions are attached to the id box through the shared datalist. */}
          </p>
        </>
      )
    case 'custom':
      return (
        <>
          <TextField
            label="Base URL"
            hint="Where the OpenAI-compatible endpoint listens."
            value={provider.baseUrl}
            placeholder="http://127.0.0.1:11434/v1"
            onCommit={(baseUrl) => onChange({ ...provider, baseUrl })}
          />
          <TextField
            label="npm package"
            hint="The SDK opencode should drive this endpoint with."
            value={provider.npm}
            placeholder={DEFAULT_CUSTOM_NPM}
            onCommit={(npm) => onChange({ ...provider, npm })}
          />
          <TextField
            label="API key"
            hint="Usually empty for a local endpoint."
            value={provider.apiKey}
            placeholder="(none)"
            trim={false}
            onCommit={(apiKey) => onChange({ ...provider, apiKey })}
          />
          <ModelRows
            provider={provider.id}
            models={provider.models as Model[]}
            onChange={(models) => onChange({ ...provider, models })}
            probeLimits={probeLimits}
          />
        </>
      )
    case 'external':
      return (
        <>
          <Note title="cide does not configure this one">{provider.setup || CODEX_PLUGIN_SETUP}</Note>
          <TextField
            label="Setup steps"
            hint="Shown above. Editable, because cide keeps no table of plugins to derive it from."
            value={provider.setup}
            placeholder={CODEX_PLUGIN_SETUP}
            onCommit={(setup) => onChange({ ...provider, setup })}
          />
          <TextField
            label="Expect"
            hint={
              'Comma-separated model ids whose presence proves the setup worked. Blank means any ' +
              'id under this provider counts.'
            }
            value={provider.expect.join(', ')}
            placeholder="openai/gpt-5.2"
            onCommit={(text) =>
              onChange({
                ...provider,
                expect: text
                  .split(',')
                  .map((part) => part.trim())
                  .filter((part) => part !== ''),
              })
            }
          />
        </>
      )
    default: {
      const unreachable: never = provider
      return unreachable
    }
  }
}

/** The sentence for a provider the probe did not see, which differs by what cide could do about it. */
/** The model ids a provider's card declares — none for a kind that stores no model list. */
function declaredModels(provider: Provider): string[] {
  if (provider.kind !== 'custom') return []
  const ids: string[] = []
  for (const model of provider.models) {
    const id = model.id.trim()
    if (id !== '' && !ids.includes(id)) ids.push(id)
  }
  return ids
}

/**
 * The model list a custom endpoint must declare.
 *
 * # Limits fill themselves (M88)
 *
 * opencode never asks a custom server for a model's window, and a model declared without one runs
 * with no compaction threshold. The server usually says — `llm_probe_limits` reads it — so a row
 * whose two number boxes are still empty is filled when its model id is committed (on blur), and
 * the refresh button asks again on demand. The automatic road **never overwrites** a number the
 * user typed; the button does, because pressing it is asking for the server's figure.
 *
 * The answer lands through `latest`, not the `models` the request was made from: the probe takes
 * a second over a network, and a row list captured at the request would revert whatever was typed
 * meanwhile. It is matched by model id, which the answer echoes, so a row retyped in the meantime
 * is left alone.
 */
function ModelRows({
  provider,
  models,
  onChange,
  probeLimits,
}: {
  provider: string
  models: Model[]
  onChange: (next: Model[]) => void
  probeLimits: (provider: string, model: string) => Promise<LlmLimitsProbe | null>
}) {
  const set = (index: number, next: Model) =>
    onChange(models.map((m, i) => (i === index ? next : m)))
  const latest = useRef({ models, onChange })
  latest.current = { models, onChange }
  const [probes, setProbes] = useState<Record<string, LlmLimitsProbe | 'running'>>({})
  const detect = (model: Model, overwrite: boolean) => {
    const id = model.id.trim()
    if (id === '' || provider === '') return
    setProbes((was) => ({ ...was, [id]: 'running' }))
    void probeLimits(provider, id).then((answer) => {
      setProbes((was) => ({
        ...was,
        [id]: answer ?? {
          provider,
          model: id,
          context: 0,
          output: 0,
          outputEstimated: false,
          detail: 'This build cannot read limits from a server.',
        },
      }))
      if (answer === null || answer.context === 0) return
      const now = latest.current
      const at = now.models.findIndex((m) => m.id.trim() === answer.model)
      const row = now.models[at]
      if (row === undefined) return
      if (!overwrite && (row.context !== 0 || row.output !== 0)) return
      now.onChange(
        now.models.map((m, i) =>
          i === at ? { ...m, context: answer.context, output: answer.output } : m,
        ),
      )
    })
  }
  return (
    <div>
      <span className={controls.label}>Models</span>
      {/*
       * A header row rather than four placeholders. The second box was labelled "(optional)",
       * which said that it may be left blank and nothing about what it *is*; a column heading
       * says both at once and costs no vertical space per model.
       */}
      <div className={styles.modelHead}>
        <span className={styles.modelHeadId}>Model id</span>
        <span className={styles.modelHeadId}>Display name</span>
        <span className={styles.modelHeadNumber}>Context</span>
        <span className={styles.modelHeadNumber}>Output</span>
      </div>
      {models.map((model, index) => (
        <div key={index}>
          <div className={styles.modelRow}>
            <input
              className={styles.entryField}
              aria-label="Model id"
              placeholder="qwen3:8b"
              value={model.id}
              onChange={(e) => set(index, { ...model, id: e.target.value })}
              onBlur={() => {
                if (model.context === 0 && model.output === 0) detect(model, false)
              }}
            />
            <input
              className={styles.entryField}
              aria-label="Model label"
              placeholder="shown in menus"
              value={model.label}
              onChange={(e) => set(index, { ...model, label: e.target.value })}
            />
            <input
              className={`${styles.entryField} ${styles.modelNumber}`}
              aria-label="Context limit"
              placeholder="tokens"
              inputMode="numeric"
              value={model.context === 0 ? '' : String(model.context)}
              onChange={(e) => set(index, { ...model, context: numberOf(e.target.value) })}
            />
            <input
              className={`${styles.entryField} ${styles.modelNumber}`}
              aria-label="Output limit"
              placeholder="tokens"
              inputMode="numeric"
              value={model.output === 0 ? '' : String(model.output)}
              onChange={(e) => set(index, { ...model, output: numberOf(e.target.value) })}
            />
            <IconButton
              icon="refresh-cw"
              label="Read this model's context and output limits from the server"
              disabled={model.id.trim() === '' || probes[model.id.trim()] === 'running'}
              onClick={() => detect(model, true)}
            />
            <IconButton
              icon="x"
              label="Remove model"
              onClick={() => onChange(removeAt(models, index))}
            />
          </div>
          <LimitsNote probe={probes[model.id.trim()]} />
        </div>
      ))}
      <div className={styles.actions}>
        <ActionButton
          label="Add model"
          onClick={() => onChange([...models, { id: '', label: '', context: 0, output: 0 }])}
        />
      </div>
    </div>
  )
}

/** What the server said about one row's limits, under it — or nothing before it was asked. */
function LimitsNote({ probe }: { probe: LlmLimitsProbe | 'running' | undefined }) {
  if (probe === undefined) return null
  if (probe === 'running') return <span className={styles.limitsNote}>Asking the server…</span>
  return (
    <span className={probe.context > 0 ? styles.limitsNote : styles.testResultBad}>
      {probe.detail}
    </span>
  )
}

/** A blank or unparseable limit is `0`, which `provider_members` reads as "do not write one". */
function numberOf(text: string): number {
  const parsed = Number.parseInt(text, 10)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0
}

/** One pool, with its ordered entries and the argv they resolve to. */
function PoolCard({
  pool,
  providers,
  offered,
  variantInModel,
  testModel,
  onChange,
  onRemove,
}: {
  pool: Pool
  providers: readonly Provider[]
  /** Every `provider/model` the probe reported, so an entry can be picked rather than typed. */
  offered: readonly string[]
  /** How the installed CLI spells an effort variant — see `entryFlags`. */
  variantInModel: boolean
  testModel: (model: string, variant?: string) => Promise<LlmModelTest | null>
  onChange: (next: Pool) => void
  onRemove: () => void
}) {
  const entries = pool.entries as Entry[]
  const set = (index: number, next: Entry) =>
    onChange({ ...pool, entries: entries.map((e, i) => (i === index ? next : e)) })

  /**
   * "Check entries": one real turn per entry, **with the entry's variant**, so a check passes
   * exactly when a run falling to that entry would start. The per-model Test on a provider card
   * cannot say that: it tries the bare model, and t-1090 died on `o3/Qwen-Coder#xhigh` — a model
   * that answered perfectly on its own and was refused the moment the pool's variant was added.
   *
   * Keyed by what was checked (`checkKey`), not by row index: an entry edited, moved or removed
   * while a check is in flight must not have the old answer drawn against it. One at a time, as
   * `TestableModels` does — each check is a `--standalone` opencode, and six at once would be six
   * servers booting for a button press.
   */
  const [checks, setChecks] = useState<Record<string, LlmModelTest | 'running'>>({})
  const [checking, setChecking] = useState(false)
  const checkAll = async () => {
    setChecking(true)
    const due = entries.filter((entry) => !isStruck(judgeEntry(entry, providers)))
    setChecks(Object.fromEntries(due.map((entry) => [checkKey(entry), 'running' as const])))
    for (const entry of due) {
      const id = joinModelId(entry.provider, entry.model)
      const answer = await testModel(id, entry.variant)
      setChecks((prev) => ({
        ...prev,
        [checkKey(entry)]: answer ?? { model: id, ok: false, detail: 'This build has no model test.' },
      }))
    }
    setChecking(false)
  }
  return (
    <div className={styles.card}>
      <div className={styles.cardHead}>
        <span className={styles.cardTitle}>{pool.name === '' ? 'New pool' : pool.name}</span>
        <IconButton
              icon="trash-2"
              label="Remove pool" onClick={onRemove}
            />
      </div>
      <TextField
        label="Name"
        hint="What a role's local override names to use this pool."
        value={pool.name}
        placeholder="cheap-first"
        onCommit={(name) => onChange({ ...pool, name })}
      />
      <TextField
        label="Description"
        hint="For you. Nothing a model ever sees."
        value={pool.description}
        placeholder="cheap first, then the subscription"
        onCommit={(description) => onChange({ ...pool, description })}
      />

      {/*
       * One grid for the heading and every entry, rather than a flex row per entry: with each
       * field `flex: 1` the provider, the model and the variant split the row in thirds, and the
       * model box — the one field whose value is long — lost its third to the picker beside it
       * and was drawn one character wide. Nothing said what the columns were, either; a
       * "(default)" box and a bare number read as noise without the heading.
       */}
      {entries.length > 0 && (
        <div className={styles.entries}>
          <span />
          <span className={styles.entryHead}>Provider</span>
          <span className={styles.entryHead}>Model</span>
          <span
            className={styles.entryHead}
            title="Reasoning effort, e.g. high or xhigh. Blank is the model's default."
          >
            Effort
          </span>
          <span
            className={styles.entryHead}
            title="How many runs may use this model at once, across every project. Blank is no limit."
          >
            At once
          </span>
          <span className={styles.entryHeadRest} />
          {entries.map((entry, index) => {
            const verdict = judgeEntry(entry, providers)
            return (
              <div
                key={index}
                className={isStruck(verdict) ? `${styles.entry} ${styles.entryDead}` : styles.entry}
              >
                <span className={styles.entryOrdinal}>{index + 1}</span>
                <span className={styles.entrySelect}>
                  <Select
                    size="sm"
                    aria-label="Entry provider"
                    value={entry.provider === '' ? null : entry.provider}
                    placeholder="(pick a provider)"
                    onChange={(provider) => {
                      // A model belongs to its provider: carried across, it would name something
                      // the new provider does not declare and dispatch to nothing.
                      const next = { ...entry, provider }
                      const keep = entryModelChoices(next, providers, offered).some(
                        (c) => c.listed && c.model === entry.model,
                      )
                      set(index, keep ? next : { ...next, model: '' })
                    }}
                    options={[
                      // A provider the entry names but that no longer exists still has to be
                      // displayable, or the row reads as empty over a value that is really there.
                      ...(providers.every((p) => p.id !== entry.provider) && entry.provider !== ''
                        ? [{ value: entry.provider, label: entry.provider }]
                        : []),
                      ...providers.map((p) => ({
                        value: p.id,
                        label: p.id,
                        detail: isEnabled(p) ? undefined : 'off',
                      })),
                    ]}
                  />
                </span>
                {/*
                 * A dropdown alone, over the models the provider declares (`entryModelChoices`).
                 * This was a free-text box with a chevron beside it fed only by the opencode probe:
                 * the chevron was usually empty, and the box took any string, so a typo became a
                 * pool entry that dispatched to nothing. A model the provider does not have yet is
                 * added on the provider's card first, which is also what opencode needs to reach it.
                 */}
                <span className={styles.entrySelect}>
                  <Select
                    size="sm"
                    aria-label="Entry model"
                    value={entry.model === '' ? null : entry.model}
                    disabled={entry.provider === ''}
                    placeholder={
                      entry.provider === ''
                        ? '(pick a provider first)'
                        : entryModelChoices(entry, providers, offered).length === 0
                          ? '(this provider declares no models)'
                          : '(pick a model)'
                    }
                    onChange={(model) => set(index, { ...entry, model })}
                    options={entryModelChoices(entry, providers, offered).map((c) => ({
                      value: c.model,
                      label: c.model,
                      detail: c.listed ? undefined : 'not declared',
                    }))}
                  />
                </span>
                <input
                  className={styles.entryField}
                  aria-label="Entry effort"
                  list={VARIANT_LIST}
                  placeholder="(default)"
                  value={entry.variant}
                  onChange={(e) => set(index, { ...entry, variant: e.target.value })}
                />
                {/*
                 * The entry's running limit. Blank is no limit. A run starts on the first entry
                 * with room, and waits in the queue when every one is full — so this is how a
                 * subscription that allows four sessions is kept from being asked for a fifth.
                 */}
                <input
                  className={`${styles.entryField} ${styles.entryLimit}`}
                  aria-label="Entry max running"
                  title="How many runs may use this model at once, across every project. Blank is no limit."
                  type="number"
                  min={1}
                  step={1}
                  placeholder="no limit"
                  value={entry.maxRunning ?? ''}
                  onChange={(e) => set(index, withMaxRunning(entry, e.target.value))}
                />
                {/* Buttons rather than drag: keyboard-reachable, and the move logic is a pure
                    function `check:pools` drives. `check:ui-icons` forbids a Unicode arrow. */}
                <IconButton
                  icon="arrow-up"
                  label="Move entry up"
                  disabled={index === 0}
                  onClick={() => onChange({ ...pool, entries: moveUp(entries, index) })}
                />
                <IconButton
                  icon="arrow-down"
                  label="Move entry down"
                  disabled={index === entries.length - 1}
                  onClick={() => onChange({ ...pool, entries: moveDown(entries, index) })}
                />
                <IconButton
                  icon="x"
                  label="Remove entry"
                  onClick={() => onChange({ ...pool, entries: removeAt(entries, index) })}
                />
              </div>
            )
          })}
        </div>
      )}

      <div className={styles.actions}>
        <ActionButton
          label="Add entry"
          onClick={() =>
            onChange({ ...pool, entries: [...entries, { provider: '', model: '', variant: '' }] })
          }
        />
        <ActionButton
          label={checking ? 'Checking…' : 'Check entries'}
          disabled={checking || entries.every((entry) => isStruck(judgeEntry(entry, providers)))}
          onClick={() => void checkAll()}
        />
      </div>
      {entries.length > 0 && (
        <p className={styles.modelHead}>
          Check entries sends one very small prompt to each entry, at its effort, which spends a
          little of each provider&rsquo;s quota.
        </p>
      )}

      {entries.length > 0 && (
        <div className={styles.readout}>
          {entries.map((entry, index) => {
            const verdict = judgeEntry(entry, providers)
            return (
              <span
                key={index}
                className={
                  isStruck(verdict)
                    ? `${styles.readoutLine} ${styles.readoutDead}`
                    : styles.readoutLine
                }
              >
                {index + 1}{'  '}
                {verdict === 'incomplete'
                  ? '(pick a provider and a model)'
                  : entryFlags(entry, variantInModel).join(' ')}
                {verdict === 'unknownProvider' && '   ← no such provider'}
                {verdict === 'disabledProvider' && '   ← provider switched off'}
                {verdict !== 'incomplete' &&
                  entry.maxRunning !== undefined &&
                  `   (up to ${entry.maxRunning} at once)`}
                <CheckVerdict check={checks[checkKey(entry)]} />
              </span>
            )
          })}
          {poolCapacity(entries) !== null && (
            <span className={styles.readoutLine}>
              holds {poolCapacity(entries)} run(s) at once; the next waits for a model to free
            </span>
          )}
        </div>
      )}
    </div>
  )
}

/** What a pool entry's check is keyed by: everything the checked argv was made of. */
function checkKey(entry: Entry): string {
  return `${entry.provider}\u0000${entry.model}\u0000${entry.variant.trim()}`
}

/** One entry's "Check entries" answer, on its readout line. */
function CheckVerdict({ check }: { check: LlmModelTest | 'running' | undefined }) {
  if (check === undefined) return null
  if (check === 'running') return <span className={styles.checkRunning}>   checking…</span>
  return (
    <span className={check.ok ? styles.checkOk : styles.checkBad}>
      {check.ok ? '   answers' : `   fails: ${check.detail}`}
    </span>
  )
}
