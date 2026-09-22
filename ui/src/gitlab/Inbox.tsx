import { UserLink } from './UserLink'
import { useEffect, useState } from 'react'
import { gitlab } from '@/ipc/client'
import { openReview, startGitLab, useGitLab } from './store'
import { message } from './model'
import type { MR } from './types'
import { Icon } from '@/icons/Icon'
import { showOverlay } from '@/overlays/store'
import { MRState } from './ReviewStatus'
import chrome from './ReviewChrome.module.css'
import styles from './GitLab.module.css'
export function GitLabInbox() {
  const { board, error: connectionError } = useGitLab()
  const [account, setAccount] = useState('')
  const [scope, setScope] = useState('review')
  const [state, setState] = useState('opened')
  const [search, setSearch] = useState('')
  const [project, setProject] = useState('')
  const [rows, setRows] = useState<{ mr: MR; account: string }[]>([])
  const [page, setPage] = useState(1)
  const [more, setMore] = useState(false)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [refresh, setRefresh] = useState(0)
  const [opening, setOpening] = useState<string | null>(null)
  useEffect(() => startGitLab(), [])
  useEffect(() => {
    setPage(1)
    setRows([])
  }, [account, scope, state, search, project])
  useEffect(() => {
    let current = true
    setBusy(true)
    const timer = setTimeout(() => {
      setError('')
      const accounts = board.accounts.filter(
        (a) => !account || a.id === account,
      )
      void Promise.allSettled(
        accounts.map(async (a) => {
          const result = await gitlab.request({
            kind: 'list',
            account: a.id,
            scope,
            state,
            search,
            project: project || null,
            page,
          })
          return {
            rows: (result.data as unknown as MR[]).map((mr) => ({
              mr,
              account: a.id,
            })),
            more: result.nextPage !== null,
          }
        }),
      ).then((results) => {
        if (!current) return
        const successes = results.flatMap((r) =>
          r.status === 'fulfilled' ? [r.value] : [],
        )
        setRows((old) =>
          page === 1
            ? successes.flatMap((r) => r.rows)
            : [...old, ...successes.flatMap((r) => r.rows)].filter(
                (r, i, all) =>
                  all.findIndex(
                    (v) => v.account === r.account && v.mr.id === r.mr.id,
                  ) === i,
              ),
        )
        setMore(successes.some((r) => r.more))
        setError(
          results
            .filter((r) => r.status === 'rejected')
            .map((r) => message(r.reason))
            .join('\n'),
        )
        setBusy(false)
      })
    }, 250)
    return () => {
      current = false
      clearTimeout(timer)
    }
  }, [account, scope, state, search, project, page, refresh, board.accounts])
  async function open(a: string, link: string) {
    if (opening) return
    setError('')
    setOpening(link)
    try {
      await openReview(a, link)
    } catch (e) {
      setError(message(e))
    } finally {
      setOpening(null)
    }
  }
  const sorted = [...rows].sort(
    (a, b) =>
      (Date.parse(b.mr.updated_at) || 0) - (Date.parse(a.mr.updated_at) || 0),
  )
  return (
    <section className={chrome.inbox} aria-label="GitLab merge requests">
      <header className={chrome.panelHeader}>
        <h2>Merge requests</h2>
        <button
          className={chrome.iconButton}
          title="Refresh merge requests"
          aria-label="Refresh merge requests"
          disabled={busy}
          onClick={() => {
            setPage(1)
            setRefresh((n) => n + 1)
          }}
        >
          <Icon name="refresh-cw" />
        </button>
        <button
          className={chrome.iconButton}
          title="Open merge request by URL"
          aria-label="Open merge request by URL"
          onClick={() => showOverlay('gitlabOpen')}
        >
          <Icon name="link" />
        </button>
      </header>
      <div className={chrome.inboxFilters}>
        <div
          className={chrome.scopeTabs}
          role="group"
          aria-label="MR relationship"
        >
          {(
            [
              ['review', 'To review', 'Review requested from me'],
              ['created', 'Created', 'Created by me'],
              ['assigned', 'Assigned', 'Assigned to me'],
            ] as const
          ).map(([value, label, title]) => (
            <button
              key={value}
              aria-pressed={scope === value}
              aria-label={title}
              title={title}
              onClick={() => setScope(value)}
            >
              {label}
            </button>
          ))}
        </div>
        <label className={chrome.searchField}>
          <Icon name="search" size={1} />
          <input
            aria-label="Search merge requests"
            placeholder="Search merge requests…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </label>
        <div className={chrome.filterRow}>
          <select
            aria-label="MR state"
            value={state}
            onChange={(e) => setState(e.target.value)}
          >
            <option value="opened">Open</option>
            <option value="merged">Merged</option>
            <option value="closed">Closed</option>
            <option value="all">All states</option>
          </select>
          <input
            aria-label="Filter project"
            title="Filter by group/project"
            placeholder="All projects"
            value={project}
            onChange={(e) => setProject(e.target.value)}
          />
        </div>
        {board.accounts.length > 1 && (
          <select
            aria-label="GitLab account"
            value={account}
            onChange={(e) => setAccount(e.target.value)}
          >
            <option value="">All accounts</option>
            {board.accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.username} · {a.host}
              </option>
            ))}
          </select>
        )}
      </div>
      {(error || connectionError) && (
        <div role="alert" className={`${styles.error} ${chrome.inboxError}`}>
          {error || connectionError}
        </div>
      )}
      <div className={chrome.listCaption}>
        <span>
          {scope === 'review'
            ? 'Review requested from you'
            : scope === 'created'
              ? 'Created by you'
              : 'Assigned to you'}
        </span>
        <span>{busy ? 'Loading…' : `${rows.length}${more ? '+' : ''}`}</span>
      </div>
      <div
        className={chrome.inboxList}
        aria-label="Merge requests"
        aria-busy={busy}
      >
        {sorted.map(({ mr, account: a }) => {
          const owner = board.accounts.find((item) => item.id === a)
          const opened = board.reviews.some(
            (r) => r.account === a && r.url === mr.web_url,
          )
          const full = mr.references?.full
          const projectLabel = full?.split('!')[0] || projectName(mr.web_url)
          return (
            <article
              key={`${a}:${mr.id}`}
              className={chrome.mrItem}
              data-open={opened}
            >
              <button
                className={chrome.mrOpen}
                disabled={opening !== null}
                onClick={() => void open(a, mr.web_url)}
                aria-label={`Open merge request !${mr.iid}: ${mr.title}`}
                title={mr.title}
              >
                <span className={chrome.itemTop}>
                  <MRState state={mr.state} draft={mr.draft} />
                  <span className={chrome.mrNumber}>!{mr.iid}</span>
                  {opened && (
                    <span className={chrome.openedLabel}>In review</span>
                  )}
                </span>
                <strong className={chrome.mrTitle}>{mr.title}</strong>
                <span className={chrome.projectName} title={projectLabel}>
                  {projectLabel}
                </span>
                {mr.source_branch && (
                  <span
                    className={chrome.branchPreview}
                    title={`${mr.source_branch} → ${mr.target_branch}`}
                  >
                    <Icon name="git-branch" size={1} />
                    {mr.source_branch}
                    <span>→ {mr.target_branch}</span>
                  </span>
                )}
              </button>
              <div className={chrome.itemFooter}>
                <UserLink user={mr.author} host={owner?.host} />
                <span className={chrome.itemSignals}>
                  {mr.user_notes_count > 0 && (
                    <span title={`${mr.user_notes_count} comments`}>
                      <Icon name="message-square" size={1} />
                      {mr.user_notes_count}
                    </span>
                  )}
                  <time
                    dateTime={mr.updated_at}
                    title={new Date(mr.updated_at).toLocaleString()}
                  >
                    {updatedLabel(mr.updated_at)}
                  </time>
                </span>
              </div>
              {opening === mr.web_url && (
                <span role="status" className={chrome.opening}>
                  Opening review…
                </span>
              )}
              {board.accounts.length > 1 && owner && (
                <div className={chrome.accountLabel}>
                  Account{' '}
                  <UserLink
                    user={{
                      id: owner.userId,
                      username: owner.username,
                      name: owner.username,
                    }}
                    host={owner.host}
                  />{' '}
                  · {owner.host.replace(/^https?:\/\//, '')}
                </div>
              )}
            </article>
          )
        })}
        {!rows.length && (
          <div className={chrome.empty} role="status">
            <Icon name={busy ? 'refresh-cw' : 'git-branch'} size={3} />
            <strong>
              {!board.accounts.length
                ? 'Connect your GitLab account'
                : busy
                  ? 'Loading merge requests…'
                  : error
                    ? 'Could not load merge requests'
                    : 'No merge requests here'}
            </strong>
            <p>
              {!board.accounts.length
                ? 'Add an account in Settings → Git → GitLab accounts.'
                : busy
                  ? 'Checking your GitLab projects.'
                  : error
                    ? 'Use Refresh to try again.'
                    : 'Try another view or adjust your search and filters.'}
            </p>
          </div>
        )}
        {more && (
          <button
            className={chrome.loadMore}
            disabled={busy}
            onClick={() => setPage((p) => p + 1)}
          >
            {busy ? 'Loading…' : 'Load more merge requests'}
          </button>
        )}
      </div>
      <footer className={chrome.inboxFooter}>
        <span>Updated most recently first</span>
        <button onClick={() => showOverlay('gitlabOpen')}>
          Open by URL <Icon name="arrow-up-right" size={1} />
        </button>
      </footer>
    </section>
  )
}

function projectName(url: string): string {
  try {
    return decodeURIComponent(
      new URL(url).pathname.split('/-/merge_requests/')[0]!.replace(/^\//, ''),
    )
  } catch {
    return 'GitLab project'
  }
}
function updatedLabel(value: string): string {
  const date = Date.parse(value)
  if (!Number.isFinite(date)) return ''
  const minutes = Math.max(0, Math.floor((Date.now() - date) / 60000))
  if (minutes < 1) return 'now'
  if (minutes < 60) return `${minutes}m ago`
  if (minutes < 1440) return `${Math.floor(minutes / 60)}h ago`
  if (minutes < 10080) return `${Math.floor(minutes / 1440)}d ago`
  return new Date(date).toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
  })
}
