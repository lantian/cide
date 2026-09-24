/**
 * Go to Symbol in project — IDEA's Ctrl+Alt+Shift+N.
 *
 * A near-copy of `FilePicker`, **with the poll intact**, because the candidate set is a
 * repository and the walk is still running while the user types. Everything that differs from
 * `StructurePicker` follows from that one fact.
 *
 * The index is built lazily: `symbols.index` is called once when this opens, is idempotent, and a
 * user who never presses this chord never pays for a symbol walk of their repository.
 */
import { PickerRow, PickerStatus } from '@/kit/components/Overlay'
import { useEffect, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'

import { isNoIndex } from '@/store/fileIndex'
import { pendingCommand, symbols as symbolsApi } from '@/ipc/client'
import type { ProjectId, SymbolFrame } from '@/ipc/client'
import styles from './Overlay.module.css'
import { Hint, ModalShell } from './ModalShell'
import { groupDigits, matchCounter, symbolBadge } from './format'
import { listAction } from './listKeys'
import { scaledRow, useUiScale } from '@/settings/useUiScale'

const ROW_HEIGHT = 26
/** How often to ask again while the walk is still injecting. Matches `FilePicker`. */
const POLL_MS = 120

export interface SymbolPickerProps {
  project: ProjectId
  onDismiss: () => void
  onGoTo: (path: string, line: number, column: number, endColumn: number) => void
}

export function SymbolPicker({ project, onDismiss, onGoTo }: SymbolPickerProps) {
  const [query, setQuery] = useState('')
  const [frame, setFrame] = useState<SymbolFrame | null>(null)
  const [selected, setSelected] = useState(0)
  const [failed, setFailed] = useState(false)
  const [awaitingIndex, setAwaitingIndex] = useState(false)
  /** Bumped per query, so an answer that arrives after the user typed on is discarded. */
  const generation = useRef(0)

  // Idempotent, and the only thing that ever starts the walk. Deliberately here rather than at
  // project open: a symbol walk of a large repository is real work for a surface most sessions
  // never open.
  useEffect(() => {
    void symbolsApi.index(project).catch(() => {})
  }, [project])

  useEffect(() => {
    generation.current += 1
    const mine = generation.current
    let timer: ReturnType<typeof setTimeout> | undefined

    const poll = () => {
      void pendingCommand<SymbolFrame | 'notIndexedYet' | null>(
        'symbol_query',
        () => symbolsApi.query(project, query),
        null,
      )
        .then((incoming) => {
          // Three outcomes, kept apart — the same distinction `FilePicker` draws. A frame, a
          // walk that has not started, and a handler this build does not have are three
          // different things, and rendering all of them as "no results" is how a picker comes
          // to claim a repository has no symbols in it.
          if (mine !== generation.current) return
          if (incoming === null) {
            setFailed(true)
            return
          }
          if (incoming === 'notIndexedYet') {
            setAwaitingIndex(true)
            timer = setTimeout(poll, POLL_MS)
            return
          }
          setAwaitingIndex(false)
          setFrame(incoming)
          setSelected(0)
          if (incoming.running) timer = setTimeout(poll, POLL_MS)
        })
        .catch((error: unknown) => {
          if (mine !== generation.current) return
          if (isNoIndex(error)) {
            setAwaitingIndex(true)
            timer = setTimeout(poll, POLL_MS)
            return
          }
          setFailed(true)
        })
    }
    poll()
    return () => {
      if (timer !== undefined) clearTimeout(timer)
    }
  }, [project, query])

  const rows = frame?.items ?? []
  const viewport = useRef<HTMLDivElement>(null)
  // The chrome scale, for the one measurement CSS never sees: a virtualized row is placed by
  // an absolute transform off this number. See `settings/useUiScale.ts`.
  const rowHeight = scaledRow(ROW_HEIGHT, useUiScale())

  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => viewport.current,
    estimateSize: () => rowHeight,
    overscan: 8,
  })
  useEffect(() => {
    if (rows.length > 0) virtual.scrollToIndex(selected, { align: 'auto' })
  }, [selected, rows.length, virtual])

  const accept = (index: number) => {
    const row = rows[index]
    if (row === undefined) return
    onDismiss()
    onGoTo(row.path, row.line, row.column, row.endColumn)
  }

  const onKeyDown = (event: React.KeyboardEvent) => {
    const action = listAction(event, rows.length, selected)
    if (action.kind === 'none') return
    event.preventDefault()
    if (action.kind === 'select') setSelected(action.index)
    else if (action.kind === 'dismiss') onDismiss()
    else if (action.kind === 'accept') accept(selected)
  }

  return (
    <ModalShell
      label="Go to symbol"
      prompt="#"
      value={query}
      onChange={setQuery}
      onKeyDown={onKeyDown}
      onDismiss={onDismiss}
      counter={frame === null ? undefined : matchCounter(rows.length, frame.matched)}
      scrollRef={viewport}
      footer={
        <>
          <Hint keys="↑↓">navigate</Hint>
          <Hint keys="⏎">go to</Hint>
          <Hint keys="esc">dismiss</Hint>
        </>
      }
    >
      <Body
        failed={failed}
        awaiting={awaitingIndex}
        frame={frame}
        query={query}
        rows={rows.length}
      />
      {rows.length > 0 && (
        <div style={{ height: virtual.getTotalSize(), position: 'relative' }}>
          {virtual.getVirtualItems().map((item) => {
            const row = rows[item.index]
            if (row === undefined) return null
            const badge = symbolBadge(row.kind)
            return (
              <PickerRow
                key={`${row.path}:${row.line}:${row.name}`}
                data-audit="symbolRow"
                virtual
                selected={item.index === selected}
                style={{
                  height: ROW_HEIGHT,
                  transform: `translateY(${item.start}px)`,
                }}
                onMouseMove={() => setSelected(item.index)}
                onMouseDown={(event) => {
                  event.preventDefault()
                  accept(item.index)
                }}
              >
                <span className={`${styles.badge} ${styles[badge.tone] ?? ''}`}>{badge.label}</span>
                <span className={styles.name}>{row.name}</span>
                {/*
                  * The container and the location, told apart.
                  *
                  * They used to be two adjacent `.path` spans — both `flex: 1`, both mono, both
                  * 11px, both `--dim` — which read as one run of grey text with no boundary in
                  * it. Same class of defect as the usage heading two files over, found in the
                  * same pass and fixed the same way: the one that identifies the row keeps the
                  * width, and the one that locates it shrinks first and dims further.
                  */}
                {row.container !== null && (
                  <span className={styles.container}>{row.container}</span>
                )}
                <span className={styles.where}>
                  {row.rel}:{row.line}
                </span>
              </PickerRow>
            )
          })}
        </div>
      )}
    </ModalShell>
  )
}

function Body({
  failed,
  awaiting,
  frame,
  query,
  rows,
}: {
  failed: boolean
  awaiting: boolean
  frame: SymbolFrame | null
  query: string
  rows: number
}) {
  if (failed) {
    return <PickerStatus>Symbol search is not available in this build.</PickerStatus>
  }
  // "The walk has not started" is not "the walk found nothing". Rendering the first as the
  // second would tell the user their repository declares no functions.
  if (awaiting || frame === null) return <PickerStatus>Indexing symbols…</PickerStatus>
  if (rows > 0) {
    return frame.truncated ? (
      <PickerStatus>
        Showing the first {groupDigits(frame.total)} symbols — the index stopped at its cap.
      </PickerStatus>
    ) : null
  }
  if (query.trim() === '') {
    return (
      <PickerStatus>
        {frame.running ? 'Indexing symbols…' : 'Type to search this project’s symbols'}
      </PickerStatus>
    )
  }
  return <PickerStatus>{frame.running ? 'Indexing symbols…' : 'No matches'}</PickerStatus>
}
