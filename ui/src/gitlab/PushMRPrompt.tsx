import { useSyncExternalStore, useState } from 'react'
import { gitlab, type GitLabAccount } from '@/ipc/client'
import { api, openReview } from './store'
import { message } from './model'
import type { MR } from './types'
import styles from './GitLab.module.css'
interface Check {
  mergeRequests: MR[]
  createUrl: string
  project: string
}
interface Prompt {
  account?: string
  result?: Check
  accounts?: GitLabAccount[]
  remoteUrl?: string
  branch?: string
  error?: string
}
let queue: Prompt[] = []
const listeners = new Set<() => void>()
function emit() {
  for (const l of listeners) l()
}
export function checkAfterPush(
  project: string,
  repo: string,
  remote: string,
  branch: string,
) {
  if (!branch) return
  void gitlab.afterPush(project, repo, remote, branch).then(
    (value) => {
      if (value) {
        queue = [...queue, value as Prompt]
        emit()
      }
    },
    (error) => {
      queue = [
        ...queue,
        { error: `Push succeeded. MR lookup failed: ${message(error)}` },
      ]
      emit()
    },
  )
}
export function PushMRPrompt() {
  const items = useSyncExternalStore(
    (l) => {
      listeners.add(l)
      return () => listeners.delete(l)
    },
    () => queue,
    () => queue,
  )
  const p = items[0]
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  if (!p) return null
  function close() {
    queue = queue.slice(1)
    setError('')
    emit()
  }
  async function run(work: () => Promise<unknown>) {
    setBusy(true)
    setError('')
    try {
      await work()
      close()
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  async function choose(account: string) {
    setBusy(true)
    setError('')
    try {
      const result = await api<Check>({
        kind: 'afterPush',
        account,
        remoteUrl: p!.remoteUrl!,
        branch: p!.branch!,
      })
      queue = [{ account, result }, ...queue.slice(1)]
      emit()
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <aside
      className={styles.prompt}
      role="dialog"
      aria-label="Merge request after push"
    >
      <p>
        {p.error ??
          (p.accounts
            ? 'Choose a GitLab account for this pushed branch.'
            : p.result?.mergeRequests.length
              ? 'An open merge request already exists.'
              : `Push succeeded. Create a merge request for ${p.result?.project ?? 'this branch'}?`)}
      </p>
      <div className={styles.row}>
        {p.accounts?.map((a) => (
          <button key={a.id} disabled={busy} onClick={() => void choose(a.id)}>
            {a.username}
          </button>
        ))}
        {p.result?.mergeRequests.map((mr) => (
          <button
            key={mr.id}
            disabled={busy}
            onClick={() => void run(() => openReview(p.account!, mr.web_url))}
          >
            Open !{mr.iid}
          </button>
        ))}
        {p.result && !p.result.mergeRequests.length && (
          <button
            disabled={busy}
            onClick={() => void run(() => gitlab.openUrl(p.result!.createUrl))}
          >
            Create MR in GitLab
          </button>
        )}
        <button disabled={busy} onClick={close}>
          Dismiss
        </button>
      </div>
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
    </aside>
  )
}
