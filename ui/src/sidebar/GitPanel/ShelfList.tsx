/**
 * The `Shelf` tab: our own patches, listed newest first by the backend.
 *
 * The shelf is deliberately not the stash — `git stash` is a separate concept with its own
 * commands and its own tab later. A shelf entry is a patch cide wrote, so it is listed
 * here with the two facts that identify it: what it was called and how much is in it.
 *
 * Timestamps are rendered with the platform's locale formatter rather than a hand-rolled
 * "3 days ago". A relative string has to be recomputed to stay true, and a shelf list that
 * silently goes stale is worse than a date that is always right.
 *
 * The shelf is per repository and this tab is not, so each row names the repo it came from
 * once there is more than one — and carries its `RepoId`, because `git_unshelve` takes one.
 */
import styles from './ShelfList.module.css'
import type { RepoId, ShelfRow } from './types'

export interface ShelfListProps {
  entries: readonly ShelfRow[]
  /** Whether to name the repository on each row. Driven by the workspace, not by the list. */
  showRepo: boolean
  labelFor: (repo: RepoId) => string
  onUnshelve: (row: ShelfRow) => void
}

export function ShelfList({ entries, showRepo, labelFor, onUnshelve }: ShelfListProps) {
  if (entries.length === 0) {
    return (
      <div className={styles.list}>
        <p className={styles.empty}>Nothing shelved. Use ⤓ in the toolbar to shelve changes.</p>
      </div>
    )
  }
  return (
    <ul className={styles.list} data-audit="gitShelf">
      {entries.map((row) => {
        const files = row.entry.files.length
        return (
          <li key={row.key}>
            <button type="button" className={styles.entry} onDoubleClick={() => onUnshelve(row)}>
              <span className={styles.name}>{row.entry.name}</span>
              <span className={styles.meta}>
                {files} {files === 1 ? 'file' : 'files'}
                {` · ${formatWhen(row.entry.created)}`}
                {showRepo && ` · ${labelFor(row.repo)}`}
              </span>
            </button>
          </li>
        )
      })}
    </ul>
  )
}

/**
 * Unix seconds to a short local date-time.
 *
 * The parameter is typed `bigint` because `ShelfEntry.created` is an `i64` and ts-rs spells
 * that `bigint` — but Tauri's JSON transport delivers a plain `number`, so the value that
 * actually arrives is neither reliably one nor the other. `Number(…)` accepts both; `Date`
 * accepts neither a bigint nor a bigint multiplied by a number, and would throw inside render.
 */
function formatWhen(unixSeconds: bigint): string {
  return new Date(Number(unixSeconds) * 1000).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}
