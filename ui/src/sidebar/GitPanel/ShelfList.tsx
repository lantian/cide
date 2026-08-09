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
 */
import styles from './ShelfList.module.css'
import type { ShelfEntry } from './types'

export interface ShelfListProps {
  entries: readonly ShelfEntry[]
  onUnshelve: (entry: ShelfEntry) => void
}

export function ShelfList({ entries, onUnshelve }: ShelfListProps) {
  if (entries.length === 0) {
    return (
      <div className={styles.list}>
        <p className={styles.empty}>Nothing shelved. Use ⤓ in the toolbar to shelve changes.</p>
      </div>
    )
  }
  return (
    <ul className={styles.list} data-audit="gitShelf">
      {entries.map((entry) => (
        <li key={entry.id}>
          <button type="button" className={styles.entry} onDoubleClick={() => onUnshelve(entry)}>
            <span className={styles.name}>{entry.name}</span>
            <span className={styles.meta}>
              {entry.fileCount} {entry.fileCount === 1 ? 'file' : 'files'}
              {entry.createdAt !== undefined && ` · ${formatWhen(entry.createdAt)}`}
            </span>
          </button>
        </li>
      ))}
    </ul>
  )
}

/** Unix seconds to a short local date-time. Seconds, because that is what git deals in. */
function formatWhen(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}
