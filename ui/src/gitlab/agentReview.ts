/**
 * An agent reviewing an MR, as the MR panel follows it. (M85)
 *
 * The run is an ordinary cide run (`crates/cide-app/src/mr_review.rs` says why), so this file
 * only launches it, watches it, and opens its tab once there is a child to show — a queued run
 * has none, and a tab opened onto nothing would spawn a stray `claude` in its place.
 *
 * Followed through `gitlab.reviewRun` rather than the Agents roster: a project whose subagents
 * are switched off answers a roster with no runs in it, and a review may run there.
 */
import {
  agentRuns,
  gitlab,
  type ProjectId,
  type RunId,
  type SplitIntent,
} from '@/ipc/client'
import type { Harness } from '@/ipc/generated'
import { data, touch } from './store'

export interface AgentReview {
  project: ProjectId
  run: RunId
  harness: Harness
  state: 'queued' | 'running' | 'finished' | 'failed'
  detail: string
  tabOpened: boolean
}

/** The latest review started per MR in this window. */
export const agentReviews = new Map<string, AgentReview>()

const POLL_MS = 1500

export const HARNESS_LABEL: Record<Harness, string> = {
  claude: 'Claude Code',
  opencode: 'opencode',
  qwen: 'Qwen Code',
  codex: 'Codex',
  mimo: 'MiMo',
}

export async function startAgentReview(
  review: string,
  harness: Harness,
  prompt: string,
): Promise<void> {
  const [{ useWorkspace }, { activeProjectIdOf }] = await Promise.all([
    import('@/store/workspace'),
    import('@/keys/target'),
  ])
  const project = activeProjectIdOf(useWorkspace.getState().boot)
  if (!project)
    throw new Error(
      'Open a workspace project to host the review. The MR may belong to any repository.',
    )
  const run = await gitlab.launchReview(
    project,
    review,
    harness,
    prompt.trim() || null,
  )
  const entry: AgentReview = {
    project,
    run,
    harness,
    state: 'queued',
    detail: 'Waiting for a run slot…',
    tabOpened: false,
  }
  agentReviews.set(review, entry)
  touch()
  void follow(review, entry)
}

export async function stopAgentReview(review: string): Promise<void> {
  const entry = agentReviews.get(review)
  if (entry) await agentRuns.stop(entry.project, entry.run)
}

/** Open the run's tab again — after the user closed it, or once it finished. */
export async function showAgentReview(review: string): Promise<void> {
  const entry = agentReviews.get(review)
  if (!entry) return
  if (!(await openTab(review, entry)))
    throw new Error('This review has nothing to show yet.')
}

function update(entry: AgentReview, patch: Partial<AgentReview>) {
  Object.assign(entry, patch)
  touch()
}

async function follow(review: string, entry: AgentReview) {
  // Stops when another review of this MR replaces this one, or when the run is over.
  while (agentReviews.get(review) === entry) {
    let row
    try {
      row = await gitlab.reviewRun(entry.project, entry.run)
    } catch {
      row = undefined
    }
    if (row === null) {
      update(entry, { state: 'finished', detail: 'The run is no longer listed.' })
      return
    }
    if (row) {
      const state = row.state
      if (state.state === 'failed') {
        update(entry, { state: 'failed', detail: state.reason })
        return
      }
      if (state.state === 'finished') {
        if (!entry.tabOpened) await openTab(review, entry).catch(() => false)
        update(entry, {
          state: 'finished',
          detail:
            state.code === 0
              ? 'Finished. Its findings are in Drafts.'
              : `Ended with exit ${state.code}.`,
        })
        return
      }
      if (state.state === 'queued') {
        update(entry, {
          state: 'queued',
          detail: row.note ?? 'Waiting for a run slot…',
        })
      } else {
        update(entry, { state: 'running', detail: 'Reviewing…' })
        if (!entry.tabOpened) await openTab(review, entry).catch(() => false)
      }
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_MS))
  }
}

/** Answers whether a tab was opened. A run with no child yet answers `false`; ask again. */
async function openTab(review: string, entry: AgentReview): Promise<boolean> {
  const plan = await agentRuns.open(entry.project, entry.run)
  if (plan.kind === 'unavailable') return false
  const intent: SplitIntent =
    plan.kind === 'continue'
      ? { kind: 'continue', conversation: plan.conversation }
      : plan.continues !== null
        ? { kind: 'mirror', session: plan.session, continues: plan.continues }
        : { kind: 'mirror', session: plan.session }
  const iid = data.get(review)?.mr.iid
  const { useWorkspace } = await import('@/store/workspace')
  await useWorkspace
    .getState()
    .newRunTab(
      entry.project,
      `Review${iid === undefined ? '' : ` !${iid}`} · ${HARNESS_LABEL[entry.harness]}`,
      intent,
    )
  update(entry, { tabOpened: true })
  return true
}
