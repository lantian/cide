/**
 * The virtualized 21px file tree.
 *
 * Virtualized for the same reason the rows are windowed on the wire: a 100k-file repository
 * has 100k rows, and neither the DOM nor the IPC boundary should ever see more than a screen
 * of them. `@tanstack/react-virtual` is headless, so the row markup below is the mock's and
 * not a library's.
 *
 * Multi-root projects need no special case here. Rust contributes one depth-0 row per root,
 * so a second root is simply another top-level row in the same flattened list — which is
 * also why the indentation is computed from `row.depth` rather than from any nesting the
 * renderer tracks itself.
 */
import { useEffect, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useFileTree } from './treeStore'
import { isDegraded, type TreeRow, type TreeStatus } from '@/ipc/client'
import styles from './FileTree.module.css'

/** 21px rows, from the mock. */
const ROW_HEIGHT = 21

/** The `fs_*` calls the panel makes, in the order the notice should prefer to name them. */
const PENDING_COMMANDS = ['fs_tree_count', 'fs_tree_rows', 'fs_expand', 'fs_collapse'] as const

/** 12px per depth, from the mock. */
const INDENT = 12

/**
 * The status letter and the class that colours it.
 *
 * `untracked` and `ignored` are beyond the mock, which draws neither — the plan lists only
 * M, A, D and clean. They are given the quietest treatment that is still distinguishable
 * (dim, and faint) and no letter, rather than being invented into the tag column: a `?` the
 * mock does not have would be a decision made here rather than transcribed.
 */
interface StatusStyle {
  /** The letter in the right-aligned tag column. Empty for a row that has none. */
  letter: string
  /** Applied to the filename. Carries the line-through for a deleted path. */
  nameClass: string | undefined
  /** Applied to the letter. Never struck through — a struck-out `D` is unreadable. */
  letterClass: string | undefined
}

const CLEAN: StatusStyle = { letter: '', nameClass: undefined, letterClass: undefined }

const STATUS: Readonly<Record<TreeStatus, StatusStyle>> = {
  clean: CLEAN,
  modified: { letter: 'M', nameClass: styles.statusModified, letterClass: styles.statusModified },
  added: { letter: 'A', nameClass: styles.statusAdded, letterClass: styles.statusAdded },
  deleted: { letter: 'D', nameClass: styles.statusDeleted, letterClass: styles.tagDeleted },
  untracked: { letter: '', nameClass: styles.statusUntracked, letterClass: undefined },
  ignored: { letter: '', nameClass: styles.statusIgnored, letterClass: undefined },
}

export interface FileTreeProps {
  /** Called on a file row. Directory rows expand instead. */
  onOpen?: ((path: string) => void) | undefined
}

