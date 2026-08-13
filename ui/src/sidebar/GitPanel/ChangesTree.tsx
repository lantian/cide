/**
 * The 24px-row tri-state checkbox tree.
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
 * # Two different things called selection
 *
 * `selected` in this file is the **tick** — the set of files a commit would take. The
 * *current row*, which is what a click moves and what the arrows walk, is `current`, and it
 * is deliberately a different thing: ticking a file is a statement about a commit, and
 * pointing at one is a statement about what you are looking at. Conflating them would mean a
 * click that silently changed what the Commit button does.
 *
 * `current` is a row **id**, not an index. Row ids are stable across refreshes (`fileRowId`
 * is repo plus path) and this panel refreshes constantly — on every `cide://git-status`, on
 * every watcher burst, while Claude is editing. An index would point at a different file
 * every time a group above it gained or lost one.
 *
 * # Dragging rows into another changelist
 *
 * The gesture is in `useChangesDrag.ts` (pointer events, and why not HTML5 drag and drop) and
 * every rule it applies is in `dragDrop.ts` (what a grab carries, which targets accept it,
 * what a same-list drop does). This file only draws the result: the rows in flight are dimmed,
 * the target changelist is ringed and its band tinted, and a ghost at the pointer says both
 * what is being carried and what letting go would do.
 *
 * It is not the only route to the operation. A drag is unusable without a pointer, so the
 * context menu — Shift+F10 on the focused row — offers the same move on a file, a directory
 * and a whole changelist, with the file count in the label. See `GitPanelHost.tsx`.
 *
 * # Focus
 *
 * Roving tabindex — the tree is one tab stop and the arrows move within it, which is what
 * IDEA does and what the ARIA tree pattern requires. Focus is moved imperatively only in
 * response to a key or a click, never on render: a tree that grabs focus when a background
 * refresh changes its rows would steal the caret out of the commit message box, and this
 * panel refreshes while the user types.
 */
import { useCallback, useMemo, useRef, type KeyboardEvent, type ReactNode } from 'react'
import {
  checkState,
  diffOpenMode,
  entryStatus,
  isPartiallyStaged,
  splitPath,
  type CheckState,
  type Row,
} from './model'
import { bands, draggedIds, inDrag } from './dragDrop'
import { useChangesDrag, type ChangesDragState } from './useChangesDrag'
import { TriCheckbox } from './TriCheckbox'
import type { DiffOpenMode, RepoId, StatusView } from './types'
import { gestureOf, gitTreeClick } from '../clickSemantics'
/*
 * Reached by file rather than through the `@/icons` barrel, which is what every other caller
 * uses. The barrel also exports `useIconTheme`, which reads `@/store/workspace`, which touches
 * `document` at import time — and `check-git-render.mjs` renders this tree under node. Nothing
 * here calls that hook (the theme arrives as a prop, see `GitPanel.tsx`), but a barrel import
 * pulls it in anyway, and the failure is a `ReferenceError` in the check rather than anything
 * a reader of this file would connect to an import.
 */
import { FileIcon } from '@/icons/FileIcon'
import type { IconTheme } from '@/icons/iconFor'
import styles from './ChangesTree.module.css'

export interface ChangesTreeProps {
  rows: readonly Row[]
  /**
   * The whole tree, for the drag.
   *
   * A grab that starts on a ticked row carries every tick in the same repository, and ticks
   * survive a collapsed group — `rows` holds no file rows for one, so the rows alone would
   * quietly narrow a multi-file drag to what happens to be on screen. `flatFiles` reads the
   * view, exactly as commit does.
   */
  view: StatusView
  selected: ReadonlySet<string>
  expanded: ReadonlySet<string>
  /** Ids whose file is only partly staged — what turns a `✓` into a `–`. */
  partial: ReadonlySet<string>
  /** The row the user is pointing at, by id. Not the tick; see the module header. */
  current: string | null
  onCurrent: (id: string) => void
  /**
   * A git diff tab is open in this project.
   *
   * The one piece of state outside this tree that changes what a *single* click does. Read
   * from the workspace, not from what this panel last opened — see `openDiffTabs.ts`.
   */
  diffOpen: boolean
  /** Threaded from the panel, never subscribed to per row. Same argument as `FileTree`. */
  iconTheme: IconTheme
  onToggleCheck: (row: Row) => void
  onToggleExpand: (row: Row) => void
  /**
   * Activate a file row.
   *
   * `mode` is derived here rather than passed in, because it is a property of the *gesture*
   * and this is the only place that sees one — see the `onMouseDown` below.
   */
  onOpenDiff: (row: Row, mode: DiffOpenMode) => void
  /**
   * File paths dropped on a changelist row.
   *
   * Absent makes the tree inert rather than decorative: with nothing to run the move, no press
   * becomes a drag at all, so there is no gesture that picks rows up and puts them back.
   */
  onMovePaths?: ((repo: RepoId, changelist: string, paths: string[]) => void) | undefined
  /** From the host's `useContextMenu`. Absent in a render with no window to open one in. */
  onContextMenu?: ((e: React.MouseEvent) => void) | undefined
  /** The portalled menu itself. It must be rendered or nothing appears. */
  menu?: ReactNode
}

