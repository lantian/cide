/**
 * What a finished change offers — Publish, Integrate, Archive — as one module both doors call.
 * (OpenSpec sessions)
 *
 * The panel row and the change page draw the same three buttons, and two copies of what pressing
 * them does is how the page came to offer *Apply…* on a change the panel already showed as done.
 * So the gestures live here, and so does Archive's confirm: a small store the one
 * `SpecActsConfirm` (rendered by `App.tsx`) draws, whichever surface asked.
 *
 * Import-light on purpose — no React, no portal — because `SpecTab.tsx` exports a view the render
 * check SSR-bundles under node, and this is reached from its host.
 */
import { create } from 'zustand'
import {
  git as gitApi,
  spec as specApi,
  specSessions,
  type ProjectId,
} from '@/ipc/client'
import { notify, notifyFailure, type NoticeAction } from '@/chrome/notices'
import type { SpecAcceptPlan } from '@/ipc/generated'
import type { ReadyAct } from './model'
import { startSession, useSpecRuns } from './specRuns'

export interface ArchiveConfirmation {
  project: ProjectId
  change: string
  phase: 'preparing' | 'ready' | 'archiving' | 'failed'
  plan: SpecAcceptPlan | null
  error: string | null
}
interface ConfirmStore {
  confirming: ArchiveConfirmation | null
  set: (confirming: ArchiveConfirmation | null) => void
}

export const useSpecConfirm = create<ConfirmStore>((set) => ({
  confirming: null,
  set: (confirming) => set({ confirming }),
}))

// Persists after dismissing the dialog, so another surface cannot submit the same operation.
const archiving = new Map<string, ArchiveConfirmation>()
const archiveKey = (project: ProjectId, change: string) => `${project}:${change}`
const messageOf = (error: unknown) => error instanceof Error ? error.message : String(error)

export function runReadyAct(project: ProjectId, change: string, act: ReadyAct['id']): void {
  if (act === 'archive') archiveChange(project, change)
  else if (act === 'publish') publishChange(project, change)
  else integrateChange(project, change)
}

/** Open immediately; the preview is asynchronous and never silently consumes a click. */
export function archiveChange(project: ProjectId, change: string, force = false): void {
  const running = archiving.get(archiveKey(project, change))
  if (running) { useSpecConfirm.getState().set(running); return }
  const current = useSpecConfirm.getState().confirming
  if (current?.project === project && current.change === change && current.phase === 'preparing') return
  const state: ArchiveConfirmation = { project, change, phase: 'preparing', plan: null, error: null }
  useSpecConfirm.getState().set(state)
  void specApi.changePlan(project, change as never, force).then((plan) => {
    if (useSpecConfirm.getState().confirming !== state) return
    useSpecConfirm.getState().set({ ...state, plan, phase: plan.refusals.length ? 'failed' : 'ready', error: plan.refusals.join('\n\n') || null })
  }).catch((error: unknown) => {
    if (useSpecConfirm.getState().confirming === state) useSpecConfirm.getState().set({ ...state, phase: 'failed', error: messageOf(error) })
    else notifyFailure(error, { project })
  })
}

/** A retry prepares a fresh preview; it never repeats a mutation automatically. */
export async function confirmArchive(): Promise<void> {
  const current = useSpecConfirm.getState().confirming
  if (!current) return
  if (current.phase === 'failed') { archiveChange(current.project, current.change, true); return }
  if (current.phase !== 'ready' || !current.plan) return
  const { project, change, plan } = current
  const key = archiveKey(project, change)
  if (archiving.has(key)) return
  const running: ArchiveConfirmation = { ...current, phase: 'archiving', error: null }
  archiving.set(key, running)
  useSpecConfirm.getState().set(running)
  try {
    const outcome = await specApi.archiveChange(project, change as never, plan.archiveRoot)
    switch (outcome.kind) {
      case 'accepted':
        if (useSpecConfirm.getState().confirming === running) useSpecConfirm.getState().set(null)
        notify(`${change} archived.`, { kind: 'ok', detail: outcome.archive.path })
        void useSpecRuns.getState().refreshCheckouts()
        break
      case 'refused':
        throw new Error(outcome.plan.refusals.join('\n\n') || 'Archive was refused. Review a fresh preview.')
      case 'conflicts':
        throw new Error(`Archive was not completed. Conflicting paths: ${outcome.paths.join(', ')}`)
    }
  } catch (error) {
    if (useSpecConfirm.getState().confirming === running) useSpecConfirm.getState().set({ ...running, phase: 'failed', error: messageOf(error) })
    notify(`${change} was not archived.`, { kind: 'error', detail: messageOf(error) })
  } finally { archiving.delete(key) }
}

