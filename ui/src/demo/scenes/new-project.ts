import type { Scene } from '../scenes'
import { click, sleep } from '../drive'
import { requestNewProject } from '../../chrome/newProject/newProjectStore'

/**
 * The New project wizard on its first step: the three ways a project starts — empty, driven by
 * an OpenSpec change, or driven by a brief that agents break into tasks.
 *
 * Raised through `requestNewProject`, the store `project.new` goes through, so the picture is
 * of the dialog the command opens and not one assembled by hand.
 */
export const newProject: Scene = {
  setup: () => {},
  drive: async () => {
    await sleep(200)
    requestNewProject()
    await sleep(400)
    // Task-driven picked, so the rail on the left grows to the longest path's five steps.
    await click('[role="radiogroup"][aria-label="Project type"] [role="radio"]:nth-child(3)')
  },
}
