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
import { useCallback, useMemo, useState } from 'react'

import { Icon } from '@/icons'
import type { AgentModels, LlmModelTest, LlmSettings, SettingsPatch } from '@/ipc/generated'

import { ActionButton, Group, Note, Row, TextField, ToggleRow } from './controls'
import controls from './controls.module.css'
import {
  CODEX_PLUGIN_SETUP,
  DEFAULT_CUSTOM_NPM,
  KNOWN_CATALOG_IDS,
  VARIANT_SUGGESTIONS,
  blankProvider,
  entryFlags,
  isEnabled,
  isStruck,
  joinModelId,
  judgeEntry,
  localProblems,
  modelsFor,
  moveDown,
  moveUp,
  removeAt,
  splitModelId,
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
  recheckModels: () => void
  /**
   * Try one `provider/model` for real. Spends a very small amount of quota, so it is only ever
   * wired to a button — never to a keystroke or a mount.
   */
  testModel: (model: string) => Promise<LlmModelTest | null>
}

export function ModelsSection({
  settings,
  patch,
  models,
  recheckModels,
  testModel,
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

  /** The provider ids the probe actually saw, so a row can say whether it works. */
  const seen = useMemo(() => {
    const byProvider = new Map<string, string[]>()
    for (const line of models?.models ?? []) {
      const split = splitModelId(line)
      if (split === null) continue
      const list = byProvider.get(split.provider) ?? []
      list.push(split.model)
      byProvider.set(split.provider, list)
    }
    return byProvider
  }, [models])

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
            offered={seen.get(provider.id) ?? null}
            probed={models !== null}
            open={openProviders.has(index)}
            onToggle={() => toggleProvider(index)}
            onChange={(next) => setProvider(index, next)}
            onRemove={() => write({ ...settings, providers: removeAt(providers, index) })}
            testModel={testModel}
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

      <Group title="What opencode sees">
        <Row
          label="Check the providers"
          hint={
            'Runs `opencode models` with the configuration cide would give a run, so this list is ' +
            'exactly what a pool entry can reach. A provider missing from it is one opencode ' +
            'could not load — a wrong key, an endpoint that is not running, or a plugin that is ' +
            'not installed.'
          }
          control={<ActionButton label="Re-check" onClick={recheckModels} />}
        />
        {models?.problem != null && (
          <Note title="That check did not answer" tone="warn">
            {models.problem}
          </Note>
        )}
        {models !== null && models.problem == null && (
          <div className={styles.readout}>
            {models.models.length === 0 ? (
              <span className={styles.readoutLine}>
                opencode listed no models at all on this machine.
              </span>
            ) : (
              models.models.map((line) => (
                <span key={line} className={styles.readoutLine}>
                  {line}
                </span>
              ))
            )}
          </div>
        )}
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
  offered,
  probed,
  open,
  onToggle,
  onChange,
  onRemove,
  testModel,
}: {
  provider: Provider
  offered: readonly string[] | null
  probed: boolean
  open: boolean
  onToggle: () => void
  onChange: (next: Provider) => void
  onRemove: () => void
  testModel: (model: string) => Promise<LlmModelTest | null>
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
        {/* Drawn collapsed as well as open: whether a provider works is the one thing worth
            seeing without unfolding anything. */}
        {probed && provider.id !== '' && (
          <span className={offered === null ? styles.verdictBad : styles.verdictOk}>
            {offered === null ? (
              <Icon name="circle-alert" size={1} label="not answering" />
            ) : (
              `${offered.length}`
            )}
          </span>
        )}
        <span className={styles.kind}>{provider.kind}</span>
        <button
          type="button"
          className={styles.iconButton}
          aria-label="Remove provider"
          onClick={onRemove}
        >
          <Icon name="trash-2" size={1} />
        </button>
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

      {providerBody(provider, onChange)}

      {problems.map((problem) => (
        <p key={problem} className={styles.problem}>
          {problem}
        </p>
      ))}
      {probed && provider.id !== '' && (
        <p className={offered === null ? styles.verdictBad : styles.verdictOk}>
          {offered === null
            ? verdictMiss(provider)
            : `opencode offers ${offered.length} model${offered.length === 1 ? '' : 's'} under this id.`}
        </p>
      )}
      {/*
       * A per-model test, for every kind — including the ones cide does not configure. Listing an
       * id proves opencode *resolved* it; it does not prove a key is right, that an endpoint will
       * answer a completion, that a model has not been retired, or that a plugin's OAuth is still
       * good. Each of those lists perfectly and fails on the first token.
       */}
      {(offered ?? []).length > 0 && (
        <TestableModels
          provider={provider.id}
          models={offered ?? []}
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
  testModel: (model: string) => Promise<LlmModelTest | null>
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
function providerBody(provider: Provider, onChange: (next: Provider) => void) {
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
            models={provider.models as Model[]}
            onChange={(models) => onChange({ ...provider, models })}
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
function verdictMiss(provider: Provider): string {
  switch (provider.kind) {
    case 'catalog':
      return 'opencode listed no models under this id. Either the key is wrong, or it is not in your environment and the box above is empty.'
    case 'custom':
      return 'opencode listed no models under this id. Check the base URL is reachable and the npm package is right.'
    case 'external':
      return 'Not visible yet. Follow the steps above, then re-check.'
    default: {
      const unreachable: never = provider
      return unreachable
    }
  }
}

/** The model list a custom endpoint must declare. */
function ModelRows({
  models,
  onChange,
}: {
  models: Model[]
  onChange: (next: Model[]) => void
}) {
  const set = (index: number, next: Model) =>
    onChange(models.map((m, i) => (i === index ? next : m)))
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
        <div key={index} className={styles.modelRow}>
          <input
            className={styles.entryField}
            aria-label="Model id"
            placeholder="qwen3:8b"
            value={model.id}
            onChange={(e) => set(index, { ...model, id: e.target.value })}
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
          <button
            type="button"
            className={styles.iconButton}
            aria-label="Remove model"
            onClick={() => onChange(removeAt(models, index))}
          >
            <Icon name="x" size={1} />
          </button>
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
  onChange,
  onRemove,
}: {
  pool: Pool
  providers: readonly Provider[]
  /** Every `provider/model` the probe reported, so an entry can be picked rather than typed. */
  offered: readonly string[]
  onChange: (next: Pool) => void
  onRemove: () => void
}) {
  const entries = pool.entries as Entry[]
  const set = (index: number, next: Entry) =>
    onChange({ ...pool, entries: entries.map((e, i) => (i === index ? next : e)) })
  return (
    <div className={styles.card}>
      <div className={styles.cardHead}>
        <span className={styles.cardTitle}>{pool.name === '' ? 'New pool' : pool.name}</span>
        <button type="button" className={styles.iconButton} aria-label="Remove pool" onClick={onRemove}>
          <Icon name="trash-2" size={1} />
        </button>
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

      {entries.map((entry, index) => {
        const verdict = judgeEntry(entry, providers)
        return (
          <div
            key={index}
            className={isStruck(verdict) ? `${styles.entry} ${styles.entryDead}` : styles.entry}
          >
            <span className={styles.entryOrdinal}>{index + 1}</span>
            <select
              className={styles.entryField}
              aria-label="Entry provider"
              value={entry.provider}
              onChange={(e) => set(index, { ...entry, provider: e.target.value })}
            >
              <option value="">(pick a provider)</option>
              {/* A provider the entry names but that no longer exists still has to be
                  displayable, or the row reads as empty over a value that is really there. */}
              {providers.every((p) => p.id !== entry.provider) && entry.provider !== '' && (
                <option value={entry.provider}>{entry.provider}</option>
              )}
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.id}
                  {isEnabled(p) ? '' : ' (off)'}
                </option>
              ))}
            </select>
            {/*
             * A picker *and* a box, the shape `AgentsSection`'s ModelField already uses: the menu
             * is what this machine actually reports, and the box stays typable because a model
             * opencode has not listed — one behind a plugin whose OAuth has lapsed, one added
             * since the last probe — is still a model a run can be pointed at.
             */}
            <div className={styles.combo}>
              <input
                className={styles.entryField}
                aria-label="Entry model"
                placeholder="deepseek/deepseek-chat"
                value={entry.model}
                onChange={(e) => set(index, { ...entry, model: e.target.value })}
              />
              <select
                className={styles.comboPick}
                aria-label="Pick a model"
                value=""
                onChange={(e) => {
                  if (e.target.value !== '') set(index, { ...entry, model: e.target.value })
                }}
              >
                <option value="">Pick…</option>
                {modelsFor(entry.provider, offered).map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))}
              </select>
            </div>
            <input
              className={styles.entryField}
              aria-label="Entry variant"
              list={VARIANT_LIST}
              placeholder="(default)"
              value={entry.variant}
              onChange={(e) => set(index, { ...entry, variant: e.target.value })}
            />
            {/* Buttons rather than drag: keyboard-reachable, and the move logic is a pure
                function `check:pools` drives. `check:ui-icons` forbids a Unicode arrow. */}
            <button
              type="button"
              className={styles.iconButton}
              aria-label="Move entry up"
              disabled={index === 0}
              onClick={() => onChange({ ...pool, entries: moveUp(entries, index) })}
            >
              <Icon name="arrow-up" size={1} />
            </button>
            <button
              type="button"
              className={styles.iconButton}
              aria-label="Move entry down"
              disabled={index === entries.length - 1}
              onClick={() => onChange({ ...pool, entries: moveDown(entries, index) })}
            >
              <Icon name="arrow-down" size={1} />
            </button>
            <button
              type="button"
              className={styles.iconButton}
              aria-label="Remove entry"
              onClick={() => onChange({ ...pool, entries: removeAt(entries, index) })}
            >
              <Icon name="x" size={1} />
            </button>
          </div>
        )
      })}

      <div className={styles.actions}>
        <ActionButton
          label="Add entry"
          onClick={() =>
            onChange({ ...pool, entries: [...entries, { provider: '', model: '', variant: '' }] })
          }
        />
      </div>

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
                  : entryFlags(entry).join(' ')}
                {verdict === 'unknownProvider' && '   ← no such provider'}
                {verdict === 'disabledProvider' && '   ← provider switched off'}
              </span>
            )
          })}
        </div>
      )}
    </div>
  )
}
