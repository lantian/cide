/**
 * The 252px search panel: the query input, the three toggles, and the results.
 *
 * Same width and the same row metrics as the Explorer — 252px, 24px proportional rows — because the
 * two are the same piece of chrome with different contents, and a sidebar that changes width
 * when the rail's selection changes reads as a layout bug. `FileTree.module.css` owns those
 * numbers for the tree; this panel restates them rather than importing that stylesheet,
 * because a CSS module is a component's private surface and sharing one would make every
 * change to the tree's rows a change to this panel's.
 *
 * Virtualized for the same reason the tree is: a search over a large repository caps out at
 * 5 000 hits, and neither the DOM nor a repaint should ever see more than a screen of them.
 *
 * Everything that can be got wrong quietly — the grouping, the byte-offset highlight, the
 * state machine, the readout, the caret position a hit resolves to — lives in
 * `SearchModel.ts` and is tested by `ui/scripts/check-search.mjs`. What is left here is
 * markup and event wiring.
 *
 * # A result is a place, so clicking one goes there
 *
 * > *"search result click doesn't point me to found place (should open file and select the
 * > line)"*
 *
 * A single click opens, unlike the two trees where a single click only selects — and that is
 * not an inconsistency; see `clickSemantics.ts`. What a click hands the host is the file
 * *and* the caret: `onOpenHit(path, line, column, endColumn)`. The host may still ignore the
 * last three, and today's shell does, which is why the file opening is not conditional on
 * anything: opening at the top of the file beats not opening at all, and it is what this
 * panel already did before it could say where to look.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useSearch } from './SearchStore'
import {
  groupHits,
  hitPosition,
  panelState,
  rowKey,
  splitHighlight,
  summarize,
  trimIndent,
} from './SearchModel'
import type { SearchRow } from './SearchModel'
import { gestureOf, moveIndex, searchClick } from './clickSemantics'
import { copyText } from './copyText'
import { clearFocusRequest, useFocusRequested } from '@/chrome/focusRequests'
import { groupDigits } from '@/overlays/format'
import type { ProjectId } from '@/ipc/client'
import { FileIcon, useIconTheme, type IconTheme } from '@/icons'
import { useContextMenu, type MenuEntry } from '@/menus'
import styles from './SearchPanel.module.css'

/**
 * 24px rows — the same as the file tree's, and for the same reason.
 *
 * Deliberately not the mock's 21: see `FileTree.tsx`, where the number and the argument for
 * departing from `Grount IDE.dc.html` live. It is restated rather than imported because these
 * are two virtualizers with two independent `estimateSize` callbacks; the pairing is a claim
 * about the design, not a dependency. This panel also draws a `<FileIcon>`, so its row shares
 * the parity constraint `icons/FileIcon.module.css` documents.
 */
const ROW_HEIGHT = 24

/**
 * Where a hit is, and what opening it means.
 *
 * Four positional arguments rather than an object, deliberately: the shell already passes
 * `(path) => open(path)` for this prop, and a host that only knows how to open a file must
 * keep compiling. Widening the *object* would have broken that call site — in `App.tsx`,
 * which this package may not edit — and the whole point of the extra arguments is that they
 * are ignorable.
 */
export type OpenHit = (path: string, line: number, column: number, endColumn: number) => void

export interface SearchPanelProps {
  /** The active project, or `null` when none is open. */
  project: ProjectId | null
  /**
   * Open a hit at its match.
   *
   * `line` is 1-based, and so are `column`/`endColumn`, which are UTF-16 columns rather than
   * the wire's byte offsets — see `hitPosition`. A host that cannot place a caret may use the
   * path alone; the file still opens, which is the part the user asked for.
   */
  onOpenHit?: OpenHit | undefined
}

