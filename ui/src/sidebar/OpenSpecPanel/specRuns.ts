/**
 * OpenSpec sessions, as the panel follows them: the runs, the worktrees, and opening a run's tab.
 *
 * A session is an agent run (`cmd::spec_sessions`), so everything here is the MR reviewer's road
 * (`gitlab/agentReview.ts`) with the change in the review's place:
 *
 * - the runs arrive whole on `cide://spec-runs-changed`, sent from the registry's coalescer on
 *   every change to them — not on `cide://agents-changed`, whose roster holds no runs while
 *   subagents are off and so never moved when a session ended;
 * - the worktrees are re-read when the spec board moves (`spec_triggers` watches
 *   `.cide/worktrees/*\/openspec` too, so a box ticked in a worktree moves it) and after every
 *   gesture here, and never on a run event — each read is one `openspec list` per worktree;
 * - a tab is a **mirror** onto the run's child (`newRunTab`), so closing it leaves the work going,
 *   and *Open session* builds another one.
 */
import { create } from 'zustand'
import {
  agentRuns,
  specEvents,
  specSessions,
  type ProjectId,
} from '@/ipc/client'
import type {
  RunId,
  SpecCheckout,
  SpecRunRow,
  SpecSessionStart,
  SplitIntent,
} from '@/ipc/generated'
import { sessionTitle } from './model'

interface SpecRunsState {
  project: ProjectId | null
  runs: readonly SpecRunRow[]
  checkouts: readonly SpecCheckout[]
  attach: (project: ProjectId | null) => void
  refreshRuns: () => Promise<void>
  /** Take the rows a `cide://spec-runs-changed` carried, for the project this panel shows. */
  adoptRuns: (project: ProjectId, runs: readonly SpecRunRow[]) => void
  refreshCheckouts: () => Promise<void>
}

let generation = 0

export const useSpecRuns = create<SpecRunsState>((set, get) => ({
  project: null,
  runs: [],
  checkouts: [],

  attach: (project) => {
    if (get().project === project) return
    generation += 1
    // Cleared before the reads: another project's sessions under this one's changes would be a
    // wrong answer that reads as real.
    set({ project, runs: [], checkouts: [] })
    if (project === null) return
    void get().refreshRuns()
    void get().refreshCheckouts()
  },

  refreshRuns: async () => {
    const project = get().project
    if (project === null) return
    const mine = ++generation
    const runs = await specSessions.runs(project).catch(() => null)
    if (runs === null || mine !== generation || get().project !== project) return
    set({ runs })
  },

  adoptRuns: (project, runs) => {
    if (get().project !== project) return
    // Claims the generation, so a `refreshRuns` already in flight cannot land its older answer
    // over these.
    generation += 1
    set({ runs })
  },

  refreshCheckouts: async () => {
    const project = get().project
    if (project === null) return
    const checkouts = await specSessions.checkouts(project).catch(() => null)
    if (checkouts === null || get().project !== project) return
    set({ checkouts })
  },
}))

/**
 * Start a session, then open its tab as soon as it has a child.
 *
 * The run is queued first and forks when the project has room, so the tab cannot be built at
 * once; the MR reviewer's follow loop polls for the same moment. Gives up quietly after a while —
 * a run still queued then is on the panel with its chip, and *Open session* opens it later.
 */
export async function startSession(project: ProjectId, request: SpecSessionStart): Promise<RunId> {
  const run = await specSessions.start(project, request)
  void useSpecRuns.getState().refreshRuns()
  void openWhenReady(project, run, sessionTitle(request.op, request.change ?? null, labelOf(request)))
  return run
}

function labelOf(request: SpecSessionStart): string {
  return request.launcher.kind === 'role' ? request.launcher.agent : request.launcher.harness
}

const READY_TRIES = 90
const READY_EVERY_MS = 1000

async function openWhenReady(project: ProjectId, run: RunId, title: string): Promise<void> {
  for (let tries = 0; tries < READY_TRIES; tries += 1) {
    if (await openRunTab(project, run, title).catch(() => false)) return
    await new Promise((resolve) => setTimeout(resolve, READY_EVERY_MS))
  }
}

/**
 * Open a tab onto a session's run. Answers whether one was opened: `false` while the run has no
 * child and no conversation to continue — queued, or never started.
 */
export async function openRunTab(project: ProjectId, run: RunId, title: string): Promise<boolean> {
  const plan = await agentRuns.open(project, run)
  if (plan.kind === 'unavailable') return false
  const intent: SplitIntent =
    plan.kind === 'continue'
      ? { kind: 'continue', conversation: plan.conversation }
      : plan.continues !== null
        ? { kind: 'mirror', session: plan.session, continues: plan.continues }
        : { kind: 'mirror', session: plan.session }
  const { useWorkspace } = await import('@/store/workspace')
  await useWorkspace.getState().newRunTab(project, title, intent)
  return true
}

/** *Open session* on a row: the tab, or the sentence saying why there is nothing to open yet. */
export async function openSession(project: ProjectId, row: SpecRunRow): Promise<void> {
  const opened = await openRunTab(project, row.run, sessionTitle(row.op, row.change ?? null, row.label))
  if (!opened) throw new Error('This session has not started yet — it is waiting for a run slot.')
}

/**
 * Follow a project's sessions and worktrees: attach, read both, and keep them live — the rows on
 * `cide://spec-runs-changed`, the worktrees when the spec board moves. Answers the cleanup.
 *
 * Called by the panel **and** the change page, because the page can be open with the panel shut,
 * and it has to know a change has a session (Open session, not a second Apply) and whether it was
 * applied in a worktree (which acts a finished change offers). Two followers of one project hear
 * the same events twice and set the same rows; that costs nothing.
 */
export function followSpecRuns(project: ProjectId | null): () => void {
  const store = useSpecRuns.getState()
  store.attach(project)
  if (project === null) return () => {}
  void store.refreshRuns()
  void store.refreshCheckouts()
  const offRuns = specSessions.onChanged((changed, rows) => {
    useSpecRuns.getState().adoptRuns(changed, rows)
  })
  const offBoard = specEvents.onChanged((changed) => {
    if (changed === project) void useSpecRuns.getState().refreshCheckouts()
  })
  return () => {
    void offRuns.then((stop) => stop())
    void offBoard.then((stop) => stop())
  }
}
