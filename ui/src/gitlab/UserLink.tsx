import { gitlab } from '@/ipc/client'
import type { User } from './types'
import { useGitLab } from './store'
import styles from './GitLab.module.css'

export function UserLink({
  user,
  review,
  host,
}: {
  user: User
  review?: string | undefined
  host?: string | undefined
}) {
  const { board } = useGitLab()
  const account = board.reviews.find((r) => r.id === review)?.account
  const base = host ?? board.accounts.find((a) => a.id === account)?.host
  const url =
    user.web_url ??
    (base
      ? `${base.replace(/\/$/, '')}/${encodeURIComponent(user.username)}`
      : undefined)
  if (!url || !url.startsWith('https://'))
    return <span>{user.name || user.username}</span>
  return (
    <a
      className={styles.userLink}
      href={url}
      title={`@${user.username}`}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        void gitlab.openUrl(url)
      }}
    >
      {user.name || user.username}
    </a>
  )
}

export function Users({
  users,
  review,
}: {
  users: readonly User[]
  review: string
}) {
  return users.length ? (
    <span className={styles.users}>
      {users.map((user) => (
        <UserLink key={user.id} user={user} review={review} />
      ))}
    </span>
  ) : (
    <span className={styles.muted}>None</span>
  )
}
