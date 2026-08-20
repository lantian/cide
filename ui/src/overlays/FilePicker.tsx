/**
 * Ctrl+P — the file picker.
 *
 * The candidate set lives in Rust and streams: `fs.index` walks the project while
 * `picker_query` answers from the matcher it is filling, and the frame says `running` while
 * that is still true. That shape is what makes the picker usable on a 100k-file repository
 * *before* indexing finishes, which is M8's acceptance criterion — a command that returned a
 * `Vec` would make the first keystroke wait for the walk. The property is asserted end to
 * end in `crates/cide-app/src/cmd/fs.rs`.
 *
 * Two kinds of non-answer are distinguished below and neither is allowed to reject into a
 * render: a handler missing from the build (`pendingCommand`, one log line and a named
 * reason), and a project whose walk has not started (`NoIndex`, which is `Indexing…` and
 * another poll).
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ModalShell, Hint } from './ModalShell'
import { kindBadge, matchCounter, pickerEmptyState, pickerEmptyText } from './format'
import { isListKey, listAction } from './listKeys'
import { useOverlays } from './store'
import { isNoIndex } from '@/store/fileIndex'
import {
  pendingCommand,
  picker as pickerApi,
  type PickerFrame,
  type PickerRow,
  type ProjectId,
} from '@/ipc/client'
import styles from './Overlay.module.css'
import { scaledRow, useUiScale } from '@/settings/useUiScale'

/**
 * The last path component, which is what the row draws large.
 *
 * `PickerRow` carries `text` (the path relative to its root, which is what was matched) and
 * `value` (the absolute path, which is what opening one means). Neither is the basename, so
 * it is derived here rather than asking Rust for a third field that is a substring of one it
 * already sends.
 */
