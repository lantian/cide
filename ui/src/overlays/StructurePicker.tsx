/**
 * The File Structure popup — IDEA's Ctrl+F12.
 *
 * # Why this one does not poll
 *
 * `FilePicker` polls because its candidate set is a *repository* and the walk is still running
 * while the user types. A file's member list is a few hundred entries and is already in the
 * window before the first keystroke, so filtering happens here with `searchCommands` — the same
 * scorer the command palette uses. That is the split `overlays/score.ts` already argues for.
 *
 * # Flat rows, indented, rather than a tree
 *
 * IDEA's popup is a filterable tree, and filtering a tree *flattens* it — which is what IDEA
 * itself does the moment you type. What a tree adds over an indented flat list is expand/collapse,
 * which is state to persist and which the fixed-height virtualizer would have to be taught. The
 * sibling context a tree gives — an `impl` block's methods sitting contiguously under it — falls
 * out of document order plus indentation with no state at all.
 *
 * The cost, stated: no collapse, so a 400-method file is a 400-row scroll. The filter is the
 * mitigation, and it is what the popup is for.
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'

import { focusedCaret } from '@/editor/caretTrack'
import { enclosingIndex, flattenOutline, type OutlineNode } from '@/editor/memberNav'
import { fetchOutline, outlineOf, subscribeOutlines, symbolsOf } from '@/editor/outlineStore'
import type { FileOutline, ProjectId } from '@/ipc/client'
import styles from './Overlay.module.css'
import { Hint, ModalShell } from './ModalShell'
import { basename, matchCounter, symbolBadge } from './format'
import { listAction } from './listKeys'
import { searchCommands } from './score'

/** Fixed, so the virtualizer never has to measure. The same height every picker row uses. */
const ROW_HEIGHT = 26

export interface StructurePickerProps {
  project: ProjectId
  onDismiss: () => void
  /** Jump the caret. Wired to `requestReveal` + `file.open` by the host. */
  onGoTo: (path: string, line: number, column: number, endColumn: number) => void
}

interface Row {
  node: OutlineNode
  depth: number
  /** Index in the unfiltered list, so preselection survives the flatten. */
  index: number
}

