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
  events,
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
let attachment = 0
const checkoutReads = new Map<ProjectId, { dirty: boolean; promise: Promise<void> }>()

export const useSpecRuns = create<SpecRunsState>((set, get) => ({
  project: null,
  runs: [],
  checkouts: [],

  attach: (project, withCheckouts = true) => {
    if (get().project === project) return
    generation += 1
    attachment += 1
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

  refreshCheckouts: () => {
    const project = get().project
    if (project === null) return Promise.resolve()
    const existing = checkoutReads.get(project)
    if (existing) { existing.dirty = true; return existing.promise }
    const request = { dirty: false, promise: Promise.resolve() }
    request.promise = (async () => {
      do {
        request.dirty = false
        const mine = attachment
        const checkouts = await specSessions.checkouts(project).catch(() => null)
        if (get().project !== project) return
        if (mine !== attachment) { request.dirty = true; continue }
        if (checkouts !== null && !request.dirty) set({ checkouts })
      } while (request.dirty)
    })().finally(() => { checkoutReads.delete(project) })
    checkoutReads.set(project, request)
    return request.promise
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
  if (opened) return
  // A run that died before forking has no conversation to open, and the queue is not why — the
  // row already carries the sentence the failure was reported with (a worktree fork into a
  // repository with no commits, say). Only a run that is genuinely waiting gets the slot sentence.
  if (row.state.state === 'failed') throw new Error(`This session failed before it started — ${row.state.reason}`)
  throw new Error('This session has not started yet — it is waiting for a run slot.')
}

/** One application-owned subscription, independent of mounted panels and tabs. */
export function followSpecRuns(project: ProjectId | null): () => void {
  useSpecRuns.getState().attach(project)
  if (project === null) return () => {}
  const offRuns = specSessions.onChanged((changed, rows) => {
    useSpecRuns.getState().adoptRuns(changed, rows)
  })
  const offGit = events.onGitStatus((changed) => {
    if (changed === project) void useSpecRuns.getState().refreshCheckouts()
  })
  return () => {
    void offRuns.then((stop) => stop())
    void offGit.then((stop) => stop())
  }
}