/** Publish: commit what the session left, push its branch, then offer the merge request. */
export function publishChange(project: ProjectId, change: string): void {
  void specSessions
    .publish(project, change as never)
    .then(async (outcome) => {
      if (outcome.kind === 'refused') {
        notify(`${change} was not published.`, { kind: 'error', detail: outcome.reason })
        return
      }
      notify(`Pushed ${outcome.branch} to ${outcome.remote}.`, {
        kind: 'ok',
        ...(outcome.committed === undefined
          ? {}
          : { detail: `Committed what was left first: ${outcome.committed.slice(0, 8)}.` }),
      })
      void useSpecRuns.getState().refreshCheckouts()
      // The push dialog's own follow-up: offer the merge request when a forge knows the remote.
      // The project's first repository is the one the worktree shares a config with.
      const previews = await gitApi.pushPlan(project).catch(() => [])
      const repo = previews[0]?.repo.id
      if (repo !== undefined) {
        const { checkAfterPush } = await import('@/gitlab/PushMRPrompt')
        checkAfterPush(project, repo, outcome.remote, outcome.branch)
      }
    })
    .catch(notifyFailure)
}

/**
 * The way out of an Integrate that could not merge: a session of the main model (Settings →
 * Harness — Claude or Codex), in a new tab, told to merge `cide/spec-<change>` and resolve the
 * conflicts. Offered on every notice where Integrate stopped short, so a conflict is never a
 * dead end with a list of paths.
 */
function resolveAction(project: ProjectId, change: string): NoticeAction {
  return {
    label: 'Resolve in a new session',
    run: () => {
      void (async () => {
        const { settings } = await import('@/ipc/client')
        const setting = (await settings.effective(project)).consoleHarness
        await startSession(project, {
          op: 'merge',
          change,
          launcher: { kind: 'harness', harness: setting },
        })
      })().catch(notifyFailure)
    },
  }
}

/** Integrate: commit, verify, merge into the checked-out branch. Never archives. */
export function integrateChange(project: ProjectId, change: string): void {
  void specSessions
    .integrate(project, change as never)
    .then((outcome) => {
      switch (outcome.kind) {
        case 'merged':
          notify(`Integrated ${change}: ${outcome.files} files.`, {
            kind: outcome.kept.length === 0 ? 'ok' : 'warn',
            // The root's own copies the user edited while the session worked: set aside, not
            // dropped, when the branch's versions landed.
            ...(outcome.kept.length === 0
              ? {}
              : {
                  detail:
                    'You had edited these in the project while the session worked; the ' +
                    'branch’s versions landed, and yours are kept here:\n' +
                    outcome.kept.join('\n'),
                }),
          })
          break
        case 'upToDate':
          notify(`${change} was already in this branch.`, { kind: 'ok' })
          break
        case 'conflicts':
          notify(`${change} would conflict, so nothing was merged.`, {
            kind: 'error',
            detail: outcome.paths.join('\n'),
            actions: [resolveAction(project, change)],
          })
          break
        case 'refused':
          notify(`${change} was not integrated.`, { kind: 'error', detail: outcome.reason })
          break
      }
      void useSpecRuns.getState().refreshCheckouts()
    })
    // A merge git refused outright — untracked files in the way, a checkout it would overwrite —
    // arrives as an error rather than a conflict list, and wants the same way out.
    .catch((error: unknown) => {
      notify(`${change} could not be integrated.`, {
        kind: 'error',
        detail: error instanceof Error ? error.message : String(error),
        actions: [resolveAction(project, change)],
      })
    })
}