export function SearchPanel({ project, onOpenHit }: SearchPanelProps) {
  const query = useSearch((s) => s.query)
  const hits = useSearch((s) => s.hits)
  const total = useSearch((s) => s.total)
  const files = useSearch((s) => s.files)
  const scanned = useSearch((s) => s.scanned)
  const running = useSearch((s) => s.running)
  const truncated = useSearch((s) => s.truncated)
  const error = useSearch((s) => s.error)
  const degraded = useSearch((s) => s.degraded)
  const collapsed = useSearch((s) => s.collapsed)
  // One subscription for the panel, not one per visible row — the same argument `FileTree`
  // makes about the icon theme, and for the same reason: rows are remounted on every scroll
  // tick, so a hook inside a row is a store listener churned per frame.
  const iconTheme = useIconTheme()

  /*
   * The caret, when a command asked for it.
   *
   * `sidebar.search` (Ctrl+Shift+F) reveals this panel and then asks for its box. It cannot
   * simply focus the input itself: the dispatcher runs inside the key gate's window listener,
   * one React render before this component exists on the common path — and on the *un*common
   * path, where the panel is already open, nothing mounts at all, so an `autoFocus` would
   * answer the first press and silently ignore every one after it. A parked request covers
   * both, because this effect runs on the mount *and* on the re-render. See
   * `chrome/focusRequests.ts`.
   *
   * `select()` as well as `focus()`: a second press replaces the last query rather than
   * appending to it, which is what every editor's find-in-files does and what makes the chord
   * usable as "search for this instead". No `keypress` follows the chord to disturb it — the
   * gate calls `preventDefault` on every stroke it handles before dispatching.
   */
  const input = useRef<HTMLInputElement>(null)
  const focusWanted = useFocusRequested('search')
  useEffect(() => {
    if (!focusWanted) return
    input.current?.focus()
    input.current?.select()
    clearFocusRequest('search')
  }, [focusWanted])

  useEffect(() => {
    useSearch.getState().attach(project)
  }, [project])

  /*
   * Stopping on unmount is not tidiness: the walk is a thread reading a repository, and the
   * panel unmounts every time the user clicks another icon in the activity rail. Without
   * this, switching to Git during a search over a large tree leaves it running to the end.
   */
  useEffect(() => () => useSearch.getState().stop(), [])

  const rows = groupHits(hits, collapsed)
  const state = panelState({ pattern: query.pattern, running, total, error })

  return (
    <div className={styles.panel} data-audit="searchPanel">
      <div className={styles.header}>
        <span className={styles.headerTitle}>Search</span>
        {total > 0 && (
          <span className={styles.headerMeta} data-audit="searchCount">
            {summarize(total, files, truncated, groupDigits)}
          </span>
        )}
      </div>

      <div className={styles.controls}>
        <input
          ref={input}
          className={styles.input}
          data-audit="searchInput"
          type="text"
          value={query.pattern}
          spellCheck={false}
          autoComplete="off"
          placeholder="Search in files"
          aria-label="Search in files"
          onChange={(e) => useSearch.getState().setQuery({ pattern: e.target.value })}
        />
        <div className={styles.toggles} role="group" aria-label="Search options">
          <Toggle
            label="Match case"
            glyph="Aa"
            on={query.caseSensitive}
            onClick={() => useSearch.getState().setQuery({ caseSensitive: !query.caseSensitive })}
          />
          <Toggle
            label="Whole word"
            glyph="ab"
            on={query.wholeWord}
            onClick={() => useSearch.getState().setQuery({ wholeWord: !query.wholeWord })}
          />
          <Toggle
            label="Regular expression"
            glyph=".*"
            on={query.mode === 'regex'}
            onClick={() =>
              useSearch.getState().setQuery({ mode: query.mode === 'regex' ? 'literal' : 'regex' })
            }
          />
        </div>
      </div>

      {degraded ? (
        <div className={styles.notice}>
          Search unavailable — <span className={styles.noticeCode}>search_query</span> is not
          registered in this build.
        </div>
      ) : (
        <Body
          state={state}
          rows={rows}
          error={error}
          scanned={scanned}
          iconTheme={iconTheme}
          onOpenHit={onOpenHit}
        />
      )}
    </div>
  )
}

interface BodyProps {
  state: ReturnType<typeof panelState>
  rows: SearchRow[]
  error: string | null
  scanned: number
  iconTheme: IconTheme
  onOpenHit: OpenHit | undefined
}

/**
 * The four things below the input.
 *
 * Empty, searching and no-results are deliberately three different panels rather than one
 * with three strings in it: the first is an invitation, the second is progress and reports a
 * live figure, and the third is an answer. A search over a large repository spends a second
 * in the second state, and a panel that showed "No results" there would be lying for exactly
 * as long as it takes the user to believe it.
 */
function Body({ state, rows, error, scanned, iconTheme, onOpenHit }: BodyProps) {
  if (state === 'empty') {
    return <div className={styles.hint}>Type to search this project&rsquo;s files.</div>
  }
  if (state === 'error') {
    return (
      <div className={styles.notice} data-audit="searchError">
        <span className={styles.noticeLabel}>Bad pattern</span>
        <span className={styles.noticeCode}>{error}</span>
      </div>
    )
  }
  if (state === 'searching') {
    return (
      <div className={styles.hint} data-audit="searchProgress">
        Searching… <span className={styles.hintMeta}>{groupDigits(scanned)} files</span>
      </div>
    )
  }
  if (state === 'noResults') {
    return (
      <div className={styles.hint} data-audit="searchEmpty">
        No results.
      </div>
    )
  }
  return <Results rows={rows} iconTheme={iconTheme} onOpenHit={onOpenHit} />
}

