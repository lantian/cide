import { closeOverlay } from '@/overlays/store'
import { data, useGitLab, openDocument } from './store'
import { UserLink } from './UserLink'
import { Markdown } from './Markdown'
import styles from './GitLab.module.css'

export function Activity({ review }: { review: string }) {
  useGitLab()
  const d = data.get(review)
  const merged = new Map(d?.activity.map((note) => [note.id, note]))
  for (const thread of d?.discussions ?? [])
    for (const note of thread.notes) merged.set(note.id, note)
  const notes = Array.from(merged.values()).sort(
    (a, b) => Date.parse(a.created_at) - Date.parse(b.created_at),
  )
  return (
    <ol className={styles.activity} aria-label="MR activity and comments">
      {notes.map((note) => (
        <li key={note.id} data-system={note.system}>
          <header className={styles.noteHeader}>
            <UserLink user={note.author} review={review} />
            <span className={styles.muted}>
              {note.system ? 'Activity' : 'Comment'}
            </span>
            <time dateTime={note.created_at} title={note.created_at}>
              {new Date(note.created_at).toLocaleString()}
            </time>
          </header>
          {note.position && (
            <button
              className={styles.activityLocation}
              onClick={() => {
                const position = note.position!
                closeOverlay()
                openDocument({
                  review,
                  path: position.new_path,
                  oldPath: position.old_path,
                  mode: 'diff',
                  refs: position,
                  at: {
                    line: position.new_line ?? position.old_line ?? 1,
                    column: 1,
                    side: position.new_line == null ? 'old' : 'new',
                  },
                })
              }}
            >
              {note.position.new_path}:
              {note.position.new_line ?? note.position.old_line}
            </button>
          )}
          <Markdown review={review} text={note.body} baseUrl={d?.mr.web_url} />
        </li>
      ))}
      {!notes.length && <li className={styles.muted}>No activity yet.</li>}
    </ol>
  )
}