const ARIA_CHECKED: Record<CheckState, 'true' | 'false' | 'mixed'> = {
  checked: 'true',
  partial: 'mixed',
  unchecked: 'false',
}

export function ChangesTree({
  rows,
  view,
  selected,
  expanded,
  partial,
  current,
  onCurrent,
  diffOpen,
  iconTheme,
  onToggleCheck,
  onToggleExpand,
  onOpenDiff,
  onMovePaths,
  onContextMenu,
  menu,
}: ChangesTreeProps) {
  const container = useRef<HTMLDivElement>(null)
  const drag = useChangesDrag({ rows, view, selected, container, onMove: onMovePaths })
  const carried = useMemo(
    () => (drag.state === null ? null : draggedIds(drag.state.drag)),
    [drag.state],
  )
  /*
   * Which changelist each row is in, while a drag is in flight and not otherwise.
   *
   * The target's *header* carries the outline, and in this panel that header has usually
   * scrolled off the top by the time the pointer is over its files — so the whole band is
   * tinted too. Built from `rows` rather than per row, and only during a drag: it is one pass
   * over a list this component is already mapping.
   */
  const dragging = drag.state !== null
  const band = useMemo(() => (dragging ? bands(rows) : null), [dragging, rows])
  /*
   * Where the current row sits *now*.
   *
   * Derived on every render rather than stored, because the rows underneath it move: a
   * refresh can insert a file above the current one, and a group folding away can remove it
   * entirely. `-1` (not found) clamps to 0, so a tree whose current row has just been
   * committed away puts the tab stop back on the top row rather than nowhere.
   */
  const found = current === null ? -1 : rows.findIndex((row) => row.id === current)
  const at = rows.length === 0 ? 0 : Math.max(0, Math.min(found, rows.length - 1))

  const move = useCallback(
    (next: number) => {
      const row = rows[next]
      if (row === undefined) return
      onCurrent(row.id)
      container.current?.querySelector<HTMLElement>(`[data-index="${next}"]`)?.focus()
    },
    [rows, onCurrent],
  )

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
          // The keyboard has no second click to wait for, so Enter opens a leaf whether or
          // not a diff is already on screen. See `enterOn` in `clickSemantics.ts`.
          if (row.expandable) onToggleExpand(row)
          // `'open'`, unconditionally. Enter is not a click that might have been a
          // double-click, so there is no gesture to disambiguate and nothing here means
          // "just follow along" — the user asked for this file.
          else onOpenDiff(row, 'open')
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
      <div className={styles.tree} data-audit="gitTree" onContextMenu={onContextMenu}>
        <p className={styles.empty}>No changes.</p>
        {menu}
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
      /* One flag for the whole box while a drag is in flight: it turns off the hover wash
         (which would otherwise follow the pointer *and* the drop outline, two bands saying
         two different things) and stops the pointer selecting the labels it crosses. */
      {...(drag.state === null ? {} : { 'data-dragging': '' })}
      onContextMenu={onContextMenu}
    >
      {rows.map((row, index) => {
        const state = checkState(row, selected, (id) => partial.has(id))
        const open = expanded.has(row.id)
        const isCurrent = index === found
        return (
          <div
            key={row.id}
            data-index={index}
            /* The two halves of the drag's feedback. `data-drag` dims what is in flight —
               without it a drag of four files looks exactly like a drag of one — and
               `data-drop` says whether the row under the pointer will take it. The verdict is
               drawn on the target as well as on the ghost because the pointer is where the eye
               is, and the ghost is 12px away from it. */
            {...(carried !== null && inDrag(carried, row) ? { 'data-drag': '' } : {})}
            {...(drag.state !== null && drag.state.over === row.id
              ? { 'data-drop': drag.state.outcome.kind }
              : {})}
            {...(drag.state !== null
              && drag.state.over !== null
              && drag.state.over !== row.id
              && band?.get(row.id) === drag.state.over
              ? { 'data-drop-band': drag.state.outcome.kind }
              : {})}
            /* Read back by the host's context menu at open time. It sits here rather than
               between `data-audit` and the aria pair, whose adjacency `check-git-render.mjs`
               matches on. */
            data-row-id={row.id}
            data-audit="gitRow"
            data-kind={row.kind}
            role="treeitem"
            aria-level={row.depth + 1}
            aria-checked={ARIA_CHECKED[state]}
            aria-selected={isCurrent}
            {...(row.expandable ? { 'aria-expanded': open } : {})}
            tabIndex={index === at ? 0 : -1}
            className={rowClass(row, isCurrent)}
            // Indent is a padding rather than a spacer element so the whole 24px row stays
            // one hit target, including the empty space to the left of a deep file.
            //
            // 19px per depth, measured off IDEA — the same step the explorer uses, and the
            // reason it is 19 rather than the 14 this shipped with is written out in
            // `sidebar/FileTree.tsx`. One number across both trees because IDEA has one.
            // It makes the path compaction in `model.ts` matter more, not less: four levels
            // now cost 76px of a 420px panel instead of 56.
            style={{ paddingLeft: `${8 + row.depth * 19}px` }}
            onFocus={() => onCurrent(row.id)}
            /* First, and deliberately separate from the click handling below: a press only
               becomes a drag after 4px of movement, so the click rules keep running exactly as
               they did and a press that never moves costs nothing at all. */
            onPointerDown={(e) => drag.onPointerDown(e, row)}
            /*
             * One handler for both halves of a double-click, told apart by `detail` rather
             * than by a timer — see `clickSemantics.ts`. The old pair of `onClick` and
             * `onDoubleClick` could not express the rule this now obeys, because whether a
             * single click opens depends on `diffOpen`.
             *
             * Selection and diff-opening live on the press; the fold lives on the release, in
             * `onMouseUp` below, because a press on an expandable row is also how a drag of
             * that row begins.
             */
            onMouseDown={(e) => {
              if (e.button !== 0) return
              const gesture = gestureOf(e.detail)
              const action = gitTreeClick({
                gesture,
                expandable: row.expandable,
                diffOpen,
              })
              if (action.select) onCurrent(row.id)
              /*
               * The gesture, not a second rule. `gitTreeClick` returns `open` for two
               * different reasons — a double-click, or a single click while a diff is
               * already up — and until now both went to `tab_open_diff`, which reuses a tab
               * only when the path matches. That is where the thirty tabs came from.
               *
               * Derived from the gesture this handler already holds rather than from a fourth
               * boolean on `RowAction`: `clickSemantics.ts` is shared with the file tree and
               * the search results, neither of which has anything to retarget. A single click
               * that opens is *only* reachable through `diffOpen`, so this needs no second
               * look at that flag — see `gitTreeClick`.
               *
               * Through `diffOpenMode` rather than inline, because this component cannot be
               * executed by anything in the repo — it needs a DOM — and an inline ternary here
               * was the one link in the chain from the click rule to `tab_retarget_diff` that
               * no test could reach. `check-git-tree.mjs` runs that function under node.
               */
              if (action.open) onOpenDiff(row, diffOpenMode(gesture))
            }}
            /*
             * Folding happens on *release*, and that is a change with a reason.
             *
             * A press on an expandable row cannot be told apart from the start of a drag until
             * the pointer has moved, and a directory row is both — grabbing `ui/src/sidebar` to
             * drop it on another changelist would otherwise begin by collapsing it under the
             * pointer, taking the rows being dragged off the screen. So the toggle waits for
             * the mouseup and is skipped when the gesture turned out to be a drag.
             *
             * The rule itself is untouched: `gitTreeClick` is asked again, with the same
             * `detail`-derived gesture (a mouseup carries the click count too), so the second
             * half of a double-click still does nothing and a file row still toggles nothing.
             * Selection and diff-opening stay on the press, where they feel immediate.
             */
            onMouseUp={(e) => {
              if (e.button !== 0 || drag.dragged()) return
              const action = gitTreeClick({
                gesture: gestureOf(e.detail),
                expandable: row.expandable,
                diffOpen,
              })
              if (action.toggle) onToggleExpand(row)
            }}
            onKeyDown={(e) => onKeyDown(e, row, index)}
          >
            <span className={styles.twisty} aria-hidden="true">
              {row.expandable ? (open ? '▾' : '▸') : ''}
            </span>

            {/*
              A press on the box must tick, not expand and not open: a group's own row folds
              on mousedown and a file row may open a diff, so without stopping the press here
              every tick would also fold the group it was in.
            */}
            <span
              className={styles.checkHit}
              onMouseDown={(e) => e.stopPropagation()}
              /* The release too, now that the fold happens there: without this, ticking the
                 box of a changelist would also collapse the changelist. */
              onMouseUp={(e) => e.stopPropagation()}
              /* And the pointer press with it, or a hand that shifts two pixels while ticking
                 a box would pick the row up instead. The box is a control, not a handle. */
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation()
                onCurrent(row.id)
                onToggleCheck(row)
              }}
            >
              <TriCheckbox state={state} />
            </span>

            {row.kind === 'file' ? (
              <FileLabel row={row} iconTheme={iconTheme} />
            ) : (
              <GroupLabel row={row} />
            )}
          </div>
        )
      })}
      {/* Portals out of this box; where it sits in the caller's tree does not move it. */}
      {menu}
      {drag.state !== null && <DragGhost state={drag.state} />}
    </div>
  )
}

