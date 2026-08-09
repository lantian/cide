/**
 * The 23px-row tri-state checkbox tree.
 *
 * # Markup
 *
 * A flat list of `treeitem`s with explicit `aria-level`, not nested `<ul>`s. The row model
 * is already flat (see `model.ts`) because the checkbox semantics are not local, and
 * rebuilding a DOM hierarchy from it would only exist to be flattened again by the
 * accessibility tree. `aria-level` is what a screen reader actually reads out.
 *
 * The checkbox is `aria-hidden` and the *row* carries `aria-checked`, including `mixed`.
 * One focusable thing per row: a nested `<input>` would make Tab walk two stops per file
 * and leave Space ambiguous between "tick this row" and "tick this box".
 *
 * # Focus
 *
 * Roving tabindex — the tree is one tab stop and the arrows move within it, which is what
 * IDEA does and what the ARIA tree pattern requires. Focus is moved imperatively only in
 * response to a key or a click, never on render: a tree that grabs focus when a background
 * refresh changes its rows would steal the caret out of the commit message box, and this
 * panel refreshes while the user types.
 */
import { useCallback, useRef, useState, type KeyboardEvent } from 'react'
import { checkState, splitPath, type CheckState, type Row } from './model'
import { TriCheckbox } from './TriCheckbox'
import styles from './ChangesTree.module.css'

export interface ChangesTreeProps {
  rows: readonly Row[]
  selected: ReadonlySet<string>
  expanded: ReadonlySet<string>
  /** Ids whose file is only partly staged — what turns a `✓` into a `–`. */
  partial: ReadonlySet<string>
  onToggleCheck: (row: Row) => void
  onToggleExpand: (row: Row) => void
  onOpenDiff: (row: Row) => void
}

const ARIA_CHECKED: Record<CheckState, 'true' | 'false' | 'mixed'> = {
  checked: 'true',
  partial: 'mixed',
  unchecked: 'false',
}

export function ChangesTree({
  rows,
  selected,
  expanded,
  partial,
  onToggleCheck,
  onToggleExpand,
  onOpenDiff,
}: ChangesTreeProps) {
  const container = useRef<HTMLDivElement>(null)
  const [cursor, setCursor] = useState(0)
  // Rows come and go under a live refresh, so the stored cursor is advisory and every read
  // clamps it. Storing a row id instead would have to answer what happens when that row is
  // the one that disappeared, which is the same clamp with more state.
  const at = rows.length === 0 ? 0 : Math.min(cursor, rows.length - 1)

  const move = useCallback((next: number) => {
    setCursor(next)
    container.current?.querySelector<HTMLElement>(`[data-index="${next}"]`)?.focus()
  }, [])

  const onKeyDown = useCallback(
    (e: KeyboardEvent, row: Row, index: number) => {
      const isOpen = expanded.has(row.id)
      switch (e.key) {
        case 'ArrowDown':
          move(Math.min(index + 1, rows.length - 1))
          break
        case 'ArrowUp':
          move(Math.max(index - 1, 0))
          break
        case 'ArrowRight':
          if (row.expandable && !isOpen) onToggleExpand(row)
          else move(Math.min(index + 1, rows.length - 1))
          break
        case 'ArrowLeft':
          if (row.expandable && isOpen) onToggleExpand(row)
          else move(parentOf(rows, index))
          break
        case 'Home':
          move(0)
          break
        case 'End':
          move(rows.length - 1)
          break
        case ' ':
          onToggleCheck(row)
          break
        case 'Enter':
          if (row.expandable) onToggleExpand(row)
          else onOpenDiff(row)
          break
        default:
          // Every other key, printable ones included, belongs to the browser.
          return
      }
      // Only reached when the key was handled: Space and the arrows scroll the panel
      // otherwise, and Enter would submit if this tree ever sits inside a form.
      e.preventDefault()
    },
    [rows, expanded, move, onToggleCheck, onToggleExpand, onOpenDiff],
  )

  if (rows.length === 0) {
    return (
      <div className={styles.tree} data-audit="gitTree">
        <p className={styles.empty}>No changes.</p>
      </div>
    )
  }

  return (
    <div
      ref={container}
      className={styles.tree}
      role="tree"
      aria-label="Changes"
      aria-multiselectable="true"
      data-audit="gitTree"
    >
      {rows.map((row, index) => {
        const state = checkState(row, selected, (id) => partial.has(id))
        const open = expanded.has(row.id)
        return (
          <div
            key={row.id}
            data-index={index}
            data-audit="gitRow"
            data-kind={row.kind}
            role="treeitem"
            aria-level={row.depth + 1}
            aria-checked={ARIA_CHECKED[state]}
            {...(row.expandable ? { 'aria-expanded': open } : {})}
            tabIndex={index === at ? 0 : -1}
            className={row.kind === 'file' ? styles.row : `${styles.row} ${styles.groupRow}`}
            // Indent is a padding rather than a spacer element so the whole 23px row stays
            // one hit target, including the empty space to the left of a deep file.
            style={{ paddingLeft: `${8 + row.depth * 14}px` }}
            onFocus={() => setCursor(index)}
            onClick={() => {
              setCursor(index)
              if (row.expandable) onToggleExpand(row)
            }}
            onDoubleClick={() => {
              if (!row.expandable) onOpenDiff(row)
            }}
            onKeyDown={(e) => onKeyDown(e, row, index)}
          >
            <span className={styles.twisty} aria-hidden="true">
              {row.expandable ? (open ? '▾' : '▸') : ''}
            </span>

            {/*
              A click on the box must tick, not expand: a group's own row toggles open on
              click, and without stopping propagation every tick would also collapse the
              group it was in.
            */}
            <span
              className={styles.checkHit}
              onClick={(e) => {
                e.stopPropagation()
                setCursor(index)
                onToggleCheck(row)
              }}
            >
              <TriCheckbox state={state} />
            </span>

            {row.kind === 'file' ? <FileLabel row={row} /> : <GroupLabel row={row} />}
          </div>
        )
      })}
    </div>
  )
}

