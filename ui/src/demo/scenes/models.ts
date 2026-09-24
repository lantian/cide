import type { AgentModels, LlmLimitsProbe, LlmModelTest } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import type { Scene } from '../scenes'
import { leaf, pane, tab } from '../world'
import { until } from '../drive'
import { LLM, OPENCODE_MODELS } from '../data/models'

/** Settings ▸ Models: providers, and pools that fail over from a paid model to a local one. */
export const models: Scene = {
  setup: (world, handlers) => {
    world.boot.workspace.settings.llm = structuredClone(LLM)
    // A settings *tab*, not the frame: the frame only draws with no project open, and the
    // picture wants the project's chrome around it. An editor pane is what Rust gives one.
    const host = pane('editor', 'settings')
    world.open(tab({ kind: 'settings', section: 'models' }, leaf(host.id), [host]))
    handlers.set('agents_models', (a: Args): AgentModels =>
      a['harness'] === 'opencode' ? OPENCODE_MODELS : { harness: 'claude', models: ['opus', 'sonnet', 'haiku'], problem: null })
    handlers.set('llm_probe_limits', (a: Args): LlmLimitsProbe =>
      ({ provider: String(a['provider']), model: String(a['model']), context: 262144, output: 32768, outputEstimated: false, detail: 'read from /v1/models' }))
    handlers.set('llm_test_model', (a: Args): LlmModelTest =>
      ({ model: String(a['model']), ok: true, detail: 'answered in 1.4s' }))
  },
  drive: async () => {
    // The pools are the story — a run falling from Sonnet to codex to the local coder — and they
    // start below the fold, so bring the last provider card to the top and let the pools follow.
    await until(() => [...document.querySelectorAll('button[aria-expanded]')].some((b) => b.textContent?.includes('Ollama')))
    const card = [...document.querySelectorAll('button[aria-expanded]')].find((b) => b.textContent?.includes('ChatGPT'))
    card?.parentElement?.scrollIntoView({ block: 'start' })
  },
}
