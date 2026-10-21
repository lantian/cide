import type { MilestoneTask } from '../../ipc/generated'

/** Inbox rows are legacy links, not milestone work. */
export function isOpenMilestoneTask(task: MilestoneTask): boolean {
  return task.status !== 'done' && task.status !== 'inbox'
}

/** Keep open work and its context in the flattened, depth-first milestone tree. */
export function openTasksAndParents(tasks: readonly MilestoneTask[]): MilestoneTask[] {
  const visible = new Set<number>()
  const ancestors: number[] = []

  tasks.forEach((task, index) => {
    while (ancestors.length > task.depth) ancestors.pop()
    if (isOpenMilestoneTask(task)) {
      visible.add(index)
      ancestors.forEach((ancestor) => visible.add(ancestor))
    }
    ancestors.push(index)
  })

  return tasks.filter((_, index) => visible.has(index))
}
