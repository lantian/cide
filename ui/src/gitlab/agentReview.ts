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
  tab as tabApi,
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
  /**
   * `idle` is a reviewer whose turn is over but whose conversation is still alive — a claude
   * reviewer does not exit after its summary. It used to fall through to `running`, so a review
   * that had long finished read "Reviewing…" with a Stop button; and it is the state in which a
   * draft's Discuss reaches the reviewer by typing rather than by reviving it.
   *
   * `stopped` is a run that ended without finishing its work: stopped by the user (the block's
   * Stop, or closing the MR) or interrupted by cide quitting. Both also used to fall through to
   * `running`, so a stopped review read "Reviewing…" with a spinner for as long as the panel
   * was open.
   */
  state: 'queued' | 'running' | 'idle' | 'waiting' | 'finished' | 'stopped' | 'failed'
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
  adoptAgentReview(review, project, run, harness)
}

/**
 * Follow a review run this window did not launch from the dialog: the one a draft's Discuss
 * revived (`gitlab.draftReply` answers its run id). Replaces any entry for the MR, which ends
 * that entry's `follow` loop.
 */
export function adoptAgentReview(
  review: string,
  project: ProjectId,
  run: RunId,
  harness: Harness,
): void {
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
      // Exhaustive, with a `never` default: this was an if-chain whose `else` called every
      // state it did not name "Reviewing…", and `cancelled` — what Stop and closing the MR
      // both leave — was one of them. A new `RunState` variant is a type error here instead.
      const state = row.state
      switch (state.state) {
        case 'failed':
          update(entry, { state: 'failed', detail: state.reason })
          return
        case 'finished':
          if (!entry.tabOpened) await openTab(review, entry).catch(() => false)
          update(entry, {
            state: 'finished',
            detail:
              state.code === 0
                ? 'Finished. Its findings are in Drafts.'
                : `Ended with exit ${state.code}.`,
          })
          return
        case 'cancelled':
          update(entry, {
            state: 'stopped',
            detail: `Stopped (${state.reason}). Drafts it wrote are kept.`,
          })
          return
        case 'interrupted':
          update(entry, {
            state: 'stopped',
            detail: 'Interrupted when cide quit. Drafts it wrote are kept.',
          })
          return
        case 'queued':
          update(entry, {
            state: 'queued',
            detail: row.note ?? 'Waiting for a run slot…',
          })
          break
        case 'idle':
          // Keep following: a Discuss message starts another turn in this same run.
          update(entry, {
            state: 'idle',
            detail: 'Done. Its findings are in Drafts; Discuss a draft to ask about it.',
          })
          break
        case 'awaitingPermission':
          update(entry, {
            state: 'waiting',
            detail: 'Waiting for a permission answer in its tab.',
          })
          if (!entry.tabOpened) await openTab(review, entry).catch(() => false)
          break
        case 'blocked':
          // The turn failed but the child is alive: its tab is where it can be recovered.
          update(entry, { state: 'waiting', detail: state.reason })
          break
        case 'paused':
          update(entry, { state: 'waiting', detail: 'Paused.' })
          break
        case 'starting':
          update(entry, { state: 'running', detail: 'Starting…' })
          break
        case 'running':
          update(entry, { state: 'running', detail: 'Reviewing…' })
          if (!entry.tabOpened) await openTab(review, entry).catch(() => false)
          break
        default: {
          const unhandled: never = state
          void unhandled
        }
      }
    }
    await new Promise((resolve) => setTimeout(resolve, POLL_MS))
  }
}

/** Answers whether a tab was opened. A run with no child yet answers `false`; ask again. */
async function openTab(review: string, entry: AgentReview): Promise<boolean> {
  const plan = await agentRuns.open(entry.project, entry.run)
  if (plan.kind === 'unavailable') return false
  const iid = data.get(review)?.mr.iid
  if (plan.kind === 'readOnly') {
    const { useWorkspace } = await import('@/store/workspace')
    await tabApi.openConversation(entry.project, plan.conversation.id, `Review${iid === undefined ? '' : ` !${iid}`} · ${HARNESS_LABEL[entry.harness]}`)
    await useWorkspace.getState().synced()
    update(entry, { tabOpened: true })
    return true
  }
  const intent: SplitIntent =
    plan.kind === 'continue'
      ? { kind: 'continue', conversation: plan.conversation }
      : plan.continues !== null
        ? { kind: 'mirror', session: plan.session, continues: plan.continues }
        : { kind: 'mirror', session: plan.session }
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
