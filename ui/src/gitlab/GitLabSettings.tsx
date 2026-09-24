import { UserLink } from './UserLink'
import { useEffect, useMemo, useState } from 'react'
import { api, refreshBoard, startGitLab, useGitLab } from './store'
import { message, repositoryOfMr } from './model'
import type { GitLabReviewPrompt } from '@/ipc/generated'
import styles from './GitLab.module.css'
export function GitLabSettings() {
  const { board } = useGitLab()
  const [host, setHost] = useState('https://gitlab.com')
  const [token, setToken] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [patterns, setPatterns] = useState(
    board.preferences.excludedFiles.join('\n'),
  )
  useEffect(() => {
    startGitLab()
  }, [])
  useEffect(
    () => setPatterns(board.preferences.excludedFiles.join('\n')),
    [board.preferences.excludedFiles],
  )
  const [reviewPrompt, setReviewPrompt] = useState(board.preferences.reviewPrompt)
  const [reviewPrompts, setReviewPrompts] = useState<GitLabReviewPrompt[]>(
    board.preferences.reviewPrompts,
  )
  useEffect(
    () => setReviewPrompt(board.preferences.reviewPrompt),
    [board.preferences.reviewPrompt],
  )
  useEffect(
    () => setReviewPrompts(board.preferences.reviewPrompts),
    [board.preferences.reviewPrompts],
  )
  // The repositories of the MRs already open, offered as the repository field's suggestions.
  const knownRepositories = useMemo(
    () =>
      [
        ...new Set(
          board.reviews.flatMap((r) => {
            const repo = repositoryOfMr(r.url)
            return repo ? [repo.path] : []
          }),
        ),
      ].sort(),
    [board.reviews],
  )
  const editPrompt = (at: number, patch: Partial<GitLabReviewPrompt>) =>
    setReviewPrompts((rows) =>
      rows.map((row, i) => (i === at ? { ...row, ...patch } : row)),
    )
  async function run(work: () => Promise<unknown>) {
    setBusy(true)
    setError('')
    try {
      await work()
      await refreshBoard()
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <section className={styles.settings} aria-label="GitLab settings">
      <h3>GitLab accounts</h3>
      {board.accounts.map((a) => (
        <div key={a.id} className={styles.row}>
          <span>
            <UserLink
              user={{ id: a.userId, username: a.username, name: a.username }}
              host={a.host}
            />{' '}
            · {a.host}
          </span>
          <button
            disabled={busy}
            onClick={() =>
              void run(() => api({ kind: 'disconnect', account: a.id }))
            }
          >
            Disconnect
          </button>
        </div>
      ))}
      <input
        aria-label="GitLab host"
        value={host}
        onChange={(e) => setHost(e.target.value)}
        placeholder="https://gitlab.example.com"
      />
      <input
        aria-label="Personal access token"
        type="password"
        autoComplete="off"
        value={token}
        onChange={(e) => setToken(e.target.value)}
        placeholder="Personal access token (api scope)"
      />
      <button
        disabled={busy || !token.trim()}
        onClick={() =>
          void run(async () => {
            await api({ kind: 'connect', host, token })
            setToken('')
          })
        }
      >
        {busy ? 'Connecting…' : 'Connect account'}
      </button>
      <h3>Review excluded files</h3>
      <label>
        <input
          type="checkbox"
          checked={board.preferences.excludeEnabled}
          disabled={busy}
          onChange={(e) =>
            void run(() =>
              api({
                kind: 'preferences',
                preferences: {
                  ...board.preferences,
                  excludeEnabled: e.target.checked,
                },
              }),
            )
          }
        />
        Hide matching files in all MR reviews
      </label>
      <textarea
        aria-label="Review exclusion patterns"
        value={patterns}
        onChange={(e) => setPatterns(e.target.value)}
        spellCheck={false}
      />
      <span className={styles.muted}>
        One repository-relative glob per line (*, **, and ?). Matching files are
        excluded from review totals. Local Git changes are unaffected.
      </span>
      <button
        disabled={busy}
        onClick={() =>
          void run(() =>
            api({
              kind: 'preferences',
              preferences: {
                ...board.preferences,
                excludedFiles: patterns
                  .split('\n')
                  .map((p) => p.trim())
                  .filter(Boolean),
              },
            }),
          )
        }
      >
        Save patterns
      </button>
      <h3>Agent review instructions</h3>
      <span className={styles.muted}>
        What the instructions box of Review with an agent starts with. A repository&apos;s own
        entry wins over the default; you can still edit the text before starting a review. Line
        breaks become spaces when the review starts.
      </span>
      <textarea
        aria-label="Default review instructions"
        value={reviewPrompt}
        onChange={(e) => setReviewPrompt(e.target.value)}
        placeholder="For every repository — e.g. focus on correctness and missing tests."
      />
      {reviewPrompts.map((row, i) => (
        <div key={i} className={styles.reviewPromptRow}>
          <div className={styles.row}>
            <input
              aria-label="Repository"
              list="gitlab-review-prompt-repositories"
              value={row.repository}
              onChange={(e) => editPrompt(i, { repository: e.target.value })}
              placeholder="group/repo"
              spellCheck={false}
            />
            <button
              disabled={busy}
              onClick={() =>
                setReviewPrompts((rows) => rows.filter((_, j) => j !== i))
              }
            >
              Remove
            </button>
          </div>
          <textarea
            aria-label={`Review instructions for ${row.repository || 'this repository'}`}
            value={row.prompt}
            onChange={(e) => editPrompt(i, { prompt: e.target.value })}
          />
        </div>
      ))}
      <datalist id="gitlab-review-prompt-repositories">
        {knownRepositories.map((r) => (
          <option key={r} value={r} />
        ))}
      </datalist>
      <div className={styles.row}>
        <button
          disabled={busy}
          onClick={() =>
            setReviewPrompts((rows) => [...rows, { repository: '', prompt: '' }])
          }
        >
          Add repository
        </button>
        <button
          disabled={busy}
          onClick={() =>
            void run(() =>
              api({
                kind: 'preferences',
                preferences: {
                  ...board.preferences,
                  reviewPrompt,
                  reviewPrompts,
                },
              }),
            )
          }
        >
          Save review instructions
        </button>
      </div>
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
    </section>
  )
}