export function FileTree({ onOpen }: FileTreeProps) {
  const count = useFileTree((s) => s.count)
  const chunks = useFileTree((s) => s.chunks)
  const degraded = useFileTree((s) => s.degraded)
  const revealTo = useFileTree((s) => s.revealTo)
  const [selected, setSelected] = useState<string | null>(null)

  const scrollRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  })

  const items = virtualizer.getVirtualItems()
  const first = items[0]?.index ?? 0
  const last = items[items.length - 1]?.index ?? 0

  // Fetching belongs in an effect, not in render: `ensure` starts IPC calls that resolve
  // into `set`, and a store write triggered from inside a render is the React warning about
  // updating one component while rendering another — here it would be the tree updating
  // itself mid-commit.
  useEffect(() => {
    if (count > 0) useFileTree.getState().ensure(first, last + 1)
  }, [count, first, last, chunks])

  useEffect(() => {
    if (revealTo === null) return
    // `center`, not `auto`: a reveal is a jump to somewhere the user was not looking, and a
    // row scrolled to the very bottom edge of the panel is technically visible and
    // practically missed.
    virtualizer.scrollToIndex(revealTo, { align: 'center' })
    useFileTree.getState().clearReveal()
  }, [revealTo, virtualizer])

  if (degraded && count === 0) {
    // Name the command that actually failed rather than a fixed one: `degraded` is set from
    // whichever of these `pendingCommand` saw reject first, and a notice that always says
    // `fs_tree_count` would send a reader to the wrong handler.
    const missing = PENDING_COMMANDS.find(isDegraded) ?? 'fs_tree_count'
    return (
      <div className={styles.notice}>
        File tree unavailable — <span className={styles.noticeCode}>{missing}</span> is not
        registered in this build.
      </div>
    )
  }

  return (
    <div className={styles.scroll} ref={scrollRef} data-audit="fileTreeScroll">
      <div className={styles.viewport} style={{ height: `${virtualizer.getTotalSize()}px` }}>
        {items.map((item) => {
          const row = useFileTree.getState().rowAt(item.index)
          if (!row) {
            // The chunk is still in flight. The box keeps its height so the scrollbar does
            // not resize under the user's thumb as rows arrive.
            return (
              <div
                key={`pending-${item.index}`}
                className={styles.placeholder}
                style={{ height: `${item.size}px`, transform: `translateY(${item.start}px)` }}
              />
            )
          }
          return (
            <Row
              key={row.path}
              row={row}
              top={item.start}
              height={item.size}
              selected={row.path === selected}
              onSelect={() => setSelected(row.path)}
              onOpen={onOpen}
            />
          )
        })}
      </div>
    </div>
  )
}

interface RowProps {
  row: TreeRow
  top: number
  height: number
  selected: boolean
  onSelect: () => void
  onOpen: ((path: string) => void) | undefined
}

function Row({ row, top, height, selected, onSelect, onOpen }: RowProps) {
  /*
   * `?? CLEAN` even though the type says the lookup is total. `TreeStatus` is this side's
   * guess at a DTO `cide-ipc` has not declared yet, so the compiler is checking the guess and
   * not the wire: a Rust variant this table does not name (`renamed`, `conflicted`) arrives as
   * a plain string, and `status.letter` on `undefined` throws *inside a render*, which unmounts
   * the whole tree rather than mis-drawing one row.
   */
  const status = STATUS[row.status] ?? CLEAN
  const twisty = row.isDir && row.hasChildren ? (row.expanded ? '▾' : '▸') : ''

  return (
    <div
      className={selected ? `${styles.row} ${styles.rowSelected}` : styles.row}
      data-audit="fileTreeRow"
      data-depth={row.depth}
      style={{ height: `${height}px`, transform: `translateY(${top}px)` }}
      title={row.path}
      onMouseDown={() => {
        onSelect()
        if (row.isDir) void useFileTree.getState().toggle(row)
        else onOpen?.(row.path)
      }}
    >
      {/* Indentation is a margin on the twisty rather than padding on the row, so the
          selection band still spans the panel at any depth. */}
      <span className={styles.twisty} style={{ marginLeft: `${row.depth * INDENT}px` }} aria-hidden="true">
        {twisty}
      </span>
      {/*
       * Literal characters, as everywhere else in this chrome: no icon font or SVG set is
       * bundled, and `▤` is the same mark the activity rail uses for Files, so the two
       * surfaces agree about what a directory looks like.
       */}
      <span
        className={row.isDir ? `${styles.glyph} ${styles.glyphDir}` : styles.glyph}
        aria-hidden="true"
      >
        {row.isDir ? '▤' : '▫'}
      </span>
      <span
        className={
          status.nameClass === undefined ? styles.name : `${styles.name} ${status.nameClass}`
        }
      >
        {row.name}
      </span>
      <span
        className={
          status.letterClass === undefined ? styles.tag : `${styles.tag} ${status.letterClass}`
        }
        /* The letter is the only signal for a colour-blind user, and it is not text a screen
           reader should read as part of the filename. */
        aria-label={status.letter === '' ? undefined : `status ${status.letter}`}
      >
        {status.letter}
      </span>
    </div>
  )
}
