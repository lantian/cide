import type { Scene } from '../scenes'
import { revealTask } from '../../chrome/taskReveal'
import { showPanel, sleep, until } from '../drive'
import { useTasks } from '../../sidebar/tasksStore'
import { agentHandlers, openCoderRun, widenAgentsSidebar } from './agents'

let project = ''

/**
 * The task board, with t-41's card open over the coder's run: the body, the thread between the
 * orchestrator, the coder and the user, and the typed links that tie it to its epic.
 *
 * The roster is answered too, not only the board: assignee chips take their colour from the
 * roles, and a board without a roster draws every assignee in the derived fallback hue.
 */
export const tasks: Scene = {
  setup: (world, handlers) => {
    widenAgentsSidebar(world)
    project = world.project.id
    const sessions = openCoderRun(world)
    for (const [cmd, answer] of agentHandlers(world, sessions)) handlers.set(cmd, answer)
  },
  drive: async () => {
    await showPanel('tasks')
    // The card opens through `revealTask`, the door a `t-41` link in a terminal goes through.
    await until(() => useTasks.getState().board.kind === 'ready')
    revealTask('t-41', project)
    await sleep(500)
    // The card is taller than the window. Scroll it the way a reader would, so the typed links
    // and the thread share the screen with the body rather than sitting below the fold.
    document.querySelector('[data-audit="taskLinks"]')?.scrollIntoView({ block: 'center' })
    await sleep(200)
  },
}
