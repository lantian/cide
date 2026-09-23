/**
 * Local draft comments on an MR — an agent's review findings, until the user publishes them.
 * (M85)
 *
 * A draft reads like a GitLab thread and is marked everywhere as not one: "Draft · not
 * published" on the card, a dashed border, and a severity badge that is the draft's metadata
 * and **never** part of its body. Publishing posts the body alone, exactly as shown.
 */
import { useMemo, useState } from 'react'
import { closeOverlay } from '@/overlays/store'
import type { GitLabDraft, GitLabPublished, GitLabSeverity } from '@/ipc/generated'
import { Markdown } from './Markdown'
import {
  api,
  data,
  openDocument,
  refreshDiscussions,
  refreshDrafts,
  useGitLab,
} from './store'
import { message } from './model'
import styles from './GitLab.module.css'

export const SEVERITIES: readonly GitLabSeverity[] = [
  'critical',
  'major',
  'minor',
  'suggestion',
]
const LABEL: Record<GitLabSeverity, string> = {
  critical: 'Critical',
  major: 'Major',
  minor: 'Minor',
  suggestion: 'Suggestion',
}

export function SeverityBadge({ severity }: { severity: GitLabSeverity }) {
  return (
    <span className={styles.severity} data-severity={severity}>
      {LABEL[severity]}
    </span>
  )
}

/** The thread anchor a draft sits on, keyed like `ReviewDiff`'s threads: `new:12` / `old:9`. */
export function draftAnchor(draft: GitLabDraft): string | null {
  const p = draft.position as {
    old_line?: number | null
    new_line?: number | null
  } | null
  if (!p) return null
  return p.new_line == null ? `old:${p.old_line}` : `new:${p.new_line}`
}

/** Whether a draft belongs on the diff drawn for `(path, base, head)`. */
export function draftOn(
  draft: GitLabDraft,
  path: string,
  base: string,
  head: string,
): boolean {
  const p = draft.position as {
    base_sha?: string
    head_sha?: string
    new_path?: string
    old_path?: string
  } | null
  return (
    !!p &&
    p.head_sha === head &&
    p.base_sha === base &&
    (p.new_path === path || p.old_path === path)
  )
}

async function publish(review: string, drafts: string[]): Promise<string> {
  const outcome = await api<GitLabPublished>({
    kind: 'draftPublish',
    review,
    drafts,
  })
  await Promise.all([refreshDrafts(review), refreshDiscussions(review)])
  if (!outcome.failed.length) return ''
  return outcome.failed
    .map((f) => `${f.draft}: ${f.error}`)
    .join('\n')
}

