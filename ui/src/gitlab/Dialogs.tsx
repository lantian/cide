import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { closeOverlay, useOverlayOpen } from '@/overlays/store'
import { data, openReview, showInbox, showReviewInfo, useGitLab } from './store'
import { gitlab } from '@/ipc/client'
import type { GitLabReviewHarness, Harness } from '@/ipc/generated'
import { Drafts } from './Drafts'
import { HARNESS_LABEL, startAgentReview } from './agentReview'
import { message } from './model'
import { Markdown } from './Markdown'
import { UserLink, Users } from './UserLink'
import { Discussions } from './Discussions'
import { Activity } from './Activity'
import { Pipelines } from './Pipelines'
import styles from './GitLab.module.css'

function modalKeys(event: KeyboardEvent<HTMLElement>) {
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    closeOverlay()
  }
  if (event.key !== 'Tab') return
  const controls = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href]',
    ),
  )
  const first = controls[0],
    last = controls.at(-1)
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last?.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first?.focus()
  }
}

export function OpenReviewDialog() {
  const { board } = useGitLab()
  const [url, setUrl] = useState('')
  const [account, setAccount] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const input = useRef<HTMLInputElement>(null)
  useLayoutEffect(() => {
    input.current?.focus()
  }, [])
  const matches = board.accounts.filter((a) => {
    try {
      const host = new URL(a.host),
        link = new URL(url.trim())
      return (
        host.origin === link.origin &&
        link.pathname.startsWith(host.pathname.replace(/\/$/, '') + '/')
      )
    } catch {
      return false
    }
  })
  const selected =
    matches.find((a) => a.id === account) ??
    (matches.length === 1 ? matches[0] : undefined)
  async function submit() {
    if (busy) return
    if (!selected) {
      setError(
        matches.length
          ? 'Choose the account for this MR.'
          : 'Add an account for this GitLab URL in Settings → Git → GitLab accounts.',
      )
      return
    }
    setBusy(true)
    setError('')
    try {
      await openReview(selected.id, url.trim())
      closeOverlay()
    } catch (e) {
      setError(message(e))
      setBusy(false)
      input.current?.focus()
    }
  }
  return (
    <OverlayCard label="Open GitLab merge request" onDismiss={closeOverlay}>
      <form
        className={styles.openDialog}
        onKeyDown={modalKeys}
        onSubmit={(event) => {
          event.preventDefault()
          void submit()
        }}
      >
        <h2>Open GitLab merge request</h2>
        <label htmlFor="gitlab-mr-url">Merge request URL</label>
        <input
          ref={input}
          id="gitlab-mr-url"
          type="url"
          required
          autoComplete="off"
          spellCheck={false}
          value={url}
          onChange={(e) => {
            setUrl(e.target.value)
            setError('')
          }}
          placeholder="https://gitlab.example/group/project/-/merge_requests/42"
        />
        {matches.length > 1 && (
          <select
            aria-label="GitLab account"
            value={account}
            onChange={(e) => setAccount(e.target.value)}
          >
            <option value="">Choose an account</option>
            {matches.map((a) => (
              <option key={a.id} value={a.id}>
                {a.username} · {a.host}
              </option>
            ))}
          </select>
        )}
        {error && (
          <div role="alert" className={styles.error}>
            {error}
          </div>
        )}
        <div className={styles.dialogFooter}>
          <button
            type="button"
            onClick={() => {
              closeOverlay()
              showInbox()
            }}
          >
            Open inbox
          </button>
          <button type="button" onClick={closeOverlay}>
            Cancel
          </button>
          <button
            className={styles.primary}
            type="submit"
            disabled={busy || !url.trim()}
          >
            {busy ? 'Opening…' : 'Open MR'}
          </button>
        </div>
      </form>
    </OverlayCard>
  )
}

