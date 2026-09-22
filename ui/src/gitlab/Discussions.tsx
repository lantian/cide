import { Icon } from '@/icons/Icon'
import { closeOverlay } from '@/overlays/store'
import { UserLink } from './UserLink'
import { Markdown } from './Markdown'
import { useEffect, useState } from 'react'
import {
  api,
  drafts,
  refreshDiscussions,
  useGitLab,
  data,
  openDocument,
} from './store'
import type { Discussion, Position } from './types'
import { message, threadResolved } from './model'
import styles from './GitLab.module.css'
export function CommentBox({
  review,
  discussion,
  position,
  onSent,
  label = 'Comment',
}: {
  review: string
  discussion?: string
  position?: Position
  onSent?: () => void
  label?: string
}) {
  const key = `${review}:${discussion ?? JSON.stringify(position ?? 'general')}`
  const [body, setBody] = useState(() => drafts.get(key) ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  async function submit() {
    setBusy(true)
    setError('')
    try {
      await api({
        kind: 'comment',
        review,
        body,
        discussion: discussion ?? null,
        position: position ?? null,
      })
      setBody('')
      drafts.delete(key)
      await refreshDiscussions(review)
      onSent?.()
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      <textarea
        className={styles.comment}
        autoFocus={position !== undefined || discussion !== undefined}
        aria-label={label}
        placeholder={label}
        value={body}
        onChange={(e) => {
          setBody(e.target.value)
          drafts.set(key, e.target.value)
        }}
        disabled={busy}
      />
      <div className={styles.row}>
        <button disabled={busy || !body.trim()}>
          {busy ? 'Posting…' : label}
        </button>
        <span className={styles.muted}>
          Posted immediately · Markdown supported
        </span>
      </div>
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
    </form>
  )
}
export function Thread({
  review,
  thread,
  inline = false,
}: {
  inline?: boolean
  review: string
  thread: Discussion
}) {
  const [reply, setReply] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const resolved = threadResolved(thread)
  const [collapsed, setCollapsed] = useState(resolved)
  useEffect(() => {
    setCollapsed(resolved)
  }, [resolved])
  const first = thread.notes[0]
  if (!first) return null
  const position = first.position
  const current = data.get(review)
  const outdated =
    position &&
    current &&
    (position.head_sha !== current.version.head_commit_sha ||
      position.base_sha !== current.version.base_commit_sha)
  async function resolve() {
    setBusy(true)
    setError('')
    try {
      await api({
        kind: 'resolve',
        review,
        discussion: thread.id,
        resolved: !resolved,
      })
      await refreshDiscussions(review)
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <article
      className={styles.thread}
      data-resolved={resolved}
      data-inline={inline}
      aria-label={`${resolved ? 'Resolved' : 'Open'} discussion by ${first.author.name}`}
    >
      <header className={styles.threadHeader}>
        <button
          className={styles.threadToggle}
          aria-expanded={!collapsed}
          onClick={() => setCollapsed(!collapsed)}
        >
          <Icon name={collapsed ? 'chevron-right' : 'chevron-down'} size={1} />{' '}
          {resolved
            ? 'Resolved thread'
            : first.resolvable
              ? 'Unresolved thread'
              : 'Discussion'}
          <span className={styles.muted}>
            {' '}
            · {thread.notes.length}{' '}
            {thread.notes.length === 1 ? 'comment' : 'comments'}
          </span>
        </button>
        {collapsed && <UserLink user={first.author} review={review} />}
      </header>
      {!collapsed && (
        <>
          {!inline && position && (
            <button
              onClick={() => {
                const change = outdated
                  ? undefined
                  : current?.version.diffs?.find(
                      (c) =>
                        c.new_path === position.new_path ||
                        c.old_path === position.old_path,
                    )
                closeOverlay()
                openDocument({
                  review,
                  path: position.new_path,
                  mode: 'diff',
                  ...(change ? { change } : {}),
                  refs: position,
                  oldPath: position.old_path,
                  at: {
                    line: position.new_line ?? position.old_line ?? 1,
                    column: 1,
                    side: position.new_line == null ? 'old' : 'new',
                  },
                })
              }}
            >
              {position.new_path}:{position.new_line ?? position.old_line}{' '}
              {outdated ? '· Outdated' : ''}
            </button>
          )}
          {thread.notes.map((n) => (
            <div key={n.id} className={styles.note}>
              <header className={styles.noteHeader}>
                <UserLink user={n.author} review={review} />
                <time dateTime={n.created_at}>
                  {new Date(n.created_at).toLocaleString()}
                </time>
                {n.resolvable && (
                  <span
                    className={styles.badge}
                    data-status={n.resolved ? 'success' : 'pending'}
                  >
                    {n.resolved ? 'Resolved' : 'Unresolved'}
                  </span>
                )}
              </header>
              <Markdown
                review={review}
                text={n.body}
                baseUrl={current?.mr.web_url}
              />
            </div>
          ))}
          {!first.system && (
            <div className={styles.row}>
              <button onClick={() => setReply(!reply)}>
                {reply ? 'Hide reply' : 'Reply'}
              </button>
              {first.resolvable && (
                <button disabled={busy} onClick={() => void resolve()}>
                  {resolved ? 'Reopen thread' : 'Resolve thread'}
                </button>
              )}
            </div>
          )}
          {reply && (
            <CommentBox
              review={review}
              discussion={thread.id}
              label="Reply"
              onSent={() => setReply(false)}
            />
          )}
          {error && (
            <div role="alert" className={styles.error}>
              {error}
            </div>
          )}
        </>
      )}
    </article>
  )
}
export function Discussions({ review }: { review: string }) {
  useGitLab()
  const d = data.get(review)
  const [unresolved, setUnresolved] = useState(false)
  return (
    <div className={styles.discussions}>
      <label>
        <input
          type="checkbox"
          checked={unresolved}
          onChange={(e) => setUnresolved(e.target.checked)}
        />
        Unresolved threads only
      </label>
      {d?.discussions
        .filter(
          (t) =>
            t.notes.some((n) => !n.system) &&
            (!unresolved || t.notes.some((n) => n.resolvable && !n.resolved)),
        )
        .map((t) => (
          <Thread key={t.id} review={review} thread={t} />
        ))}
      <CommentBox review={review} />
    </div>
  )
}
