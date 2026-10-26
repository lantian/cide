import { useEffect, useState, useSyncExternalStore } from 'react'
import { clipboardComparison } from '@/ipc/client'
import { getDiffView, getServerDiffView, setDiffView, subscribeDiffView } from '@/editor/diffViewMode'
import { saveTab } from '@/editor/openBuffers'
import { Button } from '@/kit/components/Button'
import { EditorPane, type EditorPaneProps } from './EditorPane'
import styles from './ClipboardDiffPane.module.css'

export function ClipboardDiffPane({ snapshotId, ...props }: EditorPaneProps & { snapshotId: string }) {
  const [clip, setClip] = useState<{ id: string; text: string } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const layout = useSyncExternalStore(subscribeDiffView, getDiffView, getServerDiffView)
  useEffect(() => {
    let alive = true
    setError(null)
    void clipboardComparison.content(snapshotId).then(text => {
      if (alive) setClip({ id: snapshotId, text })
    }, error => { if (alive) setError(String(error)) })
    return () => { alive = false }
  }, [snapshotId])
  return <div className={styles.pane}>
    <header className={styles.header}>
      <span className={styles.label}>Clipboard vs {props.path.split('/').at(-1)}</span>
      <div role="group" aria-label="Diff layout" className={styles.buttons}>
        {(['split', 'unified'] as const).map(mode => <Button key={mode} size="sm"
          aria-pressed={layout.view === mode} disabled={!layout.writable}
          onClick={() => setDiffView(mode)}>{mode === 'split' ? 'Split' : 'Unified'}</Button>)}
        <Button size="sm" onClick={() => { void saveTab(props.tab ?? null)?.catch(() => {}) }}>Save</Button>
      </div>
    </header>
    {error ? <div role="alert">{error}</div> : null}
    <div className={styles.editor}>
      {clip ? <EditorPane {...props} comparison={{ text: clip.text, id: clip.id, layout: layout.view }} />
        : <div role="status">Reading clipboard comparison…</div>}
    </div>
  </div>
}
