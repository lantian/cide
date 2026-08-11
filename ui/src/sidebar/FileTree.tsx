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
 *
 * The status tags come from a **second, independent** source: `gitStatusStore`, over
 * `git_tree_status`. A row's status is not a field on `TreeRow` — `cide-fs` walks the
 * filesystem and knows nothing about git — and keeping the two fetches apart is what lets the
 * tree paint before git has answered. A repository with a slow `git status` shows an untagged
 * tree that gains tags, never an empty pane.
 */
import { useEffect, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useShallow } from 'zustand/react/shallow'
import { useFileTree } from './treeStore'
import { useGitStatus } from './gitStatusStore'
import { letterFor, statusAt } from './treeStatus'
import { isDegraded, type TreeRow, type TreeStatus, type TreeStatusMap } from '@/ipc/client'
import styles from './FileTree.module.css'

/** 21px rows, from the mock. */
const ROW_HEIGHT = 21

/** The `fs_*` calls the panel makes, in the order the notice should prefer to name them. */
const PENDING_COMMANDS = ['fs_tree_count', 'fs_tree_rows', 'fs_expand', 'fs_collapse'] as const

/** 12px per depth, from the mock. */
const INDENT = 12

/**
 * Which colour class a status paints the name and the letter with.
 *
 * The *letter* is not here — `letterFor()` in `treeStatus.ts` owns that, because whether a row
 * gets one depends on its kind as well as its status, and that rule is worth testing under
 * node rather than asserting about in a screenshot.
 *
 * `deleted` is the one entry with two different classes. The name is struck through, from the
 * mock; the letter is not, because a one-glyph `D` with a rule through it at 10.5px is
 * unreadable.
 */
interface StatusStyle {
  /** Applied to the filename. Carries the line-through for a deleted path. */
  nameClass: string | undefined
  /** Applied to the letter. */
  letterClass: string | undefined
}

const CLEAN: StatusStyle = { nameClass: undefined, letterClass: undefined }

const STATUS: Readonly<Record<TreeStatus, StatusStyle>> = {
  clean: CLEAN,
  modified: { nameClass: styles.statusModified, letterClass: styles.statusModified },
  added: { nameClass: styles.statusAdded, letterClass: styles.statusAdded },
  deleted: { nameClass: styles.statusDeleted, letterClass: styles.tagDeleted },
  untracked: { nameClass: styles.statusUntracked, letterClass: undefined },
  ignored: { nameClass: styles.statusIgnored, letterClass: undefined },
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
  /*
   * Subscribed at the panel and threaded down rather than read inside `Row`. Two reasons:
   * the rows are not memoized, so a per-row subscription would be one zustand listener per
   * visible row torn down and rebuilt on every scroll tick; and the map arriving has to
   * repaint the rows already on screen, which only a re-render of this component does.
   *
   * Note what is *not* here: any wait. `count` and `chunks` come from `treeStore` and paint
   * on their own schedule, so a slow `git status` costs late tags and never a late tree.
   *
   * `useShallow` because the *map* is what matters and `gitStatusStore` installs a fresh
   * object on every refresh, equal or not — and it refreshes on every `cide://fs-changed`,
   * which is exactly the burst `treeStore.refresh` now answers with no re-render at all. A
   * plain identity selector would put that re-render straight back. The comparison is over the
   * changed paths, which the backend caps; it is not a walk of the repository.
   */
  const statuses = useGitStatus(useShallow((s) => s.status.statuses))
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
        {/*
          * Keyed by the virtualizer's key — the row *index* — and not by `row.path`.
          *
          * A virtualized list's children are positions, not identities: the box at index 42
          * sits at `42 * ROW_HEIGHT` whatever file happens to be there, so the index is what
          * two renders have in common. Keying by path meant that a file appearing at the top
          * of the tree renamed every key below it, and React answered a burst that moved
          * nothing on screen by unmounting and remounting the visible window instead of
          * rewriting one row's text. The placeholders shared in that: `pending-42` and the
          * path of the row that replaced it are two different children at one position.
          */}
        {items.map((item) => {
          const row = useFileTree.getState().rowAt(item.index)
          if (!row) {
            // The chunk is still in flight. The box keeps its height so the scrollbar does
            // not resize under the user's thumb as rows arrive.
            return (
              <div
                key={item.key}
                className={styles.placeholder}
                style={{ height: `${item.size}px`, transform: `translateY(${item.start}px)` }}
              />
            )
          }
          return (
            <Row
              key={item.key}
              row={row}
              statuses={statuses}
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
  statuses: TreeStatusMap['statuses']
  top: number
  height: number
  selected: boolean
  onSelect: () => void
  onOpen: ((path: string) => void) | undefined
}

function Row({ row, statuses, top, height, selected, onSelect, onOpen }: RowProps) {
  const isDir = row.kind === 'dir'
  const tone = statusAt(statuses, row.path)
  const letter = letterFor(tone, isDir)
  /*
   * `?? CLEAN` even though `TreeStatus` says the lookup is total. The compiler is checking a
   * generated type against a value that arrived over IPC, not the value itself: a Rust variant
   * added to the enum without regenerating — or an older backend against a newer webview —
   * lands here as a plain string, and reading `.nameClass` off `undefined` throws *inside a
   * render*, which unmounts the whole tree rather than mis-drawing one row.
   */
  const status = STATUS[tone] ?? CLEAN
  const twisty = isDir && row.hasChildren ? (row.expanded ? '▾' : '▸') : ''

  return (
    <div
      className={selected ? `${styles.row} ${styles.rowSelected}` : styles.row}
      data-audit="fileTreeRow"
      data-depth={row.depth}
      style={{ height: `${height}px`, transform: `translateY(${top}px)` }}
      title={row.path}
      onMouseDown={() => {
        onSelect()
        if (isDir) void useFileTree.getState().toggle(row)
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
        className={isDir ? `${styles.glyph} ${styles.glyphDir}` : styles.glyph}
        aria-hidden="true"
      >
        {isDir ? '▤' : '▫'}
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
        /*
         * Labelled from the *status*, not from the letter. Colour is the only signal a
         * directory rollup, an untracked file and an ignored file have — none of them draws a
         * letter — so keying the label off the glyph would leave exactly the rows with no
         * visual text as the rows with no accessible text either. It stays on this span rather
         * than the name so a screen reader does not read it as part of the filename.
         */
        aria-label={tone === 'clean' ? undefined : `git status ${tone}`}
      >
        {letter}
      </span>
    </div>
  )
}
