/**
 * *These commits are about to leave this machine* — the rules behind the push dialog. (M31)
 *
 * # Why this is its own module
 *
 * `chrome/pullStrategyModel.ts`'s reasons, mirrored. The sentences a *finished* push produces
 * live in `chrome/branchModel.ts` (`pushNote`, `pushReport`) because they belong to the branch
 * selector that has always shown them; the sentences a push is *proposed* with belong to a
 * dialog raised by the key gate, from `keys/dispatch.ts`, in both window kinds. Its own module
 * also means its own check (`ui/scripts/check-push.mjs`) rather than another two hundred
 * assertions in `check-branches.mjs`.
 *
 * **Zero imports, not even type-only ones.** `chrome/logActions.ts`'s rule, for its reason: a
 * value import — even from another import-free module — is what stops the check compiling this
 * file on its own. The shapes below are declared structurally and pinned against
 * `ui/src/ipc/generated.ts` by the check, which is the same guarantee from the other side. It is
 * also why `commits()` at the foot is duplicated rather than imported.
 *
 * # The three rules this file exists to keep honest
 *
 * **Force starts unticked, always, on every open.** This is `pullStrategyModel.ts`'s inversion
 * of `ConfirmDestructive`'s house rule and it inverts it harder. That component ticks its option
 * by default because its option makes the act *recoverable*; this one makes the act
 * *irreversible on somebody else's machine*. A force push that lands is not undone by anything
 * cide can offer — the commits it replaced are unreachable on the remote and, for everyone who
 * had already fetched them, present only in a reflog they do not know to look in.
 *
 * **Force is offered as an answer only where it is one.** [`forceNote`] returns a sentence for a
 * diverged row and nothing at all otherwise. A checkbox that reads the same whether it is about
 * to overwrite a colleague's work or about to do nothing at all is a checkbox that teaches
 * people to tick it.
 *
 * **A row that cannot push says why.** Three blocked states, three remedies, three sentences —
 * see [`blockedNote`]. The alternative, a disabled row with no explanation, reads as a bug in
 * cide rather than as a state of the repository, and the state is usually one the user can fix
 * in ten seconds.
 */

/** The generated `PulledCommit`, structurally. */
export interface PushCommit {
  readonly shortOid: string
  readonly summary: string
  readonly author: string
}

/** The generated `PushBlock`. */
export type PushBlocked = 'unborn' | 'detached' | 'noRemote'

/** The generated `PushPreview`, structurally — only the fields the dialog reads. */
export interface PushPreviewLike {
  readonly repo: { readonly id: string; readonly name: string }
  /** The generated `BranchInfo`, of which only the branch name is drawn. */
  readonly head: { readonly head: string }
  readonly remote: string
  readonly remotes: readonly string[]
  readonly refspec: string
  readonly publish: boolean
  readonly commits: readonly PushCommit[]
  readonly more: number
  readonly diverged: boolean
  readonly shellsOut: boolean
  readonly blocked?: PushBlocked | null | undefined
}

/**
 * How many commits this row would send.
 *
 * `commits.length + more` rather than `head.ahead`, and the difference is not pedantry.
 * `BranchInfo.ahead` is measured against the branch's configured **upstream**; this is measured
 * against the remote-tracking ref the refspec actually names, and it is the only one of the two
 * that is defined for a branch with no upstream at all — which is the row the dialog most needs
 * a number for, because *publish this branch* is the push people are least sure about.
 */
export function outgoing(preview: PushPreviewLike): number {
  return preview.commits.length + preview.more
}

/** Rows this dialog is able to push at all. Blocked rows are drawn, never pushed. */
export function pushable(preview: PushPreviewLike): boolean {
  const blocked = preview.blocked ?? null
  return blocked === null && outgoing(preview) > 0
}

/**
 * Which rows are ticked when the dialog opens: every one with something to send.
 *
 * Not "every row" — a repository already up to date would then be ticked, the button would count
 * it, and the push would report *already up to date* for something the user never asked about.
 * Not "the first row" either: `git.push` has always acted on every repository in the project,
 * and a dialog that quietly narrowed that would change what the command means.
 */
export function defaultChecked(previews: readonly PushPreviewLike[]): string[] {
  const out: string[] = []
  for (const preview of previews) {
    if (pushable(preview)) out.push(preview.repo.id)
  }
  return out
}

/**
 * `master → origin: master`, the row's target.
 *
 * Read off [`PushPreviewLike.refspec`] rather than rebuilt from the branch name, because the
 * refspec is what will actually be sent and a push may rename on the way. A refspec this cannot
 * parse — a blocked row's empty one — gives the remote alone, which is still true.
 */