/**
 * What is being dragged, and whether the thing under the pointer will take it.
 *
 * A drag with no indicator is a gesture people abandon halfway, so this says both facts in
 * words rather than relying on a cursor: the load (`4 files`, `ui/src/sidebar/`) and the
 * verdict (`Move 4 files to “fixes”`, `Already in “Changes”`, or the reason it is refused).
 * The refusal is the important one — dropping onto *Unversioned Files* would otherwise write
 * to the sidecar and be undone by the next status walk, which looks exactly like nothing
 * happening.
 *
 * `position: fixed` at the pointer, offset down-right so it never sits under the cursor's own
 * hotspot, and `pointer-events: none` so it is never what `elementFromPoint` finds.
 */
function DragGhost({ state }: { state: ChangesDragState }) {
  return (
    <div
      className={styles.ghost}
      data-outcome={state.outcome.kind}
      data-audit="gitDragGhost"
      style={{ left: `${state.x + 12}px`, top: `${state.y + 14}px` }}
      aria-hidden="true"
    >
      <span className={styles.ghostLoad}>{state.drag.label}</span>
      <span className={styles.ghostHint}>{state.outcome.hint}</span>
    </div>
  )
}

/**
 * Three orthogonal facts about a row, so the ternary chain does not have to nest.
 *
 * `groupRow` is the `--panel-2` header ground, and a *directory* deliberately does not get it:
 * a folder inside a changelist is part of the list's contents, and painting it like a header
 * would make a changelist look as though it contained several changelists.
 */
