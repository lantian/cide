import { Icon, type IconName } from '@/icons/Icon'
import { Button, IconButton } from '@/kit/components/Button'
import { Banner, Note, Spinner } from '@/kit/components/Feedback'
import { Counter } from '@/kit/components/Status'
import { ApprovalStatus, MRState } from './ReviewStatus'
import chrome from './ReviewChrome.module.css'
import { Users } from './UserLink'
import { FileTree } from './FileTree'
import { useEffect, useState } from 'react'
import { gitlab } from '@/ipc/client'
import {
  api,
  data,
  loadReview,
  openDocument,
  prepareSource,
  refreshDiscussions,
  refreshApprovals,
  revealed,
  touch,
  useGitLab,
  closeReview,
  showReviewInfo,
  type ReviewSection,
  showLaunchReview,
} from './store'
import {
  agentReviews,
  HARNESS_LABEL,
  showAgentReview,
  stopAgentReview,
} from './agentReview'
import {
  changeTotals,
  message,
  versionRefs,
  visibleChanges,
  pathVisibility,
  fileDiscussions,
} from './model'
import type { MR } from './types'
import styles from './GitLab.module.css'
export function ReviewPanel({
  review,
  theme = 'dark',
}: {
  review: string
  theme?: 'light' | 'dark'
}) {
  const snap = useGitLab()
  const d = data.get(review)
  const showExcluded = revealed.has(review)
  const setShowExcluded = (show: boolean) => {
    if (show) revealed.add(review)
    else revealed.delete(review)
    touch()
  }
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [updated, setUpdated] = useState(false)
  const [source, setSource] = useState(false)
  const [files, setFiles] = useState<string[]>([])
  const [query, setQuery] = useState('')
  useEffect(() => {
    let current = true
    void loadReview(review).catch((e) => {
      if (current) setError(message(e))
    })
    return () => {
      current = false
    }
  }, [review])
  useEffect(() => {
    let current = true
    let timer: ReturnType<typeof setTimeout>
    async function check() {
      try {
        const [mr] = await Promise.all([
          api<MR>({ kind: 'detail', review }),
          refreshDiscussions(review),
          refreshApprovals(review),
        ])
        if (current) {
          const head = mr.diff_refs?.head_sha ?? mr.sha
          setUpdated(Boolean(d && head && head !== d.version.head_commit_sha))
          const old = data.get(review)
          if (old) {
            data.set(review, { ...old, mr })
            touch()
          }
        }
      } catch {
        /* Explicit Refresh reports connectivity failures. */
      }
      if (current) timer = setTimeout(() => void check(), 30000)
    }
    timer = setTimeout(() => void check(), 30000)
    return () => {
      current = false
      clearTimeout(timer)
    }
  }, [review, d?.version.id])
  async function run(work: () => Promise<unknown>) {
    setBusy(true)
    setError('')
    try {
      await work()
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  if (!d)
    return (
      <div className={styles.reviewSidebar}>
        <header className={styles.sidebarHeader}>
          <strong>Merge request</strong>
          <button
            aria-label="Close MR review"
            disabled={busy}
            onClick={() => void run(() => closeReview(review))}
          >
            ×
          </button>
        </header>
        <p>Loading merge request…</p>
        {error && (
          <div role="alert" className={styles.error}>
            {error}
          </div>
        )}
        <button onClick={() => void run(() => loadReview(review, true))}>
          Retry
        </button>
      </div>
    )
  const all = d.version.diffs ?? []
  const visible = visibleChanges(
    all,
    snap.board.preferences.excludedFiles,
    snap.board.preferences.excludeEnabled && !showExcluded,
  )
  const totals = changeTotals(visible)
  const refs = versionRefs(d.version)
  const incomplete =
    !totals.complete ||
    d.version.overflow ||
    Number(d.version.real_size ?? all.length) > all.length
  async function approve() {
    await api({
      kind: 'approve',
      review,
      sha: refs.head_sha,
      undo: !!d?.approval?.user_has_approved,
    })
    await refreshApprovals(review)
  }
  const agent = agentReviews.get(review)
  const draftsOf = (path: string, oldPath = path) =>
    d.drafts.filter((x) => x.path === path || x.path === oldPath).length
  // Counts are a kit `Counter` beside the label rather than "(3)" in it, so the three counted
  // sections read as one system. Commits sits after Pipelines, which in the two-column grid puts
  // it beside Pipelines and under Activity. A count GitLab could not give is left off, not "0".
  const sections: {
    section: ReviewSection
    label: string
    icon: IconName
    count?: number | undefined
  }[] = [
    // Two columns, filled row by row: Activity sits left of Drafts so that every button with a
    // counter (Discussions, Drafts, Commits) is in the right-hand column and the badges line up.
    { section: 'description', label: 'Overview', icon: 'file-text' },
    {
      section: 'discussions',
      label: 'Discussions',
      icon: 'message-square',
      count: d.discussions.filter((t) =>
        t.notes.some((n) => n.resolvable && !n.resolved),
      ).length,
    },
    { section: 'activity', label: 'Activity', icon: 'list' },
    { section: 'drafts', label: 'Drafts', icon: 'pencil', count: d.drafts.length },
    { section: 'pipelines', label: 'Pipelines', icon: 'play' },
    {
      section: 'commits',
      label: 'Commits',
      icon: 'git-commit-horizontal',
      count: d.commits?.length,
    },
  ]

  return (
    <section
      className={`${styles.reviewSidebar} ${chrome.reviewPanel}`}
      aria-label={`Review ${d.mr.title}`}
    >
      <div className={chrome.reviewHeading}>
        <header className={chrome.reviewTopline}>
          <span className={chrome.mrNumber}>!{d.mr.iid}</span>
          <MRState state={d.mr.state} draft={d.mr.draft} />
          <div className={chrome.headerTools}>
            <IconButton
              icon="refresh-cw"
              label="Refresh merge request"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await loadReview(review, true)
                  setUpdated(false)
                  setSource(false)
                  setFiles([])
                })
              }
            />
            <IconButton
              icon="eye"
              label="Review with an agent"
              disabled={busy}
              onClick={() => showLaunchReview(review)}
            />
            <IconButton
              icon="arrow-up-right"
              label="Open in GitLab"
              onClick={() => void gitlab.openUrl(d.mr.web_url)}
            />
            <IconButton
              icon="x"
              label="Close MR review"
              disabled={busy}
              onClick={() => void run(() => closeReview(review))}
            />
          </div>
        </header>
        <h2 className={chrome.reviewTitle}>{d.mr.title}</h2>
        <div
          className={chrome.branches}
          title={`${d.mr.source_branch} → ${d.mr.target_branch}`}
        >
          <Icon name="git-branch" size={1} />
          <code>{d.mr.source_branch}</code>
          <Icon name="arrow-right" size={1} />
          <code>{d.mr.target_branch}</code>
        </div>
        <div className={chrome.assignees}>
          <span>Assignees: </span>
          <Users users={d.mr.assignees} review={review} />
        </div>
        <ApprovalStatus
          approval={d.approval}
          stale={updated}
          error={d.approvalError}
        />
        <button
          className={chrome.approvalAction}
          disabled={
            busy ||
            updated ||
            d.mr.state !== 'opened' ||
            d.approval === null ||
            (d.approval.user_can_approve === false &&
              !d.approval.user_has_approved)
          }
          data-approved={!!d.approval?.user_has_approved}
          title={d.approvalError ?? ''}
          onClick={() => void run(approve)}
        >
          <Icon
            name={d.approval?.user_has_approved ? 'circle-check' : 'check'}
            size={1}
          />
          {d.approval?.user_has_approved ? 'Remove approval' : 'Approve'}
        </button>
      </div>
      {updated && (
        <Banner
          action={
            <Button
              size="sm"
              variant="link"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await loadReview(review, true)
                  setUpdated(false)
                  setSource(false)
                  setFiles([])
                })
              }
            >
              Refresh
            </Button>
          }
        >
          New commits available.
        </Banner>
      )}
      {(error || snap.error || d.approvalError) && (
        <div role="alert" className={styles.error}>
          {error || snap.error || d.approvalError}
        </div>
      )}
      {agent && (
        <div className={chrome.agentNote}>
          <Note
            tone={
              agent.state === 'failed'
                ? 'bad'
                : agent.state === 'waiting'
                  ? 'warn'
                  : agent.state === 'idle' || agent.state === 'finished'
                    ? 'ok'
                    : 'info'
            }
            title={`${HARNESS_LABEL[agent.harness]} review`}
            actions={
              <>
                {(agent.state === 'idle' || agent.state === 'finished') && (
                  <Button
                    size="sm"
                    icon="pencil"
                    onClick={() => showReviewInfo(review, 'drafts')}
                  >
                    Open drafts
                  </Button>
                )}
                {agent.state !== 'queued' && (
                  <Button
                    size="sm"
                    variant="quiet"
                    icon="square-terminal"
                    title="Show the review's tab"
                    onClick={() => void run(() => showAgentReview(review))}
                  >
                    Show
                  </Button>
                )}
                {agent.state !== 'finished' && agent.state !== 'failed' && (
                  <Button
                    size="sm"
                    variant="quiet"
                    icon="square"
                    title="Stop the agent; drafts it wrote are kept"
                    onClick={() => void run(() => stopAgentReview(review))}
                  >
                    Stop
                  </Button>
                )}
              </>
            }
          >
            <span className={chrome.agentDetail}>
              {(agent.state === 'queued' || agent.state === 'running') && (
                <Spinner />
              )}
              {agent.detail}
            </span>
          </Note>
        </div>
      )}
      <nav className={chrome.reviewActions} aria-label="MR information">
        {sections.map(({ section, label, icon, count }) => (
          <button key={section} onClick={() => showReviewInfo(review, section)}>
            <Icon name={icon} size={1} />
            {label}
            {/* A zero draws nothing: three dark "0" discs read as three things needing a look. */}
            {!!count && (
              <span className={chrome.sectionCount}>
                <Counter value={count} />
              </span>
            )}
          </button>
        ))}
      </nav>
      <div className={`${styles.sidebarSummary} ${chrome.fileControls}`}>
        <div className={styles.row}>
          <strong>Changes ({totals.files})</strong>
          <span className={styles.additions}>+{totals.additions}</span>
          <span className={styles.deletions}>−{totals.deletions}</span>
        </div>
        {incomplete && (
          <span className={styles.muted}>
            GitLab omitted part of the diff; totals are incomplete.
          </span>
        )}
        <input
          aria-label="Filter review files"
          placeholder="Filter files"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <label className={chrome.excludedToggle}>
          <input
            type="checkbox"
            checked={showExcluded}
            onChange={(e) => setShowExcluded(e.target.checked)}
          />{' '}
          Show excluded files{' '}
          <span className={styles.muted}>
            ({all.length - visible.length} hidden)
          </span>
        </label>
        <button
          className={chrome.sourceToggle}
          aria-pressed={source}
          disabled={busy}
          onClick={() =>
            void run(async () => {
              if (source) {
                setSource(false)
                return
              }
              await prepareSource(review)
              setFiles(
                await api<string[]>({
                  kind: 'sourceTree',
                  review,
                  sha: refs.head_sha,
                }),
              )
              setSource(true)
            })
          }
        >
          <Icon name={source ? 'file-text' : 'folder'} size={1} />
          {source ? 'Changed files' : 'Browse MR source'}
        </button>
      </div>
      <div className={styles.sidebarTree}>
        <FileTree
          key={source ? 'source' : 'changes'}
          defaultExpanded={!source}
          theme={theme}
          label={source ? 'MR source files' : 'MR changed files'}
          selected={snap.editor?.review === review ? snap.editor.path : null}
          files={
            source
              ? files
                  .filter(
                    pathVisibility(
                      snap.board.preferences.excludedFiles,
                      snap.board.preferences.excludeEnabled && !showExcluded,
                    ),
                  )
                  .filter((p) => p.toLowerCase().includes(query.toLowerCase()))
                  .map((path) => ({
                    path,
                    discussions: fileDiscussions(d.discussions, path),
                    drafts: draftsOf(path),
                  }))
              : visible
                  .filter((c) =>
                    c.new_path.toLowerCase().includes(query.toLowerCase()),
                  )
                  .map((c) => ({
                    path: c.new_path,
                    label: c.renamed_file
                      ? `${c.old_path} → ${c.new_path}`
                      : c.new_path,
                    discussions: fileDiscussions(
                      d.discussions,
                      c.new_path,
                      c.old_path,
                    ),
                    drafts: draftsOf(c.new_path, c.old_path),
                    detail: `${c.new_file ? 'A' : c.deleted_file ? 'D' : c.renamed_file ? 'R' : 'M'} +${changeTotals([c]).additions} −${changeTotals([c]).deletions}`,
                  }))
          }
          onOpen={(path) => {
            const change = visible.find((c) => c.new_path === path)
            openDocument({
              review,
              path,
              mode: source ? 'source' : 'diff',
              ...(change ? { change } : {}),
              refs,
            })
          }}
        />
        {!source && !visible.length && (
          <p className={styles.muted}>No visible changes.</p>
        )}
      </div>
      <footer className={styles.sidebarFooter}>
        Alt+PageUp / Alt+PageDown · Previous / next MR file
      </footer>
    </section>
  )
}
