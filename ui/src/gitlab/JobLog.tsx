import { Icon } from '@/icons/Icon'
import { useMemo, useState } from 'react'
import { parseJobLog, type LogEntry } from './jobLogModel'
import styles from './GitLab.module.css'
function Entry({ entry, search }: { entry: LogEntry; search: string }) {
  const [expanded, setExpanded] = useState(
    entry.kind !== 'section' || !entry.collapsed,
  )
  if (entry.kind === 'line') {
    if (search && !entry.text.toLowerCase().includes(search.toLowerCase()))
      return null
    return (
      <div className={styles.logLine}>
        <span className={styles.logNumber} aria-hidden>
          {entry.number}
        </span>
        <code>
          {entry.spans.map((s, i) => (
            <span key={i} style={s.style}>
              {s.text}
            </span>
          ))}
          {'\n'}
        </code>
      </div>
    )
  }
  return (
    <section className={styles.logSection}>
      <button
        aria-expanded={expanded || !!search}
        onClick={() => setExpanded(!expanded)}
      >
        <Icon
          name={expanded || search ? 'chevron-down' : 'chevron-right'}
          size={1}
        />{' '}
        {entry.title}{' '}
        {entry.duration !== undefined && (
          <span className={styles.muted}>{entry.duration}s</span>
        )}
      </button>
      {(expanded || search) &&
        entry.children.map((child, index) => (
          <Entry key={index} entry={child} search={search} />
        ))}
    </section>
  )
}
export function JobLog({ text, search }: { text: string; search: string }) {
  const entries = useMemo(() => parseJobLog(text), [text])
  return (
    <>
      {entries.map((entry, i) => (
        <Entry key={i} entry={entry} search={search} />
      ))}
    </>
  )
}
