/**
 * The demo project's subagents and their runs, as `agents_roster` answers them.
 *
 * Five roles on four harnesses, because "a role is a harness plus a model" is the point the
 * Agents panel and the harness scene both make: `coder` on Claude Code, `reviewer` on Codex,
 * `tester` on opencode through a model pool, `docs` on MiMo, and one subagent cide did not
 * author — read out of `.claude/agents/` — to show that those sit in the same list.
 *
 * The runs work the tasks in `data/tasks.ts`, so a run row's task link and the board agree.
 * Wire shapes (`AgentRoster`), not `AgentsPanel/fixture.ts`'s view props: that file restates the
 * model after `adapt.ts`, and the demo has to answer what comes before it.
 */
import type { AgentDef, AgentRoster, AgentRun, Harness, OrchestrationConfig, RunState, SessionId } from '../../ipc/generated'
import { wire } from '../world'
import { NOW } from './tasks'

const MIN = 60_000

function role(over: Partial<AgentDef> & Pick<AgentDef, 'id' | 'label' | 'harness' | 'description'>): AgentDef {
  return {
    scope: 'project',
    systemPrompt: '',
    model: null,
    unavailable: null,
    maxConcurrent: 1,
    worktree: true,
    ...over,
  }
}

export const ROLES: AgentDef[] = [
  role({
    id: 'coder',
    label: 'Coder',
    harness: 'claude',
    model: 'opus',
    color: 'orange',
    maxConcurrent: 2,
    description: 'Implements one task end to end in its own worktree, then hands it to review.',
    systemPrompt: 'You are the coder for cide. Work the task you are given in your worktree; keep comments that carry the why.',
  }),
  role({
    id: 'reviewer',
    label: 'Reviewer',
    harness: 'codex',
    model: 'gpt-5-codex',
    color: 'blue',
    description: 'Reviews a finished branch against its task and the house rules in CLAUDE.md.',
    systemPrompt: 'Review the branch for correctness first, then for the invariants CLAUDE.md names.',
  }),
  role({
    id: 'tester',
    label: 'Tester',
    harness: 'opencode',
    model: 'pool:fast',
    color: 'yellow',
    maxConcurrent: 2,
    description: 'Writes the failing test first, then runs the checks docs/checks.md lists for the surface.',
    systemPrompt: 'Write the test that proves the task, run the checks for the surface, report what failed.',
  }),
  role({
    id: 'docs',
    label: 'Docs',
    harness: 'mimo',
    model: 'mimo-v2-flash',
    color: 'purple',
    description: 'Keeps docs/ and the journal honest about what shipped and what was never confirmed.',
    systemPrompt: 'Update the docs a change touched. Never claim something works that was not run.',
    worktree: false,
  }),
  role({
    id: 'security-auditor',
    label: 'Security Auditor',
    scope: 'claudeProject',
    harness: 'claude',
    model: 'sonnet',
    color: 'cyan',
    description: 'Reads a change for credential handling, child environments and anything that spawns.',
    systemPrompt: 'Audit for secrets, child_env bypasses and unchecked spawns.',
  }),
]

let seq = 0
function run(
  agentId: string,
  state: RunState,
  over: Partial<AgentRun> & { startedAgo: number; harness: Harness },
): AgentRun {
  const { startedAgo, ...rest } = over
  const def = ROLES.find((r) => r.id === agentId)
  const working = state.state === 'running' || state.state === 'starting'
  const started = NOW - startedAgo
  return {
    run: `r-${(++seq).toString().padStart(4, '0')}`,
    agent: agentId,
    agentLabel: def?.label ?? agentId,
    project: '',
    session: null,
    state,
    task: null,
    startedUnixMs: wire(started),
    workedMs: wire(working ? Math.round(startedAgo * 0.35) : Math.round(startedAgo * 0.8)),
    workingSinceUnixMs: working ? wire(NOW - Math.round(startedAgo * 0.4)) : null,
    notify: { kind: 'primary' },
    staleTurn: false,
    note: null,
    openable: false,
    model: null,
    poolPosition: null,
    worktree: false,
    ...rest,
  }
}

