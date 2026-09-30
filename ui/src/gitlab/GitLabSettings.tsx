import { UserLink } from './UserLink'
import { useEffect, useMemo, useState } from 'react'
import { api, refreshBoard, startGitLab, useGitLab } from './store'
import { message, repositoryOfMr } from './model'
import type { GitLabReviewPrompt } from '@/ipc/generated'
import { Button } from '@/kit/components/Button'
import { TextInput, Textarea } from '@/kit/components/Field'
import { Group, Row, ToggleRow } from '@/settings/controls'
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
  // On the settings screen's own vocabulary since M133 — `Group`, `Row`, `ToggleRow` and the
  // kit's fields and buttons — where it had its own `h3`s, bare inputs and a paragraph of muted
  // prose under each box: the one block on the Git page that did not look like the rest of
  // Settings. The explanations are the rows' (i) now.
  return (
    <section className={styles.settings} aria-label="GitLab settings">
      <Group
        title="GitLab accounts"
        info="Each account is a host and a personal access token with the api scope. The token is stored by cide and used for every GitLab request to that host."
      >
        {board.accounts.map((a) => (
          <Row
            key={a.id}
            label={a.username}
            anchor={`gitlab:${a.id}`}
            hint={a.host}
            control={
              <span className={styles.row}>
                <UserLink
                  user={{ id: a.userId, username: a.username, name: a.username }}
                  host={a.host}
                />
                <Button
                  size="sm"
                  disabled={busy}
                  onClick={() => void run(() => api({ kind: 'disconnect', account: a.id }))}
                >
                  Disconnect
                </Button>
              </span>
            }
          />
        ))}
        <Row
          label="Connect an account"
          hint="A GitLab host and a personal access token with the api scope."
          control={
            <span className={styles.connect}>
              <TextInput
                size="sm"
                mono
                aria-label="GitLab host"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="https://gitlab.example.com"
              />
              <TextInput
                size="sm"
                mono
                aria-label="Personal access token"
                type="password"
                autoComplete="off"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="Personal access token"
              />
              <Button
                size="sm"
                variant="primary"
                disabled={busy || !token.trim()}
                onClick={() =>
                  void run(async () => {
                    await api({ kind: 'connect', host, token })
                    setToken('')
                  })
                }
              >
                {busy ? 'Connecting…' : 'Connect'}
              </Button>
            </span>
          }
        />
      </Group>

      <Group title="Review excluded files">
        <ToggleRow
          label="Hide matching files in all MR reviews"
          hint="Excluded from review totals. Local Git changes are unaffected."
          checked={board.preferences.excludeEnabled}
          disabled={busy}
          onChange={(excludeEnabled) =>
            void run(() =>
              api({
                kind: 'preferences',
                preferences: { ...board.preferences, excludeEnabled },
              }),
            )
          }
        />
        <Row
          label="Patterns"
          hint="One repository-relative glob per line: *, ** and ?."
          control={
            <Button
              size="sm"
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
            </Button>
          }
        />
        <Textarea
          aria-label="Review exclusion patterns"
          value={patterns}
          onChange={(e) => setPatterns(e.target.value)}
          spellCheck={false}
        />
      </Group>

      <Group title="Agent review instructions">
        <Row
          label="Default instructions"
          hint="What Review with an agent’s instructions box starts with."
          info="A repository’s own entry wins over the default; you can still edit the text before starting a review. Line breaks become spaces when the review starts."
          control={
            <Button
              size="sm"
              disabled={busy}
              onClick={() =>
                void run(() =>
                  api({
                    kind: 'preferences',
                    preferences: { ...board.preferences, reviewPrompt, reviewPrompts },
                  }),
                )
              }
            >
              Save review instructions
            </Button>
          }
        />
        <Textarea
          aria-label="Default review instructions"
          value={reviewPrompt}
          onChange={(e) => setReviewPrompt(e.target.value)}
          placeholder="For every repository — e.g. focus on correctness and missing tests."
        />
        {reviewPrompts.map((row, i) => (
          <div key={i} className={styles.reviewPromptRow}>
            <div className={styles.row}>
              <TextInput
                size="sm"
                mono
                aria-label="Repository"
                list="gitlab-review-prompt-repositories"
                value={row.repository}
                onChange={(e) => editPrompt(i, { repository: e.target.value })}
                placeholder="group/repo"
                spellCheck={false}
              />
              <Button
                size="sm"
                disabled={busy}
                onClick={() => setReviewPrompts((rows) => rows.filter((_, j) => j !== i))}
              >
                Remove
              </Button>
            </div>
            <Textarea
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
          <Button
            size="sm"
            disabled={busy}
            onClick={() => setReviewPrompts((rows) => [...rows, { repository: '', prompt: '' }])}
          >
            Add repository
          </Button>
        </div>
      </Group>
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
    </section>
  )
}
