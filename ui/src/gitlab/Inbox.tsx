import { UserLink } from './UserLink'
import { IconButton } from '@/kit/components/Button'
import { Segmented } from '@/kit/components/Choice'
import { SearchField, TextInput } from '@/kit/components/Field'
import { Select } from '@/kit/components/Select'
import { PanelHeader } from '@/kit/components/Surface'
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
      <PanelHeader
        title="Merge requests"
        tools={
          <>
            <IconButton
              icon="refresh-cw"
              label="Refresh merge requests"
              disabled={busy}
              onClick={() => {
                setPage(1)
                setRefresh((n) => n + 1)
              }}
            />
            <IconButton
              icon="link"
              label="Open merge request by URL"
              onClick={() => showOverlay('gitlabOpen')}
            />
          </>
        }
      />
      <div className={chrome.inboxFilters}>
        <Segmented
          label="MR relationship"
          block
          size="sm"
          value={scope}
          onChange={setScope}
          options={[
            { value: 'review', label: 'To review' },
            { value: 'created', label: 'Created' },
            { value: 'assigned', label: 'Assigned' },
          ]}
        />
        <SearchField
          size="sm"
          aria-label="Search merge requests"
          placeholder="Search merge requests…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <div className={chrome.filterRow}>
          <div className={chrome.filterState}>
            <Select
              size="sm"
              aria-label="MR state"
              value={state}
              onChange={setState}
              options={[
                { value: 'opened', label: 'Open' },
                { value: 'merged', label: 'Merged' },
                { value: 'closed', label: 'Closed' },
                { value: 'all', label: 'All states' },
              ]}
            />
          </div>
          <div className={chrome.filterProject}>
            <TextInput
              size="sm"
              aria-label="Filter project"
              title="Filter by group/project"
              placeholder="All projects"
              value={project}
              onChange={(e) => setProject(e.target.value)}
            />
          </div>
        </div>
        {board.accounts.length > 1 && (
          <Select
            size="sm"
            aria-label="GitLab account"
            value={account}
            onChange={setAccount}
            options={[
              { value: '', label: 'All accounts' },
              ...board.accounts.map((a) => ({ value: a.id, label: `${a.username} · ${a.host}` })),
            ]}
          />
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
