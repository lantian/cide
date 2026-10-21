import { useEffect, useLayoutEffect, useRef, useState } from 'react'

import { sessionJournal, type ConversationItem, type ConversationPage, type ProjectId } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'

import styles from './ConversationArchivePane.module.css'

interface Props {
  project: ProjectId
  id: string
  title: string
  active: boolean
}

/** A saved conversation is a document: this component has no terminal, prompt or resume action. */
export function ConversationArchivePane({ project, id, title, active }: Props) {
  const [page, setPage] = useState<ConversationPage | null>(null)
  const [items, setItems] = useState<ConversationItem[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [details, setDetails] = useState<Record<string, { text: string; next: number | null; error?: string }>>({})
  const scroll = useRef<HTMLDivElement>(null)
  const loaded = useRef<string | null>(null)
  const scrollToLatest = useRef(false)

  useEffect(() => {
    const key = JSON.stringify([project, id])
    if (!active || loaded.current === key) return
    loaded.current = key
    let disposed = false
    let finished = false
    setPage(null)
    setItems([])
    setDetails({})
    setError(null)
    void sessionJournal.conversationRead(project, id).then((found) => {
      if (disposed) return
      finished = true
      scrollToLatest.current = true
      setPage(found)
      setItems(found.items)
    }).catch((reason: unknown) => {
      if (!disposed) {
        finished = true
        setError(errorText(reason))
      }
    })
    return () => {
      disposed = true
      if (!finished && loaded.current === key) loaded.current = null
    }
  }, [project, id, active])

  // The first page is the newest page. Move after React has rendered its messages, before
  // painting them, and leave subsequent pagination and detail expansion at the reader's position.
  useLayoutEffect(() => {
    if (!active || page === null || !scrollToLatest.current) return
    const node = scroll.current
    if (node === null) return
    node.scrollTop = node.scrollHeight
    scrollToLatest.current = false
  }, [active, page, items])

  async function loadOlder() {
    if (page?.before == null || loadingOlder) return
    setLoadingOlder(true)
    const node = scroll.current
    const oldHeight = node?.scrollHeight ?? 0
    try {
      const older = await sessionJournal.conversationRead(project, id, page.generation, page.before)
      if (older.generation !== page.generation || older.reset) {
        setItems(older.items)
        setDetails({})
      } else {
        setItems((current) => [...older.items, ...current])
      }
      setPage(older)
      requestAnimationFrame(() => {
        if (node) node.scrollTop += node.scrollHeight - oldHeight
      })
    } catch (reason: unknown) {
      setError(errorText(reason))
    } finally {
      setLoadingOlder(false)
    }
  }

  async function showDetail(item: ConversationItem) {
    if (page === null) return
    if (details[item.id] !== undefined) {
      setDetails((current) => {
        const next = { ...current }
        delete next[item.id]
        return next
      })
      return
    }
    try {
      const [text, next] = await sessionJournal.conversationDetail(project, id, page.generation, item.id, 0)
      setDetails((current) => ({ ...current, [item.id]: { text, next } }))
    } catch (reason: unknown) {
      setDetails((current) => ({ ...current, [item.id]: { text: '', next: null, error: errorText(reason) } }))
    }
  }

  async function moreDetail(item: ConversationItem) {
    const detail = details[item.id]
    if (page === null || detail?.next == null) return
    try {
      const [text, next] = await sessionJournal.conversationDetail(project, id, page.generation, item.id, detail.next)
      setDetails((current) => ({ ...current, [item.id]: { text: (current[item.id]?.text ?? '') + text, next } }))
    } catch (reason: unknown) {
      setDetails((current) => ({ ...current, [item.id]: { ...detail, error: errorText(reason) } }))
    }
  }

  return (
    <section className={styles.pane} aria-label={`Saved conversation: ${title}`} data-audit="conversationArchive">
      <header className={styles.header}>
        <strong>{title}</strong>
        <span className={styles.badge}>Read only</span>
      </header>
      <div className={styles.scroll} ref={scroll}>
        {error !== null && <p className={styles.notice} role="alert">{error}</p>}
        {page === null && error === null && <p className={styles.notice}>Reading saved conversation…</p>}
        {page !== null && !page.available && (
          <p className={styles.notice} role="status">
            {page.detail ?? 'The saved conversation is no longer available.'}
          </p>
        )}
        {page?.before != null && (
          <button className={styles.button} type="button" onClick={() => void loadOlder()} disabled={loadingOlder}>
            {loadingOlder ? 'Loading…' : 'Load earlier messages'}
          </button>
        )}
        {page?.available && items.length === 0 && <p className={styles.notice}>No saved messages.</p>}
        {items.map((item) => {
          const detail = details[item.id]
          return (
            <article className={styles.item} data-kind={item.kind} key={item.id}>
              <div className={styles.itemHeader}>
                <span>{item.kind === 'tool' ? item.tool ?? 'Tool' : item.kind}</span>
                {item.status && <span className={styles.status}>{item.status}</span>}
              </div>
              {item.kind !== 'tool' && <pre className={styles.text}>{detail?.text ?? item.text}</pre>}
              {item.detailAvailable && (
                <button className={styles.button} type="button" onClick={() => void showDetail(item)}>
                  {detail === undefined ? 'Show details' : 'Hide details'}
                </button>
              )}
              {item.kind === 'tool' && detail !== undefined && <pre className={styles.text}>{detail.text}</pre>}
              {detail?.next != null && (
                <button className={styles.button} type="button" onClick={() => void moreDetail(item)}>Load more detail</button>
              )}
              {detail?.error && <p className={styles.notice} role="alert">{detail.error}</p>}
            </article>
          )
        })}
      </div>
    </section>
  )
}