export function ReviewInfoDialog() {
  const { info, board } = useGitLab()
  const d = info ? data.get(info.review) : undefined
  const close = useRef<HTMLButtonElement>(null)
  useLayoutEffect(() => {
    close.current?.focus()
  }, [])
  useEffect(() => {
    if (info && !d && !board.reviews.some((r) => r.id === info.review))
      closeOverlay()
  }, [info, d, board])
  if (!info || !d) return null
  const { review, section } = info
  let body: ReactNode
  if (section === 'discussions') body = <Discussions review={review} />
  else if (section === 'drafts') body = <Drafts review={review} />
  else if (section === 'activity')
    body = (
      <div className={styles.discussions}>
        <Activity review={review} />
      </div>
    )
  else if (section === 'pipelines') body = <Pipelines review={review} />
  else
    body = (
      <div className={styles.details}>
        <dl className={styles.metadata}>
          <dt>Author</dt>
          <dd>
            <UserLink user={d.mr.author} review={review} />
          </dd>
          <dt>Assignees</dt>
          <dd>
            <Users users={d.mr.assignees} review={review} />
          </dd>
          <dt>Reviewers</dt>
          <dd>
            <Users users={d.mr.reviewers} review={review} />
          </dd>
          <dt>Approved by</dt>
          <dd>
            <Users
              users={d.approval?.approved_by.map((a) => a.user) ?? []}
              review={review}
            />
          </dd>
          <dt>State</dt>
          <dd>{d.mr.state}</dd>
          <dt>Branches</dt>
          <dd>
            {d.mr.source_branch} → {d.mr.target_branch}
          </dd>
        </dl>
        <Markdown
          text={d.mr.description || 'No description.'}
          review={review}
          baseUrl={d.mr.web_url}
        />
      </div>
    )
  return (
    <OverlayCard label={`!${d.mr.iid} ${section}`} onDismiss={closeOverlay}>
      <section
        className={`${styles.infoModal} ${styles.review}`}
        onKeyDown={modalKeys}
      >
        <header className={styles.bar}>
          <strong>
            !{d.mr.iid} {d.mr.title}
          </strong>
          <button
            ref={close}
            className={styles.modalClose}
            onClick={closeOverlay}
            aria-label="Close MR information"
          >
            ×
          </button>
        </header>
        <nav className={styles.bar} aria-label="MR information sections">
          {(
            [
              'description',
              'discussions',
              'drafts',
              'activity',
              'pipelines',
            ] as const
          ).map((s) => (
            <button
              key={s}
              aria-pressed={section === s}
              onClick={() => showReviewInfo(review, s)}
            >
              {s === 'description'
                ? 'Overview'
                : s[0]!.toUpperCase() + s.slice(1)}
            </button>
          ))}
        </nav>
        {body}
      </section>
    </OverlayCard>
  )
}

/**
 * Start an agent reviewing the MR: pick a harness, optionally say what to focus on. (M85)
 *
 * The agent's findings arrive as local drafts; this dialog says so, because "review" next to a
 * GitLab MR otherwise reads as something that posts.
 */
export function LaunchReviewDialog() {
  const { launch } = useGitLab()
  const d = launch ? data.get(launch) : undefined
  const [harnesses, setHarnesses] = useState<GitLabReviewHarness[] | null>(
    null,
  )
  const [harness, setHarness] = useState<Harness | ''>('')
  const [prompt, setPrompt] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  useEffect(() => {
    let alive = true
    gitlab.reviewHarnesses().then(
      (list) => {
        if (!alive) return
        setHarnesses(list)
        setHarness((old) => old || (list.find((h) => !h.unavailable)?.harness ?? ''))
      },
      (e) => alive && setError(message(e)),
    )
    return () => {
      alive = false
    }
  }, [])
  if (!launch || !d) return null
  const chosen = harnesses?.find((h) => h.harness === harness)
  async function submit() {
    if (busy || !harness || !launch) return
    setBusy(true)
    setError('')
    try {
      await startAgentReview(launch, harness, prompt)
      closeOverlay()
    } catch (e) {
      setError(message(e))
      setBusy(false)
    }
  }
  return (
    <OverlayCard label="Review with an agent" onDismiss={closeOverlay}>
      <form
        className={`${styles.openDialog} ${styles.launch}`}
        onKeyDown={modalKeys}
        onSubmit={(event) => {
          event.preventDefault()
          void submit()
        }}
      >
        <h2>
          Review !{d.mr.iid} with an agent
        </h2>
        <p className={styles.muted}>
          The agent reads the MR and the project at the MR&apos;s head, and
          writes its findings as <strong>draft</strong> comments with a
          severity. Nothing is posted to GitLab until you publish a draft.
        </p>
        <label htmlFor="gitlab-review-harness">Harness</label>
        <select
          id="gitlab-review-harness"
          value={harness}
          disabled={!harnesses}
          onChange={(e) => setHarness(e.target.value as Harness)}
        >
          {!harnesses && <option value="">Checking installed harnesses…</option>}
          {harnesses?.map((h) => (
            <option key={h.harness} value={h.harness} disabled={!!h.unavailable}>
              {HARNESS_LABEL[h.harness]}
              {h.unavailable ? ' — unavailable' : ''}
            </option>
          ))}
        </select>
        {chosen?.unavailable && (
          <span className={styles.muted}>{chosen.unavailable}</span>
        )}
        <label htmlFor="gitlab-review-prompt">
          Instructions <span className={styles.muted}>(optional)</span>
        </label>
        <textarea
          id="gitlab-review-prompt"
          className={styles.comment}
          placeholder="What to focus on — e.g. concurrency in the new cache, or the migration's rollback."
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
        />
        {error && (
          <div role="alert" className={styles.error}>
            {error}
          </div>
        )}
        <div className={styles.dialogFooter}>
          <button type="button" onClick={closeOverlay}>
            Cancel
          </button>
          <button
            className={styles.primary}
            type="submit"
            disabled={busy || !harness || !!chosen?.unavailable}
          >
            {busy ? 'Preparing checkout…' : 'Start review'}
          </button>
        </div>
      </form>
    </OverlayCard>
  )
}

export function GitLabDialogs() {
  const open = useOverlayOpen()
  return open === 'gitlabOpen' ? (
    <OpenReviewDialog />
  ) : open === 'gitlabInfo' ? (
    <ReviewInfoDialog />
  ) : open === 'gitlabLaunch' ? (
    <LaunchReviewDialog />
  ) : null
}
