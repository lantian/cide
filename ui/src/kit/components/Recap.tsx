import { useLayoutEffect, useRef, useState, type ReactElement } from 'react'
import { IconButton } from './Button'
import styles from './Recap.module.css'

export interface RecapProps {
  text: string
  ordinal: number
  total: number
  expanded: boolean
  status?: string | undefined
  busy?: boolean | undefined
  onPrevious: () => void
  onNext: () => void
  onFirst: () => void
  onLatest: () => void
  onExpand: () => void
  onReveal?: (() => void) | undefined
}

/** A compact, controlled history surface. Conversation identity and data fetching belong to
 * the caller; the kit is usable without Tauri on its specimen page. */
export function Recap({ text, ordinal, total, expanded, status, busy,
  onPrevious, onNext, onFirst, onLatest, onExpand, onReveal }: RecapProps): ReactElement {
  const content = useRef<HTMLDivElement>(null)
  const [overflows, setOverflows] = useState(false)
  useLayoutEffect(() => {
    const element = content.current
    if (!element || status !== undefined) { setOverflows(false); return }
    // Measure the collapsed layout even while the full text is open, so the collapse
    // control stays available. A hidden clone has the same width/font but no flex sizing;
    // it catches both explicit newlines and wrapping after a pane or font resize.
    const measure = (): void => {
      const preview = element.cloneNode(true) as HTMLDivElement
      preview.className = styles.preview ?? ''
      preview.removeAttribute('tabindex')
      preview.setAttribute('aria-hidden', 'true')
      Object.assign(preview.style, { position: 'absolute', visibility: 'hidden',
        pointerEvents: 'none', width: `${element.clientWidth}px` })
      element.parentElement?.appendChild(preview)
      setOverflows(preview.scrollHeight > preview.clientHeight + 1)
      preview.remove()
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    document.fonts?.addEventListener('loadingdone', measure)
    return () => { observer.disconnect(); document.fonts?.removeEventListener('loadingdone', measure) }
  }, [text, status])
  const showExpanded = expanded && overflows
  const revealable = !!onReveal && !busy && status === undefined && total > 0
  return (
    <section className={styles.recap} aria-label="User input recap">
      <div className={styles.tools}>
        <IconButton icon="chevron-first" label="First input" disabled={busy || ordinal <= 1} onClick={onFirst} />
        <IconButton icon="chevron-left" label="Previous input" disabled={busy || ordinal <= 1} onClick={onPrevious} />
        <IconButton icon="chevron-right" label="Next input" disabled={busy || ordinal >= total} onClick={onNext} />
        <IconButton icon="chevron-last" label="Latest input" disabled={busy || ordinal >= total} onClick={onLatest} />
        <span className={styles.counter} aria-live="polite">{ordinal} / {total}</span>
        {overflows && total > 0 && <IconButton icon={showExpanded ? 'chevron-up' : 'chevron-down'} label={showExpanded ? 'Collapse input' : 'Expand input'}
          aria-expanded={showExpanded} onClick={onExpand} />}
      </div>
      <div ref={content} className={[showExpanded ? styles.full : styles.preview, revealable ? styles.reveal : ''].filter(Boolean).join(' ')}
        role={revealable ? 'button' : undefined} tabIndex={revealable || showExpanded ? 0 : undefined}
        aria-label={revealable ? 'Show input in console' : undefined} title={revealable ? 'Show input in console' : undefined}
        onClick={() => {
          if (!revealable) return
          const selection = window.getSelection()
          if (selection && !selection.isCollapsed && (content.current?.contains(selection.anchorNode) || content.current?.contains(selection.focusNode))) return
          onReveal?.()
        }} onKeyDown={event => {
          if (revealable && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); onReveal?.() }
        }}>
        {status ?? text}
      </div>
    </section>
  )
}