/** The sessions a live run's pane can mirror; the scene hands in the ones it registered. */
export interface RunSessions {
  coder: SessionId
  coder2: SessionId
  tester: SessionId
  reviewer: SessionId
}

export function runs(project: string, s: RunSessions): AgentRun[] {
  seq = 0
  const all: AgentRun[] = [
    // Live.
    run('coder', { state: 'running' }, {
      harness: 'claude', task: 't-41', session: s.coder, openable: true, model: 'claude-opus-4-5',
      startedAgo: 38 * MIN, worktree: true,
      note: 'worktree .cide/worktrees/coder-t-41 · branch cide/coder-t-41',
    }),
    // The coder's second slot: turn handed back, child alive, waiting on t-41 to land first.
    run('coder', { state: 'idle' }, {
      harness: 'claude', task: 't-43', session: s.coder2, openable: true, model: 'claude-opus-4-5',
      startedAgo: 22 * MIN, worktree: true,
    }),
    run('tester', { state: 'running' }, {
      harness: 'opencode', task: 't-42', session: s.tester, openable: true,
      model: 'zai/glm-4.6', poolPosition: 'fast entry 1 of 3', startedAgo: 14 * MIN, worktree: true,
    }),
    run('reviewer', { state: 'awaitingPermission' }, {
      harness: 'codex', task: 't-38', session: s.reviewer, openable: true, model: 'gpt-5-codex',
      startedAgo: 9 * MIN, worktree: true,
    }),
    run('docs', { state: 'queued' }, { harness: 'mimo', task: 't-44', startedAgo: 2 * MIN, model: 'mimo-v2-flash' }),

    // History: ended runs, newest first in the panel. Only roles whose declared colour is also
    // the hue their id derives to: History rows colour by `agentColor(run.agent)` without the
    // declared `color:`, so a reviewer (declared blue, derives orange) would change colour
    // between the two tabs.
    run('coder', { state: 'finished', code: 0 }, {
      harness: 'claude', task: 't-37', model: 'claude-opus-4-5', startedAgo: 70 * MIN, worktree: true,
      note: 'branch cide/coder-t-37 · 3 commits ahead of master',
    }),
    run('tester', { state: 'failed', reason: 'check:render-stall failed: 2 of 14 scenarios stalled past 250 ms' }, {
      harness: 'opencode', task: 't-45', model: 'openrouter/qwen3-coder', poolPosition: 'fast entry 2 of 3',
      startedAgo: 95 * MIN, worktree: true,
    }),
    run('coder', { state: 'finished', code: 0 }, {
      harness: 'claude', task: 't-36', model: 'claude-opus-4-5', startedAgo: 3 * 60 * MIN, worktree: false,
    }),
    run('tester', { state: 'finished', code: 0 }, {
      harness: 'opencode', task: 't-32', model: 'deepseek/deepseek-chat', poolPosition: 'fast entry 3 of 3',
      startedAgo: 5 * 60 * MIN, worktree: false,
    }),
    run('security-auditor', { state: 'finished', code: 0 }, {
      harness: 'claude', task: 't-34', model: 'claude-sonnet-4-5', startedAgo: 7 * 60 * MIN, worktree: false,
    }),
  ]
  return all.map((r) => ({ ...r, project }))
}

export function roster(project: string, s: RunSessions): AgentRoster {
  return { kind: 'ready', agents: ROLES, runs: runs(project, s), dispatching: true }
}

export const CONFIG: OrchestrationConfig = {
  enabled: true,
  maxConcurrent: 4,
  harness: 'claude',
  finishInNewTab: false,
  autoSpin: true,
  autoSpinAfterSecs: 90,
  autoSpinPrompt: 'Check the board: pick up anything assigned to you that is unblocked.',
  reviewPrompt: 'Review {task} against its body and CLAUDE.md, then move it to done or back to doing.',
  isolateEnv: ['CARGO_TARGET_DIR', 'VITE_PORT'],
  isolateEnvShare: ['RUSTC_WRAPPER'],
  verifyExclusive: true,
}
