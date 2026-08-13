/**
 * The footer: `Amend`, the selection summary, the 104px message box, and the two buttons.
 *
 * Ticking `Amend` does **not** prefill the message box. It used to say here that it did, from
 * a `headMessage` field the panel had invented; `cide_ipc::git::RepoChanges` carries no such
 * thing, and reading it per repo on every refresh would be a round trip for a string only ever
 * used when one checkbox is ticked. `git commit --amend` keeps HEAD's message when none is
 * given, so an empty box amends without rewriting the subject. See `useGitPanel::setAmend`.
 *
 * The summary (`2 modified`) is `--accent` in the mock. It is the one place that says what
 * the buttons are about to do, so it is also the element the buttons' accessible
 * description points at: a screen reader user pressing Commit hears which files.
 */
import { useEffect, useId, useRef } from 'react'
import { clearFocusRequest, useFocusRequested } from '@/chrome/focusRequests'
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

  /*
   * The caret, when *Commit changes…* asked for it.
   *
   * That command does not commit — the message and the ticked paths are this panel's own React
   * state and do not exist while the sidebar is elsewhere, and a palette row that committed
   * whatever happened to be ticked would be a foot-gun in a build with no revert surface. What
   * it does instead is reveal this panel and ask for this box, which is the part a user can
   * finish. `commands.rs` carries the full argument.
   *
   * The request is parked in a store rather than pushed as a prop because the panel usually
   * mounts one render *after* the command runs; `chrome/focusRequests.ts` explains the
   * handshake and why an `autoFocus` cannot do it. No `select()`, unlike the search box: a
   * half-typed commit message is work, and selecting it would put one keystroke between the
   * user and losing it.
   */
  const message = useRef<HTMLTextAreaElement>(null)
  const focusWanted = useFocusRequested('commitMessage')
  useEffect(() => {
    if (!focusWanted) return
    message.current?.focus()
    clearFocusRequest('commitMessage')
  }, [focusWanted])

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
        ref={message}
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
