/**
 * "Staging changed outside cide — reload or overwrite?"
 *
 * # What it is for
 *
 * Changelists are the truth in this app: committing rewrites `.git/index` from the ticked
 * changelist (ADR 0004). Bash panes inside cide are exactly where someone will run
 * `git add`, so the collision is likely rather than theoretical. When the index no longer
 * matches what cide last wrote, the choice belongs to the user:
 *
 * * **Reload** — git's view wins. cide's ticks for that repo are dropped and the panel
 *   re-reads status, so what is on screen is what is really staged.
 * * **Overwrite** — cide's view wins. The next commit is sent without an index
 *   expectation, and the changelist is written over whatever `git add` put there.
 *
 * # Why it is not a dialog
 *
 * It is non-blocking on purpose. A modal here would interrupt a commit message someone is
 * mid-way through typing, and the honest default — do nothing until asked — is available
 * only if the warning can be ignored. So it is a bar: the commit button stays live behind
 * it, and answering is optional.
 *
 * The state that raises this bar is one nobody arranges by hand, which is why
 * `?git-story=guard` renders it from a fixture. See `fixture.ts`.
 */
import styles from './GuardBar.module.css'

export interface GuardBarProps {
  /** Absolute roots whose index moved. One bar per repo — they are answered separately. */
  repos: readonly string[]
  /**
   * Whether to name the repository in the bar.
   *
   * Driven by how many repos the workspace has, NOT by how many diverged: with two roots
   * and one divergence the bar must still say which one, and keying it off `repos.length`
   * here would leave exactly that case unlabelled.
   */
  showRepo: boolean
  labelFor: (repo: string) => string
  onReload: (repo: string) => void
  onOverwrite: (repo: string) => void
}

export function GuardBar({ repos, showRepo, labelFor, onReload, onOverwrite }: GuardBarProps) {
  if (repos.length === 0) return null
  return (
    <>
      {repos.map((repo) => (
        <div
          key={repo}
          className={styles.bar}
          data-audit="gitGuard"
          /*
           * `status`, not `alert`: an assertive live region interrupts whatever a screen
           * reader is currently saying, and this can appear while the user is typing a
           * commit message. It is important, not urgent.
           */
          role="status"
        >
          <span className={styles.glyph} aria-hidden="true">
            ⚠
          </span>
          <span className={styles.text}>
            Staging changed outside cide
            {showRepo && <span className={styles.repo}> · {labelFor(repo)}</span>}
          </span>
          <button type="button" className={styles.action} onClick={() => onReload(repo)}>
            Reload
          </button>
          <button type="button" className={styles.action} onClick={() => onOverwrite(repo)}>
            Overwrite
          </button>
        </div>
      ))}
    </>
  )
}
