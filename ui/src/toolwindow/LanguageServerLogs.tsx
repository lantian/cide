import { useEffect, useRef, useState } from 'react'
import { languageServers, type ProjectId } from '@/ipc/client'
import { copyText } from '@/sidebar/copyText'
import styles from './ProblemsTab.module.css'

export function LanguageServerLogs({ project, source, label }: { project: ProjectId; source: string; label: string }) {
  const [{ text, truncated }, setLog] = useState({ text: '', truncated: false })
  const [error, setError] = useState<string | null>(null)
  const box = useRef<HTMLPreElement>(null)
  const follow = useRef(true)
  useEffect(() => {
    let disposed = false
    let cursor = 0
    let timer: ReturnType<typeof setTimeout>
    const read = async () => {
      try {
        const result = await languageServers.logs(project, source, cursor)
        if (disposed) return
        cursor = result.cursor
        setError(null)
        if (result.entries.length || result.truncated) {
          const chunk = result.entries.map(entry => `${new Date(entry.timestamp).toLocaleTimeString()} [${entry.level}] ${entry.message}`).join('\n')
          setLog(old => {
            const next = chunk ? (old.text ? `${old.text}\n${chunk}` : chunk) : old.text
            const overflow = next.length > 1024 * 1024
            return { text: overflow ? next.slice(-1024 * 1024) : next, truncated: old.truncated || result.truncated || overflow }
          })
        }
      } catch (e) { if (!disposed) setError(String(e)) }
      finally { if (!disposed) timer = setTimeout(() => { void read() }, 1000) }
    }
    void read()
    return () => { disposed = true; clearTimeout(timer) }
  }, [project, source])
  useEffect(() => { if (follow.current && box.current) box.current.scrollTop = box.current.scrollHeight }, [text])
  return <section className={styles.panel} aria-label={`${label} logs`}>
    <div className={styles.toolbar}><span>{label}</span><button className={styles.button} disabled={!text} onClick={() => { void copyText(text) }}>Copy Logs</button></div>
    {truncated && <div className={styles.note}>Older log entries were discarded to keep the log bounded.</div>}
    {error && <div className={styles.note} role="alert">{error}</div>}
    <pre ref={box} className={styles.logs} tabIndex={0} onScroll={e => { const el = e.currentTarget; follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24 }}>{text || (error ? '' : 'No log messages yet.')}</pre>
  </section>
}
