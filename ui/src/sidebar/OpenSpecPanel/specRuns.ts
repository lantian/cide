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
import { activeProjectIdOf } from '@/keys/target'
import { notifyFailure } from '@/chrome/notices'

interface SpecRunsState {
  project: ProjectId | null
  runs: readonly SpecRunRow[]
  checkouts: readonly SpecCheckout[]
  attach: (project: ProjectId | null, withCheckouts?: boolean) => void
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

  attach: (project, withCheckouts = true) => {
    if (get().project === project) return
    generation += 1
    // Cleared before the reads: another project's sessions under this one's changes would be a
    // wrong answer that reads as real.
    set({ project, runs: [], checkouts: [] })
    if (project === null) return
    void get().refreshRuns()
    if (withCheckouts) void get().refreshCheckouts()
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
 * once. Follow the registry's events until admission, including a run queued behind a long turn.
 * Switching projects cancels opening the tab while the admitted work continues.
 */
export async function startSession(project: ProjectId, request: SpecSessionStart, onStarted?: () => void): Promise<RunId> {
  const run = await specSessions.start(project, request)
  void useSpecRuns.getState().refreshRuns()
  onStarted?.()
  followRunTab(project, run, sessionTitle(request.op, request.change ?? null, labelOf(request)))
  return run
}

function labelOf(request: SpecSessionStart): string {
  return request.launcher.kind === 'role' ? request.launcher.agent : request.launcher.harness
}

const opening = new Set<RunId>()

/** Follow admission events, including runs queued longer than a polling timeout. A project
 * switch cancels presentation, not the run. Only a shell window opens automatic run views. */
export function followRunTab(project: ProjectId, run: RunId, title: string): void {
  if (opening.has(run)) return
  opening.add(run)
  let cleanup = () => { opening.delete(run) }
  void (async () => {
    const { useWorkspace } = await import('@/store/workspace')
    const current = () => {
      const boot = useWorkspace.getState().boot
      return boot?.role.kind === 'shell' && activeProjectIdOf(boot) === project
    }
    let ended = false
    let checking = false
    let offRuns: (() => void) | undefined
    let offWorkspace: (() => void) | undefined
    const finish = () => {
      ended = true
      opening.delete(run)
      offRuns?.()
      offWorkspace?.()
    }
    cleanup = finish
    const check = async (rows: readonly SpecRunRow[]) => {
      if (ended || checking) return
      if (!current()) { finish(); return }
      const row = rows.find((r) => r.run === run)
      if (!row || row.state.state === 'queued' || row.state.state === 'starting') return
      if (row.state.state === 'failed' && !row.session) {
        finish()
        notifyFailure(row.state.reason, { project })
        return
      }
      checking = true
      try {
        if (!current()) { finish(); return }
        await openRunTab(project, run, title, current)
        finish()
      } catch (error) {
        finish()
        notifyFailure(error, { project })
      } finally { checking = false }
    }
    if (!current()) { finish(); return }
    offWorkspace = useWorkspace.subscribe(() => { if (!current()) finish() })
    offRuns = await specSessions.onChanged((changed, rows) => {
      if (changed === project) void check(rows)
    })
    if (ended) { offRuns(); return }
    await check(await specSessions.runs(project))
  })().catch((error: unknown) => {
    cleanup()
    notifyFailure(error, { project })
  })
}

/**
 * Open a tab onto a session's run. Answers whether one was opened: `false` while the run has no
 * child and no conversation to continue — queued, or never started.
 */
export async function openRunTab(project: ProjectId, run: RunId, title: string, shouldOpen?: () => boolean): Promise<boolean> {
  const plan = await agentRuns.open(project, run)
  if (plan.kind === 'unavailable') return false
  const intent: SplitIntent =
    plan.kind === 'continue'
      ? { kind: 'continue', conversation: plan.conversation }
      : plan.continues !== null
        ? { kind: 'mirror', session: plan.session, continues: plan.continues }
        : { kind: 'mirror', session: plan.session }
  const { useWorkspace } = await import('@/store/workspace')
  if (shouldOpen && !shouldOpen()) return false
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
 * Called by the panel, the change page and task cards. A task card sets `withCheckouts` false
 * because linking its proposal needs only runs, without reading worktree checklists.
 * The page can be open with the panel shut,
 * and it has to know a change has a session (Open session, not a second Apply) and whether it was
 * applied in a worktree (which acts a finished change offers). Two followers of one project hear
 * the same events twice and set the same rows; that costs nothing.
 */
export function followSpecRuns(project: ProjectId | null, withCheckouts = true): () => void {
  const store = useSpecRuns.getState()
  store.attach(project, withCheckouts)
  if (project === null) return () => {}
  void store.refreshRuns()
  if (withCheckouts) void store.refreshCheckouts()
  const offRuns = specSessions.onChanged((changed, rows) => {
    useSpecRuns.getState().adoptRuns(changed, rows)
  })
  const offBoard = withCheckouts ? specEvents.onChanged((changed) => {
    if (changed === project) void useSpecRuns.getState().refreshCheckouts()
  }) : Promise.resolve(() => {})
  return () => {
    void offRuns.then((stop) => stop())
    void offBoard.then((stop) => stop())
  }
}