export function targetLabel(preview: PushPreviewLike): string {
  const dst = destination(preview.refspec)
  if (dst === null) return preview.remote
  const label = `${preview.head.head} → ${preview.remote}: ${dst}`
  return preview.publish ? `${label} (new branch)` : label
}

/** The destination branch of a `src:dst` refspec, short. `null` when there is not one. */
function destination(refspec: string): string | null {
  if (refspec === '') return null
  const cut = refspec.indexOf(':')
  const dst = cut === -1 ? refspec : refspec.slice(cut + 1)
  const prefix = 'refs/heads/'
  return dst.startsWith(prefix) ? dst.slice(prefix.length) : dst === '' ? null : dst
}

/**
 * Why this row cannot push, and what to do about it. Empty for a row that can.
 *
 * Each sentence names the remedy, because all three are ten seconds of work and a refusal that
 * does not say so reads as cide declining rather than as git being unable.
 */
export function blockedNote(preview: PushPreviewLike): string {
  switch (preview.blocked ?? null) {
    case 'unborn':
      return 'No commits yet — there is nothing to push.'
    case 'detached':
      return 'HEAD is detached, so there is no branch to push. Check a branch out first.'
    case 'noRemote':
      return `No remote named ${preview.remote}. Add one with \`git remote add\`.`
    default:
      return ''
  }
}

/**
 * The warning a diverged row carries. Empty for every other row.
 *
 * The two wordings are not decoration: they name **who** performs the lease. On the binary route
 * it is git's own `--force-with-lease`, checked against the remote-tracking ref at push time. On
 * the libgit2 route — a local or `file://` remote, the only one it takes — libgit2 has no lease
 * at all, so cide connects, reads what the remote advertises and compares it itself. A user who
 * is told "git will refuse if the remote moved" about the second case has been told something
 * false about which program is protecting them.
 */
export function forceNote(preview: PushPreviewLike): string {
  if (!preview.diverged) return ''
  const behind = `${preview.remote}/${destination(preview.refspec) ?? preview.head.head}`
  const lease = preview.shellsOut
    ? 'git refuses the push if the remote moved since your last fetch.'
    : 'cide checks the remote against your last fetch and refuses if it moved.'
  return `${behind} has commits this branch does not. A plain push is rejected; forcing overwrites them — ${lease}`
}

/** How many commits the ticked rows would send in total. The number on the button. */
export function outgoingTotal(
  previews: readonly PushPreviewLike[],
  checked: readonly string[],
): number {
  let total = 0
  for (const preview of previews) {
    if (checked.includes(preview.repo.id) && pushable(preview)) total += outgoing(preview)
  }
  return total
}

/**
 * The word on the button.
 *
 * The count is in it — `Push 2 commits`, never a bare *Push* — for `ConfirmDestructive`'s rule 1
 * restated: the user is acting on a *specific* set, and a button that does not say how big it is
 * is the one thing they cannot check before clicking.
 */
export function confirmLabel(total: number, force: boolean): string {
  if (total === 0) return force ? 'Force push' : 'Push'
  return `${force ? 'Force push' : 'Push'} ${commits(total)}`
}

/**
 * The sentence under the title.
 *
 * States the shape of what is about to happen, and nothing about force — the checkbox and the
 * per-row warnings say that, and a body that repeated it would be the third place one sentence
 * lives.
 */
export function pushSummary(
  previews: readonly PushPreviewLike[],
  checked: readonly string[],
): string {
  const rows = previews.filter((p) => checked.includes(p.repo.id) && pushable(p))
  const total = outgoingTotal(previews, checked)
  if (rows.length === 0) {
    return previews.some(pushable)
      ? 'Nothing selected. Tick a repository to push it.'
      : 'Everything is already up to date.'
  }
  const first = rows[0]
  if (rows.length === 1 && first !== undefined) {
    return `${commits(total)} to ${targetLabel(first)}.`
  }
  return `${commits(total)} from ${rows.length} repositories.`
}

/**
 * Whether ticking Force can change anything about this push.
 *
 * Drives the checkbox's enablement. `false` when no ticked row has diverged: `--force-with-lease`
 * over a fast-forward is a no-op, and offering it live would let somebody form the habit of
 * ticking it on the pushes where it is harmless.
 */
export function forceApplies(
  previews: readonly PushPreviewLike[],
  checked: readonly string[],
): boolean {
  return previews.some((p) => checked.includes(p.repo.id) && pushable(p) && p.diverged)
}

/**
 * `1 commit` / `4 commits`.
 *
 * Duplicated from `branchModel.ts` rather than imported, and the duplication is the point: see
 * this file's header, and the identical note at the foot of `chrome/pullStrategyModel.ts`.
 */
function commits(n: number): string {
  return `${n} ${n === 1 ? 'commit' : 'commits'}`
}