function rowClass(row: Row, isCurrent: boolean): string {
  const parts = [styles.row]
  if (row.kind === 'group' || row.kind === 'repo') parts.push(styles.groupRow)
  if (isCurrent) parts.push(styles.rowCurrent)
  return parts.join(' ')
}

/**
 * A changelist or a repository: name, then its file count.
 *
 * The active changelist is the one a commit defaults to, so it is the one name in this
 * tree that is drawn in `--text` rather than `--dim`. A repository row carries its absolute
 * work tree as a tooltip — the row itself shows only the last path component, and in a
 * workspace with two roots called `core` that is the only way to tell them apart.
 */
function GroupLabel({ row }: { row: Row }) {
  return (
    <>
      <span
        className={row.kind === 'dir' ? styles.dirName : styles.groupName}
        data-kind={row.kind}
        data-active={row.active === true ? '' : undefined}
        /* One tooltip, from whichever row kind has something to say: a repository's absolute
           work tree, or a directory's full path — a compacted row reads `ui/src/sidebar`, and
           where that sits is the question compaction raises and cannot answer in 420px. */
        {...title(row)}
      >
        {row.label}
      </span>
      {row.count !== undefined && <span className={styles.count}>{row.count}</span>}
    </>
  )
}

/** `title`, or nothing at all — `exactOptionalPropertyTypes` forbids `title: undefined`. */
function title(row: Row): { title?: string } {
  if (row.root !== undefined) return { title: row.root }
  if (row.kind === 'dir' && row.path !== undefined) return { title: row.path }
  return {}
}

/**
 * `name`, in the status colour.
 *
 * The colours are the explorer's, deliberately: `M` blue, `A` green, `D` faint and struck
 * through. A file that is blue in the file tree and some other colour here would read as
 * two different pieces of information about the same file.
 *
 * The icon is the explorer's too, and for the same reason. This row used to draw none at
 * all, which made the changed-files list the one place in the app where a `.rs` and a
 * `Cargo.lock` looked identical.
 *
 * There is no dimmed parent directory beside the name any more, and its absence is the point:
 * every file now sits under a directory row that says exactly that path, so repeating it on
 * each leaf spent the row's width saying what the row above already said — and it was the
 * widest thing in a 420px panel. The full path is still the row's tooltip.
 */
function FileLabel({ row, iconTheme }: { row: Row; iconTheme: IconTheme }) {
  const entry = row.entry
  if (entry === undefined) return null
  const { name } = splitPath(entry.path)
  return (
    <>
      <FileIcon row={{ name, kind: 'file' }} theme={iconTheme} className={styles.icon} />
      <span className={styles.fileName} data-status={entryStatus(entry)} title={entry.path}>
        {name}
      </span>
      {entry.origPath !== null && (
        <span className={styles.dir}>← {splitPath(entry.origPath).name}</span>
      )}
      {isPartiallyStaged(entry) && (
        // Says out loud what the `–` box means for a leaf, which is otherwise the one
        // tri-state in the tree with no second row to compare against.
        <span className={styles.partialTag} title="Only part of this file is staged">
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
