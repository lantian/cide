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
import type { ConfirmState } from '@/chrome/ConfirmDestructive'
import type { ReadyAct } from './model'
import { startSession, useSpecRuns } from './specRuns'

interface ConfirmStore {
  confirming: ConfirmState | null
  set: (confirming: ConfirmState | null) => void
}

/** Archive's confirm, waiting to be drawn. `SpecActsConfirm` is the one reader. */
export const useSpecConfirm = create<ConfirmStore>((set) => ({
  confirming: null,
  set: (confirming) => set({ confirming }),
}))

/** Run one of a finished change's acts. Every outcome is said, success included. */
export function runReadyAct(project: ProjectId, change: string, act: ReadyAct['id']): void {
  if (act === 'archive') archiveChange(project, change)
  else if (act === 'publish') publishChange(project, change)
  else integrateChange(project, change)
}

/**
 * Archive: plan first, then confirm, then run.
 *
 * The plan is a subprocess — it validates and reads the deltas — so a refusal arrives after the
 * click, as a notice naming what to do rather than a dialog to dismiss for nothing. Rust decides
 * where it runs: the change's worktree while that holds unmerged work, else the project root.
 */
export function archiveChange(project: ProjectId, change: string): void {
  void specApi
    .changePlan(project, change as never)
    .then((plan) => {
      if (plan.refusals.length > 0) {
        notify(`${change} cannot be archived yet.`, {
          kind: 'error',
          detail: plan.refusals.join('\n\n'),
        })
        return
      }
      useSpecConfirm.getState().set({
        title: `Archive ${change}?`,
        body:
          'Merges this change’s requirement edits into openspec/specs/ — the project’s ' +
          'source of truth — and moves the change to openspec/changes/archive/. Both are ' +
          'ordinary file edits in your repository, so git is the way back.',
        // Named, not counted: `ConfirmDestructive`'s rule is that the user is about to act on
        // *specific* files and "3 requirements" is not something anybody can check.
        files: plan.specsTouched.map(
          (touch) =>
            `openspec/specs/${touch.spec}/spec.md — ${touch.requirements} ${
              touch.requirements === 1 ? 'requirement' : 'requirements'
            } ${touch.operation}`,
        ),
        confirmLabel: 'Archive',
        defaultButton: 'confirm',
        danger: false,
        mark: 'file-diff',
        run: () => {
          void specApi
            .archiveChange(project, change as never)
            .then((outcome) => {
              if (outcome.kind === 'refused') {
                notify(`${change} was not archived.`, {
                  kind: 'error',
                  detail: outcome.plan.refusals.join('\n\n'),
                })
                return
              }
              notify(`${change} archived.`, { kind: 'ok' })
              void useSpecRuns.getState().refreshCheckouts()
            })
            .catch(notifyFailure)
        },
      })
    })
    .catch(notifyFailure)
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