export function StructurePicker({ project, onDismiss, onGoTo }: StructurePickerProps) {
  const caret = useMemo(() => focusedCaret(), [])
  const path = caret?.path ?? null

  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState(0)
  // A counter rather than the outline itself: the store is module-level and mutable, so a
  // subscription that compared object identity would miss a re-parse that produced an equal tree.
  const [, bump] = useState(0)
  useEffect(() => subscribeOutlines(() => bump((n) => n + 1)), [])

  useEffect(() => {
    // Only when nothing has one yet. `EditorPane` feeds the store with the buffer's *text*; this
    // is the fallback for a path no pane is feeding, and it deliberately passes no text so Rust
    // reads the file from disk. Passing `''` here — which an earlier draft did — would have
    // parsed an empty string and reported every file as declaring nothing.
    if (path === null || outlineOf(path) !== null) return
    fetchOutline(project, path)
  }, [project, path])

  const outline: FileOutline | null = path === null ? null : outlineOf(path)
  const all = useMemo(
    () => (path === null ? [] : flattenOutline(symbolsOf(path))),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the store is the dependency, and
    // the counter above is how a change in it reaches here.
    [path, outline],
  )

  const rows: Row[] = useMemo(() => {
    const numbered: Row[] = all.map((r, index) => ({ node: r.node, depth: r.depth, index }))
    if (query.trim() === '') return numbered
    // `Scorable`'s shape, so typing `fn` finds functions and a container name finds its members —
    // the keyword tier does that for free.
    const scored = searchCommands(
      numbered.map((row) => ({
        id: String(row.index),
        title: row.node.name,
        keywords: [row.node.kind, row.node.container ?? ''],
      })),
      query,
    )
    return scored
      .map((hit) => numbered[Number(hit.id)])
      .filter((row): row is Row => row !== undefined)
  }, [all, query])

  /*
   * IDEA preselects the member the caret is in. Only while the query is empty: once the user is
   * filtering, the first match is what they are looking at, and holding the selection on a row
   * that scrolled away would make ⏎ jump somewhere they cannot see.
   */
  useEffect(() => {
    if (query.trim() !== '') {
      setSelected(0)
      return
    }
    if (caret === null) return
    const at = enclosingIndex(symbolsOf(caret.path), caret.line, caret.column)
    setSelected(at >= 0 ? at : 0)
  }, [query, caret, all.length])

  const viewport = useRef<HTMLDivElement>(null)
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => viewport.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  })
  useEffect(() => {
    if (rows.length > 0) virtual.scrollToIndex(selected, { align: 'auto' })
  }, [selected, rows.length, virtual])

  const accept = (index: number) => {
    const row = rows[index]
    if (row === undefined || path === null) return
    // Dismiss first, like every other picker: acting while the modal is still up leaves the
    // caret behind a scrim for a frame.
    onDismiss()
    onGoTo(path, row.node.selection.startLine, row.node.selection.startColumn, row.node.selection.endColumn)
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
      label={path === null ? 'File structure' : `File structure — ${basename(path)}`}
      prompt="⌗"
      value={query}
      onChange={setQuery}
      onKeyDown={onKeyDown}
      onDismiss={onDismiss}
      counter={matchCounter(rows.length, all.length)}
      scrollRef={viewport}
      footer={
        <>
          <Hint keys="↑↓">navigate</Hint>
          <Hint keys="⏎">go to</Hint>
          <Hint keys="esc">dismiss</Hint>
        </>
      }
    >
      <Body outline={outline} rows={rows} query={query} />
      {rows.length > 0 && (
        <div style={{ height: virtual.getTotalSize(), position: 'relative' }}>
          {virtual.getVirtualItems().map((item) => {
            const row = rows[item.index]
            if (row === undefined) return null
            const badge = symbolBadge(row.node.kind)
            return (
              <div
                key={`${row.index}`}
                data-audit="structureRow"
                className={`${styles.row} ${item.index === selected ? styles.rowSelected : ''}`}
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  height: ROW_HEIGHT,
                  transform: `translateY(${item.start}px)`,
                  // Indentation is inline rather than a class per depth: depth is unbounded and
                  // `FileTree` uses the same idiom for the same reason. Dropped while filtering,
                  // because a filtered list is not a tree any more.
                  paddingLeft: `${12 + (query.trim() === '' ? row.depth * 12 : 0)}px`,
                }}
                onMouseMove={() => setSelected(item.index)}
                // `mousedown`, never `click`: the input would lose focus between the two.
                onMouseDown={(event) => {
                  event.preventDefault()
                  accept(item.index)
                }}
              >
                <span className={`${styles.badge} ${styles[badge.tone] ?? ''}`}>{badge.label}</span>
                <span className={styles.name}>{row.node.name}</span>
                {query.trim() !== '' && row.node.container !== null && (
                  <span className={styles.path}>{row.node.container}</span>
                )}
              </div>
            )
          })}
        </div>
      )}
    </ModalShell>
  )
}

/**
 * The three states, kept apart.
 *
 * `ready` with no members is a *real, reportable zero* — this file declares nothing — and it is
 * a different sentence from "cide cannot parse this file". The same discipline
 * `ProblemsPanel/model.ts` applies to diagnostics, and for the same reason: an empty list that
 * does not say why it is empty is a claim nobody checked.
 */
function Body({
  outline,
  rows,
  query,
}: {
  outline: FileOutline | null
  rows: readonly Row[]
  query: string
}) {
  if (outline === null) return <div className={styles.status}>Reading the file…</div>
  if (outline.kind === 'unsupported' || outline.kind === 'failed') {
    return <div className={styles.status}>{outline.reason}</div>
  }
  if (rows.length > 0) return null
  if (query.trim() !== '') return <div className={styles.status}>No matching members</div>
  return <div className={styles.status}>No members in this file</div>
}
