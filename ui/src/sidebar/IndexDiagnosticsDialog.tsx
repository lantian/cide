import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { fs, type FsIndexDiagnostics, type ProjectId } from '@/ipc/client'
import { Modal } from '@/overlays/ModalShell'
import { Dialog } from '@/kit/components/Overlay'
import { Button } from '@/kit/components/Button'
import { copyText } from './copyText'
import { fsMessage } from './fsError'
import { allocationLabels, budgetExplanation, budgetLines, folderExplanation, formatIndexDiagnostics, indexLimitLabel, indexMiB } from './indexDiagnostics'
import styles from './IndexDiagnosticsDialog.module.css'

export function IndexDiagnosticsDialog({ project, generation, indexing, onDismiss, onSettings }: {
  project: ProjectId
  generation: number
  indexing: boolean
  onDismiss: () => void
  onSettings: () => void
}) {
  const [report, setReport] = useState<FsIndexDiagnostics | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [refresh, setRefresh] = useState(0)
  const [copied, setCopied] = useState('')
  const [restarting, setRestarting] = useState(false)
  const [restartError, setRestartError] = useState<string | null>(null)
  const [restartedGeneration, setRestartedGeneration] = useState(0)
  const restartAttempt = useRef(0)
  const restartInFlight = useRef(false)
  const minimumGeneration = Math.max(generation, restartedGeneration)
  const close = useRef<HTMLButtonElement>(null)
  useEffect(() => () => { restartAttempt.current++; restartInFlight.current = false }, [project])
  useLayoutEffect(() => {
    const previous = document.activeElement
    close.current?.focus()
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus() }
  }, [])

  useEffect(() => {
    let cancelled = false
    setReport(null)
    setError(null)
    setCopied('')
    if (restarting) return
    void fs.indexDiagnostics(project).then((next) => {
      if (cancelled) return
      if (next.generation < minimumGeneration) {
        setError('The index changed while loading. Refresh to load the current diagnostics.')
      } else setReport(next)
    }).catch((reason: unknown) => { if (!cancelled) setError(fsMessage(reason)) })
    return () => { cancelled = true }
  }, [project, minimumGeneration, indexing, refresh, restarting])

  const current = !restarting && report && report.generation >= minimumGeneration ? report : null
  const busy = restarting || indexing || current?.indexing === true
  const restart = () => {
    if (!current || busy || restartInFlight.current) return
    restartInFlight.current = true
    const attempt = ++restartAttempt.current
    setRestarting(true)
    setRestartError(null)
    void fs.restartIndex(project, current.generation).then((status) => {
      if (attempt !== restartAttempt.current) return
      setRestartedGeneration((old) => Math.max(old, status.generation))
    }).catch((reason: unknown) => {
      if (attempt === restartAttempt.current) setRestartError(fsMessage(reason))
    }).finally(() => {
      if (attempt === restartAttempt.current) {
        restartInFlight.current = false
        setRestarting(false)
      }
    })
  }
  const stop = current?.refusal
  return <Modal onDismiss={onDismiss}>
    <Dialog title="Indexing diagnostics" width="wide" onClose={onDismiss}
      data-audit="indexDiagnostics" onKeyDown={(event) => {
        if (event.key === 'Escape') { event.stopPropagation(); onDismiss() }
      }}
      footNote={<span role="status">{copied}</span>}
      actions={<>
        <Button disabled={restarting} title="Reload diagnostics without rebuilding the index" onClick={() => setRefresh((value) => value + 1)}>Refresh</Button>
        <Button disabled={!current || busy} title="Rebuild this project's file index using current settings" onClick={restart}>
          {busy ? 'Indexing…' : 'Restart indexing'}
        </Button>
        <Button disabled={!current} onClick={() => {
          if (current) void copyText(formatIndexDiagnostics(current)).then((ok) => setCopied(ok ? 'Diagnostics copied' : 'Could not copy diagnostics'))
        }}>Copy diagnostics</Button>
        <Button onClick={onSettings}>Settings</Button>
        <Button ref={close} variant="primary" onClick={onDismiss}>Close</Button>
      </>}>
      {restartError && <p role="alert">Could not restart indexing: {restartError}</p>}
      {error ? <p role="alert">Could not load diagnostics: {error}</p> : !current ? <p role="status">{restarting ? 'Rebuilding the project index…' : 'Loading indexing diagnostics…'}</p> : <div className={styles.content}>
        <section aria-label="Index summary">
          <p className={styles.path}>Roots: {current.roots.join(', ')}</p>
          <p>{current.files.toLocaleString()} files · {current.dirs.toLocaleString()} folders indexed{current.indexing ? ' · Indexing in progress' : ''}</p>
          <p>Show hidden files: {current.showHiddenFiles ? 'On' : 'Off'} · Show ignored files: {current.showIgnoredFiles ? 'On' : 'Off'}</p>
          <p className={styles.path}>Indexing exclusions ({current.exclusionsSource}): {current.indexExcludedFolders?.join(', ') || 'None'}</p>
          <p>Explorer browsing cache: {indexMiB(current.browseCacheBytes ?? 0)} / 16 MiB</p>
          <p>{budgetExplanation}</p>
          {budgetLines(current.usage).map((line) => <div key={line}>{line}</div>)}
        </section>
        <section aria-label="Stop point">
          <h3>{stop ? indexLimitLabel(stop.reason) : 'No memory limit recorded for this generation'}</h3>
          {stop && <>
            <p>{allocationLabels[stop.allocation]} · Requested {stop.requestedBytes.toLocaleString()} bytes ({indexMiB(stop.requestedBytes)})</p>
            <p className={styles.path}>Path: {stop.path ?? 'No path (infrastructure allocation)'}</p>
            {stop.folder && <p className={styles.path}>Containing folder: {stop.folder}</p>}
            <p>Usage at first refusal:</p>
            {budgetLines(stop.usage).map((line) => <div key={line}>{line}</div>)}
            <p>This is the stop point, not necessarily the largest contributor. Coverage is partial; unvisited folders cannot be ranked.</p>
          </>}
        </section>
        <section aria-label="Largest indexed folders">
          <h3>Largest indexed folders</h3>
          <p>{folderExplanation}</p>
          {current.folders.length === 0 ? <p>No indexed folders available{current.indexing ? ' yet. Refresh after indexing finishes.' : '.'}</p> : <table className={styles.table}>
            <thead><tr><th scope="col">Folder</th><th scope="col">Estimated allocation</th><th scope="col">Files</th><th scope="col">Folders</th></tr></thead>
            <tbody>{current.folders.map((row) => <tr key={`${row.root}\0${row.path}`}>
              <td className={styles.path} title={row.path}>{row.relativePath}{current.roots.length > 1 && <small className={styles.root}>{row.root}</small>}</td>
              <td title={`${row.bytes.toLocaleString()} bytes`}>{indexMiB(row.bytes)}</td><td>{row.files.toLocaleString()}</td><td>{row.dirs.toLocaleString()}</td>
            </tr>)}</tbody>
          </table>}
        </section>
      </div>}
    </Dialog>
  </Modal>
}
