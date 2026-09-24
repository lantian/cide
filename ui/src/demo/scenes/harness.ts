import type { AgentDraft, AgentModels, AgentScope, Harness, ProjectOverrides } from '../../ipc/generated'
import type { ClaudeCliSupport, CodexCliSupport } from '../../ipc/client'
import type { Args } from '../fakeTauri'
import type { Scene } from '../scenes'
import { leaf, pane, tab } from '../world'
import { click, showPanel, sleep, until } from '../drive'
import { CLAUDE_CLI, CODEX_CLI, draftOf, LLM, modelsOf, OVERRIDES } from '../data/harness'
import { agentHandlers, openCoderRun, widenAgentsSidebar } from './agents'

/**
 * Roles on four harnesses, told twice: Settings → Agents lists each role file with its harness
 * badge (Claude, Codex, opencode, MiMo Code, and a Claude Code subagent), and the Agents panel's
 * History beside it shows what each run actually ran on — harness, model, and pool position.
 */
export const harness: Scene = {
  setup: (world, handlers) => {
    widenAgentsSidebar(world, 400)
    world.boot.workspace.settings.llm = LLM
    const sessions = openCoderRun(world)
    for (const [cmd, answer] of agentHandlers(world, sessions)) handlers.set(cmd, answer)
    handlers.set('agents_draft', (a: Args): AgentDraft => draftOf(String(a['agent']), a['scope'] as AgentScope))
    handlers.set('agents_models', (a: Args): AgentModels => modelsOf(a['harness'] as Harness))
    handlers.set('agent_overrides_get', (): ProjectOverrides => OVERRIDES)
    handlers.set('claude_cli_support', (): ClaudeCliSupport => CLAUDE_CLI)
    handlers.set('codex_cli_support', (): CodexCliSupport => CODEX_CLI)
    const settings = pane('editor', 'settings')
    world.open(tab({ kind: 'settings', section: 'agents' }, leaf(settings.id), [settings]))
  },
  drive: async () => {
    // History: ended runs, each naming the harness, the model and the pool it ran on — and the
    // coder's finished run offering Integrate.
    await showPanel('agents')
    await click('[data-audit="agentsTab"][data-tab="history"]')
    // The role rows carry no audit hooks, so find the list by its status cells; scrolled to the
    // top so the five roles and their harness badges fill the settings page.
    await until(() => document.querySelector('[data-audit="agentRowStatus"]') !== null)
    const list = document.querySelector('[data-audit="agentRowStatus"]')?.closest('ul')
    list?.scrollIntoView({ block: 'start' })
    // …then back a little, so the ROLES heading above the list is on screen too.
    let scroller = list?.parentElement ?? null
    while (scroller && scroller.scrollTop === 0) scroller = scroller.parentElement
    scroller?.scrollBy(0, -56)
    await sleep(300)
  },
}
