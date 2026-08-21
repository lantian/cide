/**
 * *Merging origin/main into main — 2 of 3 resolved.* (M20)
 *
 * `GuardBar.tsx`'s sibling, and its header's argument is the one that decides this component's
 * whole shape: **a bar, not a modal.** A modal would interrupt a commit message being typed,
 * and — the half that matters more here — the honest default of a conflict is *do nothing yet*.
 * A user who has just landed a merge may well want to read a file, run a build, or look at the
 * log before deciding anything, and a dialog they have to dismiss to do that teaches them to
 * dismiss dialogs.
 *
 * It owns no rules and makes no calls: `useGitPanel` holds the state and the verbs, exactly as
 * `GuardBar` does, so `check:render` can draw this from a fixture.
 */
import type { MergeState } from '@/ipc/client'
// `GuardBar`'s stylesheet, shared rather than copied: the two bars sit in the same slot,
// carry the same three parts (a glyph, a sentence, a row of equal-weight actions) and must not
// drift apart visually. The one thing this bar overrides is the glyph's colour — see
// `.glyphConflict` — because yellow means *two views disagree* and this is not that.
import styles from './GuardBar.module.css'

export interface MergeBarProps {
  state: MergeState
  /** Disabled while a verb is in flight, so Continue cannot be pressed twice. */
  busy: boolean
  onContinue: () => void
  onAbort: () => void
  onResolveSimple: () => void
  /** Reopen the *Files merged with conflicts* list, which opened itself when this landed. */
  onShowList: () => void
}

export function MergeBar({
  state,
  busy,
  onContinue,
  onAbort,
  onResolveSimple,
  onShowList,
}: MergeBarProps) {
  const total = state.entries.length
  const left = state.entries.filter((e) => !e.resolved).length
  const done = total - left

  // `git status`'s own vocabulary, because the bar and a terminal in the next pane are looking
  // at the same repository and must not describe it differently.
  const what =
    state.operation === 'rebase'
      ? `Rebasing ${state.ours} onto ${state.theirs}`
      : `Merging ${state.theirs} into ${state.ours}`
  const step =
    state.step !== undefined && state.step !== null
      ? ` · step ${state.step.done} of ${state.step.total}`
      : ''
  const counted = total === 0 ? '' : ` · ${done} of ${total} resolved`

  return (
    <div className={styles.bar} role="status" data-audit="mergeBar">
      <span className={`${styles.glyph} ${styles.glyphConflict}`} aria-hidden="true">
        ⚔
      </span>
      <span className={styles.text} data-audit="mergeBarText">
        {what}
        {step}
        {counted}
      </span>
      {left > 0 && (
        <button
          type="button"
          className={styles.action}
          onClick={onShowList}
          disabled={busy}
          title="The list of conflicted files, with a decision on each"
          data-audit="mergeBarList"
        >
          Resolve conflicts…
        </button>
      )}
      {left > 0 && (
        <button
          type="button"
          className={styles.action}
          onClick={onResolveSimple}
          disabled={busy}
          title="Answer every conflicted file whose two sides say the same thing"
        >
          Resolve simple
        </button>
      )}
      <button
        type="button"
        className={styles.action}
        onClick={onContinue}
        // IDEA's *Accept and Finish* rule: there is nothing to continue *to* while a file is
        // still unresolved, and `cide_git::conflict::cont` refuses it anyway — so the button
        // says so before it is pressed rather than after.
        disabled={busy || left > 0}
        title={
          left > 0
            ? `${left} file${left === 1 ? '' : 's'} still to resolve`
            : state.operation === 'rebase'
              ? 'Commit this step and go on to the next'
              : 'Commit the merge'
        }
        data-audit="mergeBarContinue"
      >
        Continue
      </button>
      <button
        type="button"
        className={styles.action}
        onClick={onAbort}
        disabled={busy}
        title="Put the working tree back exactly as it was before this started"
        data-audit="mergeBarAbort"
      >
        Abort
      </button>
    </div>
  )
}
