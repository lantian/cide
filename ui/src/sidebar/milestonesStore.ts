/**
 * The active project's milestones, for the two surfaces that draw them. (M83)
 *
 * The Tasks panel's Milestones tab draws the whole view, and the task card draws which milestone
 * one task serves; both read here so they cannot disagree. It follows `useTasks`' project and is
 * re-asked on `cide://milestones-changed` **and whenever the board changes** — the view carries
 * each milestone's tasks, and a task linked to a milestone is a board change, not a milestones
 * one. Coalesced, because an agent's burst of edits is many boards in a second.
 *
 * A mirror, like every store here (ADR 0002): it never computes the tree itself — Rust's
 * `milestones::view` is the one producer, through `cide_agents::milestones::tree_under`.
 */
import { create } from 'zustand'

import { milestones as milestonesApi, type MilestonesView, type ProjectId } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { requestPanel } from '@/chrome/panelRequests'
import { useTasks } from '@/sidebar/tasksStore'
import type { TaskMilestone } from './TasksPanel/TaskDetail'

/** Which tab the Tasks panel shows. Here rather than in the panel, so a task card can ask. */
export type TasksTab = 'tasks' | 'milestones'

interface MilestonesStore {
  project: ProjectId | null
  view: MilestonesView | null
  error: string | null
  /** The Tasks panel's tab, for the life of the window. */
  tab: TasksTab
  /** A milestone somebody asked to see, and on which tab of its modal — opened once, cleared. */
  focus: { milestone: string; tab: 'overview' | 'log' } | null
  setTab: (tab: TasksTab) => void
  /** Show `milestone` in the Milestones tab: switch to it and open that milestone's modal. */
  reveal: (milestone: string, tab?: 'overview' | 'log') => void
  /** The tab took the request. */
  clearFocus: () => void
  /** Ask again now. The tab's retry, and what every trigger below schedules. */
  refresh: () => void
  /** Adopt an answer a write already brought back, skipping a round trip. */
  adopt: (view: MilestonesView | null) => void
}

export const useMilestones = create<MilestonesStore>((set, get) => ({
  project: null,
  view: null,
  error: null,
  tab: 'tasks',
  focus: null,
  setTab: (tab) => set({ tab }),
  reveal: (milestone, tab = 'overview') => {
    set({ tab: 'milestones', focus: { milestone, tab } })
    // The panel may be showing Files, or be shut: the tab is only seen if the Tasks panel is.
    requestPanel('tasks')
  },
  clearFocus: () => set({ focus: null }),
  refresh: () => {
    const project = get().project
    if (project === null) return
    void milestonesApi
      .get(project)
      .then((view) => {
        // A late answer for a project the store has since left is dropped.
        if (get().project === project) set({ view, error: null })
      })
      .catch((e: unknown) => {
        if (get().project === project) set({ error: errorText(e) })
      })
  },
  adopt: (view) => set({ view, error: null }),
}))

let timer: ReturnType<typeof setTimeout> | undefined
function soon(): void {
  if (timer !== undefined) clearTimeout(timer)
  timer = setTimeout(() => {
    timer = undefined
    useMilestones.getState().refresh()
  }, 250)
}

let wired = false
/**
 * Start following. Idempotent, and called by whichever surface mounts first: a store that
 * subscribed at import time would do so in every realm that merely imports a type from here.
 */
export function followMilestones(): void {
  if (wired) return
  wired = true
  let lastProject: ProjectId | null = null
  let lastBoard: unknown = null
  const sync = () => {
    const { project, board } = useTasks.getState()
    if (project !== lastProject) {
      lastProject = project
      lastBoard = board
      useMilestones.setState({ project, view: null, error: null })
      useMilestones.getState().refresh()
      return
    }
    if (board !== lastBoard) {
      lastBoard = board
      soon()
    }
  }
  useTasks.subscribe(sync)
  sync()
  void milestonesApi.onChanged((project) => {
    if (project === useMilestones.getState().project) soon()
  })
}

/** Which milestone a task serves, from the view: its own goal, or a task under one. */
export function milestoneOfTask(view: MilestonesView | null, task: string): TaskMilestone | null {
  if (view === null) return null
  const items = view.plan.items
  const named = view.plan.active === undefined ? undefined : items.find((m) => m.id === view.plan.active)
  const current = named ?? items[0]
  const stateOf = (id: string): TaskMilestone['state'] =>
    view.accepted.includes(id) ? 'accepted' : current?.id === id ? 'active' : 'later'
  for (const m of items) {
    if (m.task === task) return { id: m.id, title: m.title, state: stateOf(m.id), goal: true }
    const list = view.tasks.find((t) => t.milestone === m.id)
    if (list?.tasks.some((t) => t.id === task) === true) {
      return { id: m.id, title: m.title, state: stateOf(m.id), goal: false }
    }
  }
  return null
}

/** What a task's verify looks like, for the row's mark and the card's line. (M83) */
export type VerifyMark = 'running' | 'passed' | 'failed'

/** Task id → its verify mark, from the view. A new object per view, so memoise on the view. */
export function verifyMarks(view: MilestonesView | null): Record<string, VerifyMark> {
  const marks: Record<string, VerifyMark> = {}
  for (const v of view?.verifies ?? []) {
    marks[String(v.task)] = v.running ? 'running' : v.last?.passed === true ? 'passed' : 'failed'
  }
  return marks
}

/** The milestones whose gate is running now. */
export function runningGates(view: MilestonesView | null): string[] {
  return (view?.gates ?? []).filter((g) => g.running).map((g) => g.milestone)
}