function basename(path: string): string {
  const cut = path.lastIndexOf('/')
  return cut === -1 ? path : path.slice(cut + 1)
}

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
  /** Bumped per query so a late frame for an older query is dropped. */
  const queryGeneration = useRef(0)
  const [failed, setFailed] = useState(false)
  /** The project exists but its walk has not started yet. Distinct from `failed`. */
  const [awaitingIndex, setAwaitingIndex] = useState(false)

  /*
   * Whether *External Libraries* are in scope.
   *
   * Read from the store rather than held here, because the two writers are outside this
   * component: the key gate dispatches `picker.libraries` from a window capture listener, and
   * the command palette can set it for the *next* Ctrl+P. `useState` here would also reset the
   * scope every time the overlay closed, which is the one thing the store's own note rules out.
   */
  const libraries = useOverlays((s) => s.libraries)
  const toggleLibraries = useOverlays((s) => s.toggleLibraries)

  /*
   * Start the walk the first time the scope is switched on for this project.
   *
   * Here rather than in `dispatch.ts`, and the reason is not tidiness: the walk needs a project
   * id and this component has one, the dispatcher does not, and a second place that could start
   * a `cargo metadata` is a second place to get *at most once per project* wrong. The command
   * itself is a flag flip and nothing more.
   *
   * Fire-and-forget, and idempotent on the Rust side: a second press, a second window, or a
   * re-open of the overlay finds the walk running or finished and starts nothing. The user sees
   * it happen through the poll below — `running` stays true and the counter climbs, exactly as
   * it does during the project's own walk.
   *
   * `pendingCommand` because a build without the handler must not reject into a render. The
   * fallback is `undefined`: nothing was started, the query below simply answers from an empty
   * library matcher, and the picker degrades to project-only rather than breaking.
   */
  useEffect(() => {
    if (!libraries) return
    void pendingCommand<void | undefined>(
      'picker_index_libraries',
      () => pickerApi.indexLibraries(project),
      undefined,
    )
  }, [libraries, project])

  const scrollRef = useRef<HTMLDivElement>(null)

  /*
   * A poll, not a subscription.
   *
   * This component was built against a `picker.open(project, kind, onFrame)` channel that
   * does not exist: the Rust side answers `picker_query` with a whole frame and reports
   * `running` while the walk is still filling the index. Polling is therefore not a
   * simplification, it is the protocol — and it is what makes the picker usable before
   * indexing finishes, which is the property `cide-search`'s streaming injector exists for.
   *
   * `generation` guards against the classic out-of-order search render: a slow frame for
   * `ma` must not repaint a list the user has already narrowed to `main`.
   */
  useEffect(() => {
    let cancelled = false
    const generation = ++queryGeneration.current
    let timer: ReturnType<typeof setTimeout> | undefined

    const poll = () => {
      /*
       * Three outcomes, not two.
       *
       * `NoIndex` is not a failure here: the project's walk is started by
       * `store/workspace.ts` and Ctrl+P can beat it by a frame, or arrive during the moment
       * a re-index has taken the old entry out. Treating it as one put *File index
       * unavailable — picker_query is not registered in this build* in front of a user whose
       * index was about to exist, and left it there, because `failed` never clears.
       */
      void pendingCommand<PickerFrame | 'notIndexedYet' | null>(
        'picker_query',
        async () => {
          try {
            return await pickerApi.query(project, query, undefined, libraries)
          } catch (error) {
            if (isNoIndex(error)) return 'notIndexedYet'
            throw error
          }
        },
        null,
      ).then((incoming) => {
        if (cancelled || generation !== queryGeneration.current) return
        if (incoming === null) {
          setFailed(true)
          return
        }
        if (incoming === 'notIndexedYet') {
          setAwaitingIndex(true)
          timer = setTimeout(poll, 120)
          return
        }
        setAwaitingIndex(false)
        setFrame(incoming)
        // Only while the index is still growing. A settled index is polled once.
        if (incoming.running) timer = setTimeout(poll, 120)
      })
    }
    poll()

    return () => {
      cancelled = true
      if (timer !== undefined) clearTimeout(timer)
    }
    // `libraries` is in here, and it has to be: flipping the scope changes the answer to the
    // *same* query, so without it ⌥L would do nothing visible until the user typed another
    // character — a control that appears not to work, which is worse than no control.
  }, [project, query, libraries])

  const hits: PickerRow[] = useMemo(() => frame?.items ?? [], [frame])
  // The chrome scale, for the one measurement CSS never sees: a virtualized row is placed by
  // an absolute transform off this number. See `settings/useUiScale.ts`.

  const rowHeight = scaledRow(ROW_HEIGHT, useUiScale())


  const virtualizer = useVirtualizer({
    count: hits.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
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
      if (modifier === 'shift') onOpenInSplit(hit.value)
      else if (modifier === 'alt') onMention(hit.value)
      else onOpen(hit.value)
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
          {/* One element that documents the chord *and* is the mouse target, which is what
              stops this being either an undiscoverable keystroke or a button whose keyboard
              equivalent nobody knows. It sits in the footer rather than beside the counter
              because that row is prompt + field + counter and is the one place the eye is
              while typing. */}
          <ScopeToggle on={libraries} onToggle={toggleLibraries} />
        </>
      }
    >
      {failed ? (
        <div className={styles.status}>
          File index unavailable — <code>picker_query</code> is not registered in this build.
        </div>
      ) : hits.length === 0 ? (
        /* Four states, two of which look identical and mean opposite things. The rule is
           `format.ts`'s and is driven by `check:picker`; this is the `switch` over its answer
           and holds no decision of its own. */
        <div className={styles.status}>
          {pickerEmptyText(
            pickerEmptyState({
              running: frame?.running === true,
              awaitingIndex,
              query,
              libraries,
              total: frame?.total ?? null,
            }),
          )}
        </div>
      ) : (
        <div className={styles.viewport} style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((item) => {
            const hit = hits[item.index]
            if (!hit) return null
            const badge = kindBadge(basename(hit.value))
            const tone = TONE_CLASS[badge.tone] ?? styles.toneFaint
            return (
              <div
                key={hit.value}
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
                <span className={styles.name}>{basename(hit.value)}</span>
                <span className={styles.path}>{hit.text}</span>
                {/* The provenance chip, right-aligned. Rendered only when there is one — a
                    project row has nothing to say and an empty chip would be a column of
                    blank boxes down the left of the user's own files. */}
                {hit.source != null && <span className={styles.source}>{hit.source}</span>}
              </div>
            )
          })}
        </div>
      )}
    </ModalShell>
  )
}

/**
 * The footer's scope chip: `⌥L Libraries`, lit when the scope is on.
 *
 * # Why an `aria-pressed` button and not a checkbox
 *
 * Nothing in the overlay language uses a checkbox, and the app's own precedent for a binary
 * control that is not a form field is `SearchPanel`'s `aria-pressed` buttons. A segmented
 * control was the other candidate and is too heavy at this width for two states of one binary.
 *
 * # `onMouseDown` with `preventDefault`, never `onClick`
 *
 * The same trick the rows use, and for the same reason: a click blurs the input between
 * `mousedown` and `click`, and `ModalShell`'s `selectionchange` listener repaints the caret
 * when it does — after which typing stops working. Keeping focus in the field is also what
 * makes the chord and the chip interchangeable rather than two different gestures.
 */
function ScopeToggle({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      data-audit="pickerScopeToggle"
      className={on ? `${styles.scope} ${styles.scopeOn}` : styles.scope}
      title="Search the source of this project's resolved dependencies as well. Nothing is walked until you ask, and nothing is watched."
      onMouseDown={(ev) => {
        ev.preventDefault()
        onToggle()
      }}
    >
      <span className={styles.footerKey}>⌥L</span>
      Libraries
    </button>
  )
}
