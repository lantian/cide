/**
 * Find usages, as a list you dismiss in under two seconds.
 *
 * # Why the 620px card and not the Go to line box
 *
 * `GoToLine.module.css`'s header states the rule this follows: the small centred popup is for "one
 * short number and at most one sentence". This is a list of forty rows grouped by file, and it
 * wants the height and the docked position every other picker in this app has. `SymbolPicker` is
 * the template — `ModalShell`, a virtualizer, `listAction`, and a `Body` whose whole job is telling
 * the honest empty states apart.
 *
 * # Why there is no preview pane
 *
 * IDEA's Find Usages tool window has one. Here it would mean a second CodeMirror instance inside a
 * modal, or a second read of every file, for a surface that exists to be looked at and dismissed.
 * The preview is the *row*: the source line, clipped in Rust, with the occurrence marked — which
 * is the same thing a search hit draws, through the same `splitHighlight`, off the same byte
 * offsets. That reuse is why `cide_ipc::Usage` copies `SearchHit`'s three fields.
 *
 * # Why the filter is a substring and not the fuzzy scorer
 *
 * `overlays/score.ts` is a port of a command-title matcher. Run over source lines it produces
 * confident nonsense, because almost any subsequence of characters occurs somewhere in forty lines
 * of code. See `usagesModel.ts`, which holds the predicate and says so.
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'

import { useDiagnostics } from '@/sidebar/diagnosticsStore'
import { groupHits, splitHighlight } from '@/sidebar/SearchModel'
import { basename } from '@/editor/languages'
import { FileIcon, useIconTheme } from '@/icons'
import styles from './Overlay.module.css'
import { Hint, ModalShell } from './ModalShell'
import { matchCounter } from './format'
import { listAction } from './listKeys'
import { fileCount, filterUsages, usagesLabel, usagesStatus } from './usagesModel'
import { cancelUsages, useUsages } from './usagesStore'

const ROW_HEIGHT = 30
const HEADER_HEIGHT = 22

/** No collapsing here: the popup is transient, and a fold state nobody can save is a control that
 * only ever costs a click. `groupHits` takes the set, so an empty frozen one is passed. */
const NOTHING_COLLAPSED: ReadonlySet<string> = new Set<string>()

export interface UsagesPopupProps {
  onDismiss: () => void
  /** ⏎ on a row: put the caret on the occurrence. Same shape as `goToSymbol`. */
  onGoTo: (path: string, line: number, column: number, endColumn: number) => void
}