function Results({
  rows,
  iconTheme,
  onOpenHit,
}: {
  rows: SearchRow[]
  iconTheme: IconTheme
  onOpenHit: OpenHit | undefined
}) {
  const scrollRef = useRef<HTMLDivElement>(null)
  /**
   * The selected row, by key rather than by position.
   *
   * Results stream in while the user reads them and folding a group renumbers everything
   * below it, so a stored index would point at a different line every few hundred
   * milliseconds. See `rowKey`.
   */
  const [selected, setSelected] = useState<string | null>(null)
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  })

  const open = useCallback(
    (row: SearchRow) => {
      if (row.kind === 'file') {
        useSearch.getState().toggleGroup(row.path)
        return
      }
      const at = hitPosition(row.hit)
      onOpenHit?.(row.hit.path, at.line, at.column, at.endColumn)
    },
    [onOpenHit],
  )

  const act = useCallback(
    (row: SearchRow, gesture: 'single' | 'double') => {
      const action = searchClick({ gesture, kind: row.kind })
      if (action.select) setSelected(rowKey(row))
      // A file heading has nothing to open and a hit has nothing to fold, so one call covers
      // both — `open` branches on the row it is given rather than the caller branching twice.
      if (action.toggle || action.open) open(row)
    },
    [open],
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const at = selected === null ? -1 : rows.findIndex((row) => rowKey(row) === selected)
      const next = moveIndex(e.key, at < 0 ? 0 : at, rows.length)
      if (next !== null) {
        // From nothing, any navigation key means the top: `moveIndex` from a notional -1
        // would answer 1 for ArrowDown, which skips the first result.
        const to = at < 0 ? 0 : next
        const row = rows[to]
        if (row !== undefined) setSelected(rowKey(row))
        virtualizer.scrollToIndex(to, { align: 'auto' })
        e.preventDefault()
        return
      }
      const row = at < 0 ? undefined : rows[at]
      if (row === undefined) return
      if (e.key === 'Enter') {
        open(row)
        e.preventDefault()
      } else if (e.key === 'ArrowLeft' && row.kind === 'file' && !row.collapsed) {
        useSearch.getState().toggleGroup(row.path)
        e.preventDefault()
      } else if (e.key === 'ArrowRight' && row.kind === 'file' && row.collapsed) {
        useSearch.getState().toggleGroup(row.path)
        e.preventDefault()
      }
    },
    [open, rows, selected, virtualizer],
  )

  const { onContextMenu, menu } = useContextMenu({
    label: 'Search results',
    items: ({ target }) => {
      const el = target?.closest<HTMLElement>('[data-row-key]')
      const key = el?.dataset['rowKey']
      const row = key === undefined ? undefined : rows.find((r) => rowKey(r) === key)
      // The empty space under the last result opens nothing rather than an empty box.
      if (row === undefined || key === undefined) return []
      setSelected(key)

      const path = row.kind === 'file' ? row.path : row.hit.path
      const entries: MenuEntry[] = [
        {
          id: 'open',
          label: row.kind === 'file' ? (row.collapsed ? 'Expand' : 'Collapse') : 'Open',
          run: () => open(row),
        },
        { kind: 'separator' },
        {
          id: 'copyMatch',
          label: 'Copy Match',
          ...(row.kind === 'file'
            ? // A heading is a file, not a match. Disabled with the reason rather than hidden,
              // so the menu is the same shape on both row kinds and nothing appears to move.
              { disabledReason: 'A file heading has no matched text' }
            : {
                run: () => {
                  // The *untrimmed* hit: `trimIndent` is a display decision for a 252px panel
                  // and copying the panel's rendering rather than the file's bytes is how a
                  // paste comes out subtly different from what was searched for.
                  const parts = splitHighlight(row.hit.text, row.hit.start, row.hit.end)
                  void copyText(parts.match)
                },
              }),
        },
        { id: 'copyPath', label: 'Copy Path', run: () => void copyText(path) },
      ]
      return entries
    },
  })

  return (
    <div
      className={styles.scroll}
      ref={scrollRef}
      data-audit="searchScroll"
      role="tree"
      aria-label="Search results"
      tabIndex={0}
      onKeyDown={onKeyDown}
      onContextMenu={onContextMenu}
    >
      <div className={styles.viewport} style={{ height: `${virtualizer.getTotalSize()}px` }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index]
          if (row === undefined) return null
          const style = {
            height: `${item.size}px`,
            transform: `translateY(${item.start}px)`,
          }
          const key = rowKey(row)
          return row.kind === 'file' ? (
            <FileHeading
              key={key}
              row={row}
              rowKey={key}
              style={style}
              iconTheme={iconTheme}
              selected={key === selected}
              onAct={act}
            />
          ) : (
            <HitLine
              key={key}
              row={row}
              rowKey={key}
              style={style}
              selected={key === selected}
              onAct={act}
            />
          )
        })}
      </div>
      {/* `{menu}` must be rendered or nothing appears; it portals out of this scroll box. */}
      {menu}
    </div>
  )
}

