import type { Scene } from '../scenes'
import { showPanel, sleep } from '../drive'
import { useMilestones } from '../../sidebar/milestonesStore'
import { agentHandlers, openCoderRun, widenAgentsSidebar } from './agents'

/**
 * The Milestones tab of the Tasks panel: one milestone accepted, the active one's gate
 * re-running after a failure, a verify in flight, and two agent proposals in the inbox.
 *
 * The panel alone rather than a milestone's modal over it: the modal (overview form, task list
 * or gate log) hides the plan, and the plan with its gates and proposals is the picture.
 */
export const milestones: Scene = {
  setup: (world, handlers) => {
    widenAgentsSidebar(world)
    const sessions = openCoderRun(world)
    for (const [cmd, answer] of agentHandlers(world, sessions)) handlers.set(cmd, answer)
  },
  drive: async () => {
    await showPanel('tasks')
    // The tab a task card's milestone chip asks for, through the same store.
    useMilestones.getState().setTab('milestones')
    await sleep(400)
  },
}
