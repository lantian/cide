import { UserLink } from './UserLink'
import { useEffect, useState } from 'react'
import { api, refreshBoard, startGitLab, useGitLab } from './store'
import { message } from './model'
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
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
    </section>
  )
}