export function UsagesPopup({ onDismiss, onGoTo }: UsagesPopupProps) {
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState(0)
  const name = useUsages((s) => s.name)
  const searching = useUsages((s) => s.searching)
  const rows = useUsages((s) => s.rows)
  const truncated = useUsages((s) => s.truncated)
  const failed = useUsages((s) => s.failed)
  /* One subscription for the whole list rather than one per heading — the icon set has a second
     *file* per icon for the light theme rather than a CSS filter, so every heading needs it.
     `FileTree` states the argument at length for a list a thousand times longer. */
  const iconTheme = useIconTheme()

  /*
   * What the *server* says it is doing, appended to "Finding usages…".
   *
   * Read from the snapshot the panel already subscribes to and never re-derived. A twenty-second
   * wait with "Indexing…" beside it is explained; the same wait with nothing beside it is
   * indistinguishable from a hang. Note what this is deliberately **not** used for: nothing here
   * gates the *request* on a source being `Ready`. `cide-lsp`'s `READY_SETTLE` exists because
   * rust-analyzer's indexing is a sequence of progress tokens and "nothing in flight" is true in
   * every gap between them — this repository has already shipped one bug reading such a gap as
   * "indexed, no results". The status is for prose. The timeout is the readiness answer.
   */
  const detail = useDiagnostics((s) => {
    const snapshot = s.snapshot
    /*
     * No `kind === 'ready'` gate, and its absence is the fix.
     *
     * The snapshot's own kind is `scanning` whenever ANY enabled source is scanning
     * (`cide-core/src/diagnostics.rs`), so requiring `ready` before looking for a scanning source
     * asked for two states at once. The progress line — the whole reason this popup can say
     * something better than an empty list during a twenty-second wait — was unreachable by
     * construction: the branch could only run when nothing was indexing, and then there is no
     * detail to show.
     *
     * `sources` is read on every kind because it is the same array either way; a source that is
     * not scanning simply does not match below.
     */
    for (const source of snapshot.sources ?? []) {
      if (source.status.kind === 'scanning' && source.status.detail !== '') {
        return source.status.detail
      }
    }
    return null
  })

  const filtered = useMemo(() => filterUsages(rows, query), [rows, query])
  const grouped = useMemo(() => groupHits(filtered, NOTHING_COLLAPSED), [filtered])
  /** Flat-hit index → row index, so the selection can be scrolled to without a search. */
  const rowOfHit = useMemo(() => {
    const map = new Map<number, number>()
    grouped.forEach((row, at) => {
      if (row.kind === 'hit') map.set(row.index, at)
    })
    return map
  }, [grouped])

  useEffect(() => setSelected(0), [query, rows])

  /*
   * Stop the search when this goes away, however it goes away.
   *
   * On unmount rather than in each dismissal path, because there are four of them and one has no
   * call site to put a line in: `OverlayHost` unmounts this component when the project closes.
   * `cancelUsages` releases the blocked Rust thread *and* sends `$/cancelRequest`, so a
   * twenty-second whole-workspace search really does stop rather than merely stopping being
   * watched — the difference between "Escape cancelled it" and "Escape stopped showing it".
   */
  useEffect(() => cancelUsages, [])

  const viewport = useRef<HTMLDivElement>(null)
  const virtual = useVirtualizer({
    count: grouped.length,
    getScrollElement: () => viewport.current,
    estimateSize: (index) => (grouped[index]?.kind === 'file' ? HEADER_HEIGHT : ROW_HEIGHT),
    overscan: 8,
  })
  useEffect(() => {
    const at = rowOfHit.get(selected)
    if (at !== undefined) virtual.scrollToIndex(at, { align: 'auto' })
  }, [selected, rowOfHit, virtual])

  const accept = (index: number) => {
    const row = filtered[index]
    if (row === undefined) return
    // Dismiss first, exactly as every other picker does: `onGoTo` reveals into a buffer, and
    // revealing under a modal that still holds focus is how a caret moves and the keyboard does
    // not follow it. `onDismiss` also cancels, so a search still running is stopped.
    onDismiss()
    onGoTo(row.path, row.line, row.column, row.endColumn)
  }

  const onKeyDown = (event: React.KeyboardEvent) => {
    const action = listAction(event, filtered.length, selected)
    if (action.kind === 'none') return
    event.preventDefault()
    if (action.kind === 'select') setSelected(action.index)
    else if (action.kind === 'dismiss') onDismiss()
    else if (action.kind === 'accept') accept(selected)
  }

  const status = usagesStatus({
    searching,
    detail,
    failed,
    total: rows.length,
    shown: filtered.length,
    truncated,
    name,
    query,
  })

  return (
    <ModalShell
      label={usagesLabel(name)}
      prompt="⌕"
      value={query}
      placeholder="Filter by file or line"
      onChange={setQuery}
      onKeyDown={onKeyDown}
      onDismiss={onDismiss}
      counter={rows.length === 0 ? undefined : matchCounter(filtered.length, rows.length)}
      scrollRef={viewport}
      footer={
        <>
          <Hint keys="↑↓">navigate</Hint>
          <Hint keys="⏎">go to</Hint>
          <Hint keys="esc">dismiss</Hint>
        </>
      }
    >
      {status !== null && <div className={styles.status}>{status}</div>}
      {grouped.length > 0 && (
        <div style={{ height: virtual.getTotalSize(), position: 'relative' }}>
          {virtual.getVirtualItems().map((item) => {
            const row = grouped[item.index]
            if (row === undefined) return null
            const style = {
              position: 'absolute' as const,
              top: 0,
              left: 0,
              width: '100%',
              height: item.size,
              transform: `translateY(${item.start}px)`,
            }
            if (row.kind === 'file') {
              return (
                <div
                  key={`f:${row.path}`}
                  data-audit="usageFile"
                  className={`${styles.row} ${styles.usageFile}`}
                  style={style}
                >
                  {/*
                    * The icon, and then the name in the UI face. Both are the search panel's,
                    * because both lists show the same thing and the one the user already reads
                    * is the one to agree with. `HEADER_HEIGHT` is 22 and `FileIcon` is a 16px
                    * box, so it fits without moving the row.
                    */}
                  <FileIcon row={{ name: basename(row.rel), kind: 'file' }} theme={iconTheme} />
                  <span className={styles.usageFileName}>{row.rel}</span>
                  <span className={styles.usageCount}>{row.hits}</span>
                </div>
              )
            }
            // Read back out of `filtered` rather than off `row.hit`: `groupHits` is generic over
            // the structural `Hit`, so its rows carry only the six fields it needs — and this row
            // has to jump, which wants `column` and `endColumn` too. A cast would compile and
            // would be a claim rather than a fact.
            const usage = filtered[row.index]
            if (usage === undefined) return null
            const parts = splitHighlight(usage.text, usage.start, usage.end)
            return (
              <div
                key={`h:${row.index}`}
                data-audit="usageRow"
                className={`${styles.row} ${row.index === selected ? styles.rowSelected : ''}`}
                style={style}
                onMouseMove={() => setSelected(row.index)}
                onMouseDown={(event) => {
                  event.preventDefault()
                  accept(row.index)
                }}
              >
                <span className={styles.usageAt}>{usage.line}</span>
                <span className={styles.usageText}>
                  {parts.before}
                  <mark className={styles.usageMatch}>{parts.match}</mark>
                  {parts.after}
                </span>
              </div>
            )
          })}
        </div>
      )}
      {!searching && failed === null && rows.length > 0 && (
        <div className={styles.usageSummary} data-audit="usageSummary">
          {rows.length} {rows.length === 1 ? 'usage' : 'usages'} in {fileCount(rows)}{' '}
          {fileCount(rows) === 1 ? 'file' : 'files'}
        </div>
      )}
    </ModalShell>
  )
}
