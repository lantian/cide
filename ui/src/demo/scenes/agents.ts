import type { AgentRoster, MilestonesView, OrchestrationConfig, RunOpen, TaskBoard, TaskDetail } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import type { Scene } from '../scenes'
import { showPanel } from '../drive'
import { leaf, pane, tab, type World } from '../world'
import { CONFIG, roster, type RunSessions } from '../data/agents'
import { BOARD, detailOf } from '../data/tasks'
import { M2_LOG, milestonesView } from '../data/milestones'

/**
 * Open the coder's run in its own tab — what the run row's Open does — and register panes for
 * the other live runs so their sessions exist to be mirrored. Shared by every scene that shows
 * the roster, so the run a row names is always the transcript on screen.
 */
export function openCoderRun(world: World): RunSessions {
  const coder = pane('claude', 'coder · t-41', { kind: 'claude', conversation: 'feature' }, true)
  const coder2 = pane('claude', 'coder · t-43', { kind: 'claude', conversation: 'refactor' })
  const tester = pane('claude', 'tester · t-42', { kind: 'claude', conversation: 'tests' })
  const reviewer = pane('claude', 'reviewer · t-38', { kind: 'claude', conversation: 'review' })
  world.open(tab({ kind: 'claudeFull', title: 'coder · t-41', ephemeral: false }, leaf(coder.id), [coder]))
  return { coder: coder.session ?? '', coder2: coder2.session ?? '', tester: tester.session ?? '', reviewer: reviewer.session ?? '' }
}

/**
 * The Agents and Tasks panels share one width token, and its 320 px default truncates every
 * run's task title to a word. A user who lives in these panels drags it wider; so does the demo.
 */
export function widenAgentsSidebar(world: World, px = 460): void {
  const s = world.boot.workspace.settings.sidebar
  if (s) world.boot.workspace.settings.sidebar = { ...s, agentsWidth: px }
}

function sessionNames(world: World): Record<string, string> {
  const names = ['pty-backpressure', 'coalescer tests', 'review']
  const panes = Object.values(world.project.tabs[0]?.tree.panes ?? {}).filter((p) => p.kind === 'claude')
  return Object.fromEntries(panes.flatMap((p, i) => (p.session && names[i] ? [[p.session, names[i]]] : [])))
}

/** The answers every roster-showing scene needs: roles, runs, the board their tasks are on. */
export function agentHandlers(world: World, sessions: RunSessions): Array<[string, (a: Args) => unknown]> {
  const project = world.project.id
  return [
    ['agents_roster', (): AgentRoster => roster(project, sessions)],
    ['agents_config_get', (): OrchestrationConfig => CONFIG],
    ['tasks_board', (): TaskBoard => BOARD],
    ['task_get', (a: Args): TaskDetail | null => detailOf(String(a['task']))],
    // What `/rename` called each console conversation. Answered because the empty value of a
    // `Record` is `null` in `defaults.ts`, and the task card indexes into it.
    ['claude_session_names', (): Record<string, string> => sessionNames(world)],
    ['milestones_get', (): MilestonesView => milestonesView(project)],
    ['milestones_check_log', (): string | null => M2_LOG],
    ['agents_run_open', (): RunOpen => ({ kind: 'mirror', session: sessions.coder, continues: null })],
  ]
}

/** The Agents panel over the coder's live run: five roles, four harnesses, runs in flight. */
export const agents: Scene = {
  setup: (world, handlers) => {
    widenAgentsSidebar(world)
    const sessions = openCoderRun(world)
    for (const [cmd, answer] of agentHandlers(world, sessions)) handlers.set(cmd, answer)
  },
  drive: async () => {
    await showPanel('agents')
  },
}