/**
 * The gesture handler both row kinds share.
 *
 * `onMouseDown` rather than `onClick`, and `detail` rather than a timer — the same decision
 * the file tree makes, for the same reason. See `clickSemantics.ts`.
 */
function pressHandler(row: SearchRow, onAct: (row: SearchRow, gesture: 'single' | 'double') => void) {
  return (e: React.MouseEvent) => {
    if (e.button !== 0) return
    onAct(row, gestureOf(e.detail))
  }
}

function FileHeading({
  row,
  rowKey: key,
  style,
  iconTheme,
  selected,
  onAct,
}: {
  row: Extract<SearchRow, { kind: 'file' }>
  rowKey: string
  style: React.CSSProperties
  iconTheme: IconTheme
  selected: boolean
  onAct: (row: SearchRow, gesture: 'single' | 'double') => void
}) {
  return (
    <div
      className={selected ? `${styles.fileRow} ${styles.rowSelected}` : styles.fileRow}
      data-audit="searchFileRow"
      data-row-key={key}
      role="treeitem"
      aria-level={1}
      aria-selected={selected}
      aria-expanded={!row.collapsed}
      style={style}
      title={row.path}
      onMouseDown={pressHandler(row, onAct)}
    >
      <span className={styles.twisty} aria-hidden="true">
        {row.collapsed ? '▸' : '▾'}
      </span>
      {/*
       * The same vendored Material icon the file tree draws, keyed off the basename of the
       * *relative* path. This used to be a literal `▫`, which meant the two sidebars named the
       * same file two different ways — and a search over a repository is exactly where a
       * glance at the icon tells you whether the hit is in Rust, in CSS or in a lockfile.
       */}
      <FileIcon row={{ name: basename(row.rel), kind: 'file' }} theme={iconTheme} />
      <span className={styles.fileName}>{row.rel}</span>
      {/* Tabular so a growing count does not shift the name beside it. */}
      <span className={styles.fileCount}>{row.hits}</span>
    </div>
  )
}

function HitLine({
  row,
  rowKey: key,
  style,
  selected,
  onAct,
}: {
  row: Extract<SearchRow, { kind: 'hit' }>
  rowKey: string
  style: React.CSSProperties
  selected: boolean
  onAct: (row: SearchRow, gesture: 'single' | 'double') => void
}) {
  // Trimmed first, then split: the offsets move with the indentation, and a hit inside a
  // nested block would otherwise draw as an empty row in a 252px panel. Only the *drawing*
  // is trimmed — `hitPosition` reads the untrimmed hit, so the caret lands on the real column.
  const hit = trimIndent(row.hit)
  const parts = splitHighlight(hit.text, hit.start, hit.end)
  return (
    <div
      className={selected ? `${styles.hitRow} ${styles.rowSelected}` : styles.hitRow}
      data-audit="searchHitRow"
      data-row-key={key}
      role="treeitem"
      aria-level={2}
      aria-selected={selected}
      style={style}
      title={`${hit.path}:${hit.line}`}
      onMouseDown={pressHandler(row, onAct)}
    >
      <span className={styles.lineNo}>{hit.line}</span>
      <span className={styles.lineText}>
        {parts.before}
        <mark className={styles.match}>{parts.match}</mark>
        {parts.after}
      </span>
    </div>
  )
}

/**
 * The last path component of a relative path.
 *
 * `rel` is what the backend drew the heading with, and in a multi-root project it is prefixed
 * with the root label — so it is not a filesystem path and is not handed to anything that
 * treats it as one. Only the icon needs it, and only the part after the last separator.
 */
function basename(rel: string): string {
  const cut = rel.lastIndexOf('/')
  return cut < 0 ? rel : rel.slice(cut + 1)
}

function Toggle({
  label,
  glyph,
  on,
  onClick,
}: {
  label: string
  glyph: string
  on: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className={on ? `${styles.toggle} ${styles.toggleOn}` : styles.toggle}
      data-audit="searchToggle"
      /* `aria-pressed` rather than a checkbox: these are three independent switches on a
         toolbar, and the glyph is the only visible label. */
      aria-pressed={on}
      aria-label={label}
      title={label}
      onClick={onClick}
    >
      {glyph}
    </button>
  )
}
