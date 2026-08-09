/**
 * Ctrl+P — the file picker.
 *
 * The candidate set lives in Rust and streams: `picker_open` starts a session, frames of at
 * most 200 rows arrive on a channel as the walk finds them, and `picker_query` re-runs the
 * match. That shape is what makes the picker usable on a 100k-file repository *before*
 * indexing finishes, which is M8's acceptance criterion — a command that returned a `Vec`
 * would make the first keystroke wait for the walk.
 *
 * None of those three commands exist yet (`cide-fs`/`cide-search` are being built in
 * parallel), so every call goes through `pendingCommand` and a missing handler shows an
 * empty list with a reason rather than an unhandled rejection.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ModalShell, Hint } from './ModalShell'
import { kindBadge, matchCounter } from './format'
import { isListKey, listAction } from './listKeys'
import { pendingCommand, picker as pickerApi, type PickerFrame, type PickerHit, type ProjectId } from '@/ipc/client'
import styles from './Overlay.module.css'

/** 26px rows, from the mock. Fixed, so the virtualizer never has to measure. */
const ROW_HEIGHT = 26

/* `string | undefined` because Vite types a CSS module as `Record<string, string>` and
   `noUncheckedIndexedAccess` makes every lookup optional; see `src/vite-env.d.ts`. */
const TONE_CLASS: Readonly<Record<string, string | undefined>> = {
  accent: styles.toneAccent,
  blue: styles.toneBlue,
  cyan: styles.toneCyan,
  yellow: styles.toneYellow,
  green: styles.toneGreen,
  purple: styles.tonePurple,
  dim: styles.toneDim,
  faint: styles.toneFaint,
}

export interface FilePickerProps {
  project: ProjectId
  onDismiss: () => void
  /** ⏎ */
  onOpen: (path: string) => void
  /** ⇧⏎ */
  onOpenInSplit: (path: string) => void
  /** ⌥⏎ — sends `@path` to the focused Claude pane via the IDE MCP `at_mentioned` tool. */
  onMention: (path: string) => void
}

export function FilePicker({ project, onDismiss, onOpen, onOpenInSplit, onMention }: FilePickerProps) {
  const [query, setQuery] = useState('')
  const [frame, setFrame] = useState<PickerFrame | null>(null)
  const [selected, setSelected] = useState(0)
  const [session, setSession] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)

  const scrollRef = useRef<HTMLDivElement>(null)
  /*
   * The query the *user* has typed, readable from the frame handler without re-subscribing.
   *
   * The handler is installed once, at open, and closes over whatever `query` was then. A
   * ref is the only way it can compare an arriving frame against the current query, and
   * without that comparison a slow frame for `ma` repaints the list after the user has
   * already typed `main` — the classic out-of-order search render.
   */
  const liveQuery = useRef('')

  useEffect(() => {
    let cancelled = false
    let opened: string | null = null

    void pendingCommand(
      'picker_open',
      () =>
        pickerApi.open(project, 'files', (incoming) => {
          if (incoming.query !== liveQuery.current) return
          setFrame(incoming)
        }),
      null,
    ).then((id) => {
      if (id === null) {
        if (!cancelled) setFailed(true)
        return
      }
      opened = id
      if (cancelled) {
        // Opened after the overlay was already dismissed. Close it rather than leak the
        // session: the Rust side is holding a walker and a channel for a window that is
        // gone, and nothing else will ever ask it to stop.
        void pendingCommand('picker_close', () => pickerApi.close(id), undefined)
        return
      }
      setSession(id)
    })

    return () => {
      cancelled = true
      const id = opened
      if (id !== null) void pendingCommand('picker_close', () => pickerApi.close(id), undefined)
    }
  }, [project])

  useEffect(() => {
    liveQuery.current = query
    if (session === null) return
    void pendingCommand('picker_query', () => pickerApi.query(session, query), undefined)
  }, [session, query])

  const hits: PickerHit[] = useMemo(() => frame?.hits ?? [], [frame])

  const virtualizer = useVirtualizer({
    count: hits.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  })

  // Keep the selection on screen. `scrollToIndex` is a no-op when the row is already
  // visible, so this does not fight a user who is scrolling with the wheel.
  useEffect(() => {
    if (hits.length > 0) virtualizer.scrollToIndex(Math.min(selected, hits.length - 1))
  }, [selected, hits.length, virtualizer])

  const accept = useCallback(
    (modifier: 'plain' | 'shift' | 'alt' | 'ctrl') => {
      const hit = hits[selected]
      if (!hit) return
      onDismiss()
      if (modifier === 'shift') onOpenInSplit(hit.path)
      else if (modifier === 'alt') onMention(hit.path)
      else onOpen(hit.path)
    },
    [hits, selected, onDismiss, onOpen, onOpenInSplit, onMention],
  )

  const onKeyDown = useCallback(
    (ev: React.KeyboardEvent<HTMLInputElement>) => {
      const action = listAction(ev, hits.length, selected)
      if (!isListKey(action)) return
      // Claimed keys never also move the text caret: Home and End belong to the list here.
      ev.preventDefault()
      if (action.kind === 'select') setSelected(action.index)
      else if (action.kind === 'dismiss') onDismiss()
      else if (action.kind === 'accept') accept(action.modifier)
    },
    [hits.length, selected, onDismiss, accept],
  )

  const total = frame?.total ?? 0
  const matched = frame?.matched ?? hits.length

  return (
    <ModalShell
      label="Go to file"
      prompt="›"
      value={query}
      placeholder="Search files by name"
      counter={failed ? undefined : matchCounter(matched, total)}
      onChange={(next) => {
        setQuery(next)
        // A new query is a new result list, and keeping the old index would leave the
        // highlight on whatever row happens to land there — a different file.
        setSelected(0)
      }}
      onKeyDown={onKeyDown}
      onDismiss={onDismiss}
      scrollRef={scrollRef}
      footer={
        <>
          <Hint keys="↑↓">navigate</Hint>
          <Hint keys="⏎">open</Hint>
          <Hint keys="⇧⏎">open in split</Hint>
          <Hint keys="⌥⏎">send path to Claude</Hint>
        </>
      }
    >
      {failed ? (
        <div className={styles.status}>
          File index unavailable — <code>picker_open</code> is not registered in this build.
        </div>
      ) : hits.length === 0 ? (
        <div className={styles.status}>
          {frame?.indexing === true ? 'Indexing…' : query === '' ? 'Type to search' : 'No matches'}
        </div>
      ) : (
        <div className={styles.viewport} style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((item) => {
            const hit = hits[item.index]
            if (!hit) return null
            const badge = kindBadge(hit.name)
            const tone = TONE_CLASS[badge.tone] ?? styles.toneFaint
            return (
              <div
                key={hit.path}
                className={item.index === selected ? `${styles.row} ${styles.rowSelected}` : styles.row}
                data-audit="pickerRow"
                style={{ height: `${item.size}px`, transform: `translateY(${item.start}px)` }}
                onMouseMove={() => setSelected(item.index)}
                onMouseDown={(ev) => {
                  // `mousedown`, not `click`: the field would lose focus between the two and
                  // the shell's `selectionchange` listener would repaint the caret first.
                  ev.preventDefault()
                  accept(ev.shiftKey ? 'shift' : ev.altKey ? 'alt' : 'plain')
                }}
              >
                <span className={`${styles.badge} ${tone}`}>{badge.label}</span>
                <span className={styles.name}>{hit.name}</span>
                <span className={styles.path}>{hit.relative}</span>
              </div>
            )
          })}
        </div>
      )}
    </ModalShell>
  )
}
