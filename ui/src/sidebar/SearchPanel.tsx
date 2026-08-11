/**
 * The 252px search panel: the query input, the three toggles, and the results.
 *
 * Same width and the same row metrics as the Explorer — 252px, 21px mono rows — because the
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
 * state machine, the readout — lives in `SearchModel.ts` and is tested by
 * `ui/scripts/check-search.mjs`. What is left here is markup and event wiring.
 */
import { useEffect, useRef } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useSearch } from './SearchStore'
import { groupHits, panelState, splitHighlight, summarize, trimIndent } from './SearchModel'
import type { SearchRow } from './SearchModel'
import { groupDigits } from '@/overlays/format'
import type { ProjectId } from '@/ipc/client'
import styles from './SearchPanel.module.css'

/** 21px rows, from the mock — the same as the file tree's. */
const ROW_HEIGHT = 21

export interface SearchPanelProps {
  /** The active project, or `null` when none is open. */
  project: ProjectId | null
  /**
   * Open a hit. The line is 1-based and may be ignored by a host that cannot scroll to one;
   * the file still opens, which is the part the user asked for.
   */
  onOpenHit?: ((path: string, line: number) => void) | undefined
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
  onOpenHit: ((path: string, line: number) => void) | undefined
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
function Body({ state, rows, error, scanned, onOpenHit }: BodyProps) {
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
  return <Results rows={rows} onOpenHit={onOpenHit} />
}

function Results({
  rows,
  onOpenHit,
}: {
  rows: SearchRow[]
  onOpenHit: ((path: string, line: number) => void) | undefined
}) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  })

  return (
    <div className={styles.scroll} ref={scrollRef} data-audit="searchScroll">
      <div className={styles.viewport} style={{ height: `${virtualizer.getTotalSize()}px` }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index]
          if (row === undefined) return null
          const style = {
            height: `${item.size}px`,
            transform: `translateY(${item.start}px)`,
          }
          return row.kind === 'file' ? (
            <FileHeading key={row.path} row={row} style={style} />
          ) : (
            <HitLine key={`${row.hit.path}:${row.index}`} row={row} style={style} onOpen={onOpenHit} />
          )
        })}
      </div>
    </div>
  )
}

function FileHeading({
  row,
  style,
}: {
  row: Extract<SearchRow, { kind: 'file' }>
  style: React.CSSProperties
}) {
  return (
    <button
      type="button"
      className={styles.fileRow}
      data-audit="searchFileRow"
      style={style}
      title={row.path}
      aria-expanded={!row.collapsed}
      onClick={() => useSearch.getState().toggleGroup(row.path)}
    >
      <span className={styles.twisty} aria-hidden="true">
        {row.collapsed ? '▸' : '▾'}
      </span>
      {/* A literal `▫`, still. The file tree now draws the vendored Material icon for the
          file's type instead; matching that here means threading the icon theme through
          `Body` → `Results` and taking a basename off `row.rel`, which is a change to this
          panel and not to the icon set. Until then the two sidebars deliberately differ. */}
      <span className={styles.glyph} aria-hidden="true">
        ▫
      </span>
      <span className={styles.fileName}>{row.rel}</span>
      {/* Tabular so a growing count does not shift the name beside it. */}
      <span className={styles.fileCount}>{row.hits}</span>
    </button>
  )
}

function HitLine({
  row,
  style,
  onOpen,
}: {
  row: Extract<SearchRow, { kind: 'hit' }>
  style: React.CSSProperties
  onOpen: ((path: string, line: number) => void) | undefined
}) {
  // Trimmed first, then split: the offsets move with the indentation, and a hit inside a
  // nested block would otherwise draw as an empty row in a 252px panel.
  const hit = trimIndent(row.hit)
  const parts = splitHighlight(hit.text, hit.start, hit.end)
  return (
    <button
      type="button"
      className={styles.hitRow}
      data-audit="searchHitRow"
      style={style}
      title={`${hit.path}:${hit.line}`}
      onClick={() => onOpen?.(hit.path, hit.line)}
    >
      <span className={styles.lineNo}>{hit.line}</span>
      <span className={styles.lineText}>
        {parts.before}
        <mark className={styles.match}>{parts.match}</mark>
        {parts.after}
      </span>
    </button>
  )
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
