/**
 * Settings ▸ Models for the demo: the providers a working setup has — two catalogued ones, a
 * local Ollama declared as a custom endpoint, the codex subscription — and pools that fall from
 * a paid model to a cheaper one and finally to the machine under the desk.
 *
 * The pool entries only name providers and models that the probe answer below also lists, so the
 * screen's own "does this entry resolve" verdicts come out green rather than a wall of red that
 * would read as a broken install.
 */
import type { AgentModels, LlmSettings } from '../../ipc/generated'

export const LLM: LlmSettings = {
  providers: [
    { kind: 'catalog', id: 'anthropic', label: 'Anthropic', enabled: true, apiKey: '' },
    { kind: 'catalog', id: 'openrouter', label: 'OpenRouter', enabled: true, apiKey: '' },
    {
      kind: 'custom',
      id: 'ollama',
      label: 'Ollama (workstation)',
      enabled: true,
      npm: '@ai-sdk/openai-compatible',
      baseUrl: 'http://localhost:11434/v1',
      apiKey: '',
      models: [
        { id: 'qwen3-coder:30b', label: 'Qwen3 Coder 30B', context: 262144, output: 32768 },
        { id: 'devstral:24b', label: 'Devstral 24B', context: 131072, output: 16384 },
        { id: 'gpt-oss:20b', label: 'gpt-oss 20B', context: 131072, output: 32768 },
      ],
    },
    {
      kind: 'external',
      id: 'openai',
      label: 'ChatGPT subscription (codex)',
      setup:
        'Add "opencode-openai-codex-auth" to the `plugin` array in your own opencode.json, then run '
        + '`opencode auth login`. cide does not write either.',
      expect: ['openai/gpt-5.2', 'openai/gpt-5.2-codex', 'openai/gpt-5.1-codex-max'],
    },
  ],
  pools: [
    {
      name: 'implement',
      description: 'Feature work: Sonnet first, codex when it is rate-limited, the local coder last.',
      entries: [
        { provider: 'anthropic', model: 'claude-sonnet-4-5', variant: '', maxRunning: 3 },
        { provider: 'openai', model: 'gpt-5.2-codex', variant: 'high', maxRunning: 2 },
        { provider: 'openrouter', model: 'moonshotai/kimi-k2', variant: '', maxRunning: 4 },
        { provider: 'ollama', model: 'qwen3-coder:30b', variant: '', maxRunning: 1 },
      ],
    },
    {
      name: 'review',
      description: 'Reviewers read more than they write; a strong model, never the local one.',
      entries: [
        { provider: 'anthropic', model: 'claude-opus-4-1', variant: '', maxRunning: 1 },
        { provider: 'openai', model: 'gpt-5.2', variant: 'xhigh', maxRunning: 1 },
      ],
    },
    {
      name: 'triage',
      description: 'Cheap and many: labelling the inbox, first reads of a failing check.',
      entries: [
        { provider: 'ollama', model: 'gpt-oss:20b', variant: '', maxRunning: 2 },
        { provider: 'openrouter', model: 'deepseek/deepseek-chat-v3.1', variant: '', maxRunning: 6 },
      ],
    },
  ],
}

/** What `opencode models` answers with cide's document injected — every pool entry resolves. */
export const OPENCODE_MODELS: AgentModels = {
  harness: 'opencode',
  models: [
    'anthropic/claude-opus-4-1',
    'anthropic/claude-sonnet-4-5',
    'anthropic/claude-haiku-4-5',
    'ollama/devstral:24b',
    'ollama/gpt-oss:20b',
    'ollama/qwen3-coder:30b',
    'openai/gpt-5.1-codex-max',
    'openai/gpt-5.2',
    'openai/gpt-5.2-codex',
    'openrouter/deepseek/deepseek-chat-v3.1',
    'openrouter/moonshotai/kimi-k2',
    'openrouter/qwen/qwen3-coder',
  ],
  problem: null,
}
