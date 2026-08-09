/**
 * The footer: `Amend`, the selection summary, the 104px message box, and the two buttons.
 *
 * Ticking `Amend` prefills HEAD's message when the box is empty — that decision lives in
 * `useGitPanel`, because it needs the repo, and this component stays a render target.
 *
 * The summary (`2 modified`) is `--accent` in the mock. It is the one place that says what
 * the buttons are about to do, so it is also the element the buttons' accessible
 * description points at: a screen reader user pressing Commit hears which files.
 */
import { useId } from 'react'
import styles from './CommitBox.module.css'

export interface CommitBoxProps {
  message: string
  amend: boolean
  /** `2 modified`, already formatted by `summarize`. */
  summary: string
  /** False when nothing is ticked, or when a git call is in flight. */
  canCommit: boolean
  /** Non-null while a command runs; shown in place of the summary. */
  busy: string | null
  /** No HEAD to amend — an unborn branch. The checkbox is disabled rather than hidden. */
  canAmend: boolean
  onMessage: (text: string) => void
  onAmend: (on: boolean) => void
  onCommit: () => void
  onCommitAndPush: () => void
}

export function CommitBox(props: CommitBoxProps) {
  const summaryId = useId()
  const messageId = useId()
  const empty = props.message.trim() === ''
  // An empty message is refused here rather than by git: `git commit` with an empty
  // message aborts, and finding that out after the index has been rewritten is a worse
  // place to learn it.
  const ready = props.canCommit && !empty

  return (
    <div className={styles.footer} data-audit="gitFooter">
      <div className={styles.top}>
        <label className={styles.amend}>
          <input
            type="checkbox"
            className={styles.checkbox}
            checked={props.amend}
            disabled={!props.canAmend}
            onChange={(e) => props.onAmend(e.currentTarget.checked)}
          />
          Amend
        </label>
        <span className={styles.summary} id={summaryId} data-audit="gitSummary">
          {props.busy ?? props.summary}
        </span>
      </div>

      <label className={styles.srOnly} htmlFor={messageId}>
        Commit message
      </label>
      <textarea
        id={messageId}
        className={styles.message}
        data-audit="gitMessage"
        value={props.message}
        placeholder="Commit message"
        spellCheck={true}
        onChange={(e) => props.onMessage(e.currentTarget.value)}
      />

      <div className={styles.actions}>
        <button
          type="button"
          className={styles.commit}
          data-audit="gitCommit"
          disabled={!ready}
          aria-describedby={summaryId}
          onClick={props.onCommit}
        >
          Commit
        </button>
        <button
          type="button"
          className={styles.push}
          disabled={!ready}
          aria-describedby={summaryId}
          onClick={props.onCommitAndPush}
        >
          {/* The ellipsis is the mock's and it is load-bearing: push may need a remote, a
              branch or a credential, so this opens something rather than acting at once. */}
          Commit and Push…
        </button>
      </div>
    </div>
  )
}
