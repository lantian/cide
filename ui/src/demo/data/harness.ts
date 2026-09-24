/**
 * What Settings → Agents reads beyond the roster: each role's file as a draft, the models each
 * harness offers, the per-project overrides, and the two CLI probes.
 *
 * The roles themselves are `data/agents.ts`'s, so the settings screen and the Agents panel list
 * the same five on the same harnesses.
 */
import type { AgentDraft, AgentModels, AgentScope, Harness, LlmSettings, ProjectOverrides } from '../../ipc/generated'
import type { ClaudeCliSupport, CodexCliSupport } from '../../ipc/client'
import { ROLES } from './agents'

/** The frontmatter a role file carries beyond what the roster shows. */
const EXTRA: Record<string, Pick<AgentDraft, 'tools' | 'effort' | 'permissionMode'>> = {
  coder: { tools: ['Read', 'Edit', 'Write', 'Bash', 'Grep', 'Glob', 'mcp__cide'], effort: 'high', permissionMode: 'acceptEdits' },
  reviewer: { tools: ['Read', 'Grep', 'Glob', 'Bash(git diff:*)'], effort: 'high' },
  tester: { tools: ['Read', 'Edit', 'Bash', 'Grep'] },
  docs: { tools: ['Read', 'Edit', 'Grep', 'Glob'] },
  'security-auditor': { tools: ['Read', 'Grep', 'Glob'], effort: 'medium' },
}

const PROMPTS: Record<string, string> = {
  coder: `You are the coder for cide, an IDE built around a live Claude Code session.

Work the one task you were dispatched against, in your own worktree. Read CLAUDE.md and the
docs/checks.md row for every surface you touch, and run those checks before you hand back.

- Comments carry the why, at length, including the option that lost.
- Rust owns durable state; the UI mirrors it. Never edit the tree from the webview.
- Only cide-app may depend on tauri.

When you are done, comment on the task with what changed and what you ran, and move it to review.`,
  reviewer: `Review the branch named in the task against its body and against CLAUDE.md's invariants.
Correctness first; then the silent-failure rules in docs/checks.md. Say what you would change.`,
  tester: `Write the failing test first. Then run every check docs/checks.md lists for the surface,
and report the ones that failed with their output.`,
  docs: `Update the docs a change touched. The journal is the honest record: never claim something
works that was not run on a display.`,
  'security-auditor': `Audit a change for credential handling, child environments that bypass
cide-core::child_env, and anything that spawns a process.`,
}

export function draftOf(agent: string, scope: AgentScope): AgentDraft {
  const def = ROLES.find((r) => r.id === agent)
  if (!def || def.scope !== scope) throw new Error(`no ${scope} role named ${agent}`)
  const draft: AgentDraft = {
    scope,
    name: def.id,
    original: { scope, name: def.id },
    label: def.label,
    harness: def.harness,
    description: def.description,
    tools: [],
    maxConcurrent: def.maxConcurrent,
    worktree: def.worktree,
    systemPrompt: PROMPTS[def.id] ?? def.systemPrompt,
    extras: def.color ? [{ key: 'color', value: def.color }] : [],
    ...EXTRA[def.id],
  }
  if (def.model) draft.model = def.model
  return draft
}

const MODELS: Record<Harness, string[]> = {
  claude: ['opus', 'sonnet', 'haiku', 'claude-opus-4-5', 'claude-sonnet-4-5'],
  codex: ['gpt-5-codex', 'gpt-5', 'gpt-5-mini'],
  opencode: ['zai/glm-4.6', 'openrouter/qwen3-coder', 'anthropic/claude-sonnet-4-5', 'deepseek/deepseek-chat'],
  mimo: ['mimo-v2-flash', 'mimo-v2-pro'],
  qwen: ['qwen3-coder-plus', 'qwen3-coder-flash'],
}

export const modelsOf = (harness: Harness): AgentModels => ({ harness, models: MODELS[harness], problem: null })

export const OVERRIDES: ProjectOverrides = {
  all: {},
  roles: {
    // A local, uncommitted override: this machine runs the tester on the cheap pool.
    tester: { pool: 'fast' },
  },
}

export const CLAUDE_CLI: ClaudeCliSupport = {
  version: '2.1.247',
  verifiedRange: '>=2.1.200 <2.2',
  warning: null,
  handshake: { version: '2.1.247', atUnixMs: Date.now() - 40 * 60_000, verifiedRange: '>=2.1.200 <2.2' },
  binary: 'claude',
  resolved: '/home/dev/.local/bin/claude',
  problem: null,
  argReasons: [],
  envReasons: [],
  injectReasons: [],
}

export const CODEX_CLI: CodexCliSupport = {
  binary: 'codex',
  resolved: '/home/dev/.local/bin/codex',
  problem: null,
  version: '0.64.0',
  argNotes: [],
  envNotes: [],
  argv: [
    { text: 'codex', ours: true },
    { text: 'exec', ours: true },
    { text: '--json', ours: true },
  ],
}

/**
 * The pool the tester's local override names, so the override reads as a working redirect
 * rather than a pointer to nothing. Three providers deep: a rate limit on the first falls
 * through to the next, which is what the run rows' "fast entry 2 of 3" is about.
 */
export const LLM: LlmSettings = {
  providers: [
    { kind: 'catalog', id: 'zai', label: 'Z.ai', enabled: true, apiKey: '' },
    { kind: 'catalog', id: 'openrouter', label: 'OpenRouter', enabled: true, apiKey: '' },
    { kind: 'catalog', id: 'deepseek', label: 'DeepSeek', enabled: true, apiKey: '' },
  ],
  pools: [
    {
      name: 'fast',
      description: 'Cheap and quick, for tests and chores.',
      entries: [
        { provider: 'zai', model: 'glm-4.6', variant: '', maxRunning: 2 },
        { provider: 'openrouter', model: 'qwen3-coder', variant: '' },
        { provider: 'deepseek', model: 'deepseek-chat', variant: '' },
      ],
    },
  ],
}