/**
 * A changelist, a repo or a submodule: name, then its file count.
 *
 * The active changelist is the one a commit defaults to, so it is the one name in this
 * tree that is drawn in `--text` rather than `--dim`.
 */
function GroupLabel({ row }: { row: Row }) {
  return (
    <>
      <span
        className={styles.groupName}
        data-kind={row.kind}
        data-active={row.active === true ? '' : undefined}
      >
        {row.label}
      </span>
      {row.count !== undefined && <span className={styles.count}>{row.count}</span>}
    </>
  )
}

/**
 * `name` in the status colour, then its directory dimmed.
 *
 * The colours are the explorer's, deliberately: `M` blue, `A` green, `D` faint and struck
 * through. A file that is blue in the file tree and some other colour here would read as
 * two different pieces of information about the same file.
 */
function FileLabel({ row }: { row: Row }) {
  const file = row.file
  if (file === undefined) return null
  const { name, dir } = splitPath(file.path)
  return (
    <>
      <span className={styles.fileName} data-status={file.status} title={file.path}>
        {name}
      </span>
      {dir !== '' && <span className={styles.dir}>{dir}</span>}
      {file.originalPath !== undefined && (
        <span className={styles.dir}>← {splitPath(file.originalPath).name}</span>
      )}
      {file.partial === true && (
        // Says out loud what the `–` box means for a leaf, which is otherwise the one
        // tri-state in the tree with no second row to compare against.
        <span className={styles.partialTag} title="Only some hunks of this file are staged">
          partial
        </span>
      )}
    </>
  )
}

/**
 * The row index of the nearest ancestor, or the row itself when it is a root.
 *
 * Left-arrow on a collapsed row goes to its parent; with a flat row list that is "walk
 * back to the first row with a smaller depth".
 */
function parentOf(rows: readonly Row[], index: number): number {
  const depth = rows[index]?.depth ?? 0
  for (let i = index - 1; i >= 0; i--) {
    const candidate = rows[i]
    if (candidate && candidate.depth < depth) return i
  }
  return index
}