export function DraftCard({
  review,
  draft,
  inline = false,
  selected,
  onSelect,
}: {
  review: string
  draft: GitLabDraft
  inline?: boolean
  selected?: boolean
  onSelect?: (selected: boolean) => void
}) {
  const [editing, setEditing] = useState(false)
  const [body, setBody] = useState(draft.body)
  const [severity, setSeverity] = useState<GitLabSeverity>(draft.severity)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const current = data.get(review)
  const outdated =
    current !== undefined && draft.headSha !== current.version.head_commit_sha
  const where =
    draft.path && draft.line
      ? `${draft.path}:${draft.line}${draft.side === 'old' ? ' (old)' : ''}`
      : 'General comment'
  async function act(work: () => Promise<string | void>) {
    setBusy(true)
    setError('')
    try {
      const failed = await work()
      if (failed) setError(failed)
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <article
      className={styles.draft}
      data-inline={inline}
      data-severity={draft.severity}
      aria-label={`${LABEL[draft.severity]} draft comment, not published`}
    >
      <header className={styles.threadHeader}>
        <span className={styles.draftTitle}>
          {onSelect && (
            <input
              type="checkbox"
              aria-label="Select draft for publishing"
              checked={!!selected}
              onChange={(e) => onSelect(e.target.checked)}
            />
          )}
          <SeverityBadge severity={draft.severity} />
          <strong>Draft · not published</strong>
          <span className={styles.muted}>{draft.author.label}</span>
          {outdated && <span className={styles.muted}>· Outdated</span>}
        </span>
      </header>
      {!inline && (
        <button
          className={styles.draftWhere}
          disabled={!draft.path}
          onClick={() => {
            const change = current?.version.diffs?.find(
              (c) => c.new_path === draft.path || c.old_path === draft.path,
            )
            if (!draft.path || !current) return
            closeOverlay()
            openDocument({
              review,
              path: change?.new_path ?? draft.path,
              mode: 'diff',
              ...(change && !outdated ? { change } : {}),
              refs: {
                base_sha: current.version.base_commit_sha,
                start_sha: current.version.start_commit_sha,
                head_sha: current.version.head_commit_sha,
              },
              oldPath: draft.oldPath ?? draft.path,
              at: {
                line: draft.line ?? 1,
                column: 1,
                side: draft.side === 'old' ? 'old' : 'new',
              },
            })
          }}
        >
          {where}
        </button>
      )}
      {editing ? (
        <form
          className={styles.draftEdit}
          onSubmit={(e) => {
            e.preventDefault()
            void act(async () => {
              await api({
                kind: 'draftEdit',
                review,
                draft: draft.id,
                body,
                severity,
              })
              await refreshDrafts(review)
              setEditing(false)
            })
          }}
        >
          <select
            aria-label="Severity"
            value={severity}
            onChange={(e) => setSeverity(e.target.value as GitLabSeverity)}
          >
            {SEVERITIES.map((s) => (
              <option key={s} value={s}>
                {LABEL[s]}
              </option>
            ))}
          </select>
          <textarea
            className={styles.comment}
            aria-label="Draft comment"
            value={body}
            onChange={(e) => setBody(e.target.value)}
            autoFocus
          />
          <div className={styles.row}>
            <button disabled={busy || !body.trim()}>Save draft</button>
            <button
              type="button"
              onClick={() => {
                setEditing(false)
                setBody(draft.body)
                setSeverity(draft.severity)
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <div className={styles.note}>
          <Markdown review={review} text={draft.body} baseUrl={current?.mr.web_url} />
        </div>
      )}
      {!editing && (
        <div className={styles.row}>
          <button
            disabled={busy}
            title="Post this comment to GitLab, exactly as written"
            onClick={() => void act(() => publish(review, [draft.id]))}
          >
            {busy ? 'Working…' : 'Publish'}
          </button>
          <button
            disabled={busy}
            onClick={() => {
              // From the draft as it is now: an agent may have edited it since this rendered.
              setBody(draft.body)
              setSeverity(draft.severity)
              setEditing(true)
            }}
          >
            Edit
          </button>
          <button
            disabled={busy}
            onClick={() =>
              void act(async () => {
                await api({ kind: 'draftDiscard', review, drafts: [draft.id] })
                await refreshDrafts(review)
              })
            }
          >
            Discard
          </button>
        </div>
      )}
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
    </article>
  )
}

/** Every draft on the MR, by file, most severe first, with the batch publish. */
export function Drafts({ review }: { review: string }) {
  useGitLab()
  const d = data.get(review)
  const drafts = d?.drafts ?? []
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set())
  const [filter, setFilter] = useState<GitLabSeverity | 'all'>('all')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const shown = useMemo(
    () =>
      drafts
        .filter((x) => filter === 'all' || x.severity === filter)
        .slice()
        .sort(
          (a, b) =>
            (a.path ?? '').localeCompare(b.path ?? '') ||
            SEVERITIES.indexOf(a.severity) - SEVERITIES.indexOf(b.severity) ||
            (a.line ?? 0) - (b.line ?? 0),
        ),
    [drafts, filter],
  )
  const live = new Set(drafts.map((x) => x.id))
  const chosen = [...selected].filter((id) => live.has(id))
  const groups = new Map<string, GitLabDraft[]>()
  for (const x of shown) {
    const key = x.path ?? ''
    groups.set(key, [...(groups.get(key) ?? []), x])
  }
  async function publishMany(ids: string[]) {
    setBusy(true)
    setError('')
    try {
      setError(await publish(review, ids))
      setSelected(new Set())
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  const counts = SEVERITIES.map(
    (s) => [s, drafts.filter((x) => x.severity === s).length] as const,
  )
  return (
    <div className={styles.discussions}>
      <p className={styles.muted}>
        Drafts stay on this machine. Nothing is posted to GitLab until you
        publish it; the severity is shown here only and is never published.
      </p>
      <div className={styles.row}>
        <select
          aria-label="Show severity"
          value={filter}
          onChange={(e) => setFilter(e.target.value as GitLabSeverity | 'all')}
        >
          <option value="all">All severities ({drafts.length})</option>
          {counts.map(([s, n]) => (
            <option key={s} value={s}>
              {LABEL[s]} ({n})
            </option>
          ))}
        </select>
        <button
          disabled={busy || !chosen.length}
          onClick={() => void publishMany(chosen)}
        >
          Publish selected ({chosen.length})
        </button>
        <button
          disabled={busy || !shown.length}
          onClick={() => void publishMany(shown.map((x) => x.id))}
        >
          Publish {filter === 'all' ? 'all' : 'shown'} ({shown.length})
        </button>
      </div>
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
      {!drafts.length && (
        <p className={styles.muted}>
          No drafts. Start a review with an agent from the MR panel.
        </p>
      )}
      {[...groups].map(([path, list]) => (
        <section key={path} aria-label={path || 'General comments'}>
          <h3 className={styles.draftGroup}>{path || 'General comments'}</h3>
          {list.map((x) => (
            <DraftCard
              key={x.id}
              review={review}
              draft={x}
              selected={selected.has(x.id)}
              onSelect={(on) =>
                setSelected((old) => {
                  const next = new Set(old)
                  if (on) next.add(x.id)
                  else next.delete(x.id)
                  return next
                })
              }
            />
          ))}
        </section>
      ))}
    </div>
  )
}
