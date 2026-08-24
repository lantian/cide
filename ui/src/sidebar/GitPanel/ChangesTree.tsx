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
 * # Three different things called selection
 *
 * `selected` in this file is the **tick** — the set of files a commit would take. The
 * **row selection** is `selection`, the set of rows a gesture is about, drawn as the band and
 * carried by a drag. The **current row** is `current`, one id, what the arrows walk and what
 * a focus lands on. All three are deliberately separate: ticking a file is a statement about a
 * commit, selecting one is a statement about what you are pointing at, and the cursor is a
 * statement about where the keyboard is. Conflating the first two would mean a click that
 * silently changed what the Commit button does — and conflating them the other way is what the
 * drag used to do, moving a whole changelist when the user grabbed one file out of it.
 *
 * The rules for all three live where a check script can run them: ticks in `model.ts`, row
 * selection in `rowSelection.ts`, click meanings in `clickSemantics.ts`. This file wires them
 * to a DOM and nothing more, because nothing in this repo can execute a DOM.
 *
 * `current` is a row **id**, not an index, and so is every id in `selection`. Row ids are
 * stable across refreshes (`fileRowId` is repo plus path) and this panel refreshes constantly
 * — on every `cide://git-status`, on every watcher burst, while Claude is editing. An index
 * would point at a different file every time a group above it gained or lost one.
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
import { blurLeftTheTree } from '@/sidebar/speedSearch'
import {
  checkState,
  diffOpenMode,
  entryStatus,
  splitPath,
  type CheckState,
  type Row,
} from './model'
import { bands, draggedIds, inDrag } from './dragDrop'
import type { RowSelection, SelectMods } from './rowSelection'
import { useChangesDrag, type ChangesDragState } from './useChangesDrag'
import { TriCheckbox } from './TriCheckbox'
import type { DiffOpenMode, RepoId, StatusView } from './types'
import { gestureOf, gitTreeClick } from '../clickSemantics'
import { useSpeedSearch } from '../useSpeedSearch'
import { SpeedName, SpeedSearchBar } from '../SpeedSearchBar'
import { fs as fsApi } from '@/ipc/client'
/*
 * Reached by file rather than through the `@/icons` barrel, which is what every other caller
 * uses. The barrel also exports `useIconTheme`, which reads `@/store/workspace`, which touches
 * `document` at import time — and `check-git-render.mjs` renders this tree under node. Nothing
 * here calls that hook (the theme arrives as a prop, see `GitPanel.tsx`), but a barrel import
 * pulls it in anyway, and the failure is a `ReferenceError` in the check rather than anything
 * a reader of this file would connect to an import.
 */
import { FileIcon } from '@/icons/FileIcon'
import { Icon } from '@/icons/Icon'
import type { IconTheme } from '@/icons/iconFor'
import styles from './ChangesTree.module.css'

export interface ChangesTreeProps {
  rows: readonly Row[]
  /**
   * The whole tree, for the drag.
   *
   * A grab that starts on a selected row carries the whole selection in the same repository,
   * and a selected file survives a collapsed group — `rows` holds no file rows for one, so the
   * rows alone would quietly narrow a multi-file drag to what happens to be on screen.
   * `flatFiles` reads the view, exactly as commit does.
   */
  view: StatusView
  /** The **ticks**: what a commit would take. Drawn as the checkboxes. */
  selected: ReadonlySet<string>
  /** The **row selection**: what a gesture is about. Drawn as the band. */
  selection: RowSelection
  /** `selection`, resolved for the drag — see `rowSelection.ts::carriedIds`. */
  carried: ReadonlySet<string>
  expanded: ReadonlySet<string>
  /** The row the user is pointing at, by id. Neither a tick nor the selection. */
  current: string | null
  /** Move the cursor and nothing else. What a focus calls. */
  onCurrent: (id: string) => void
  /** A left press. Returns `true` when its collapse was deferred to the release. */
  onPress: (id: string, mods: SelectMods) => boolean
  /** The release of a deferred press, when the gesture was not a drag after all. */
  onRelease: (id: string) => void
  /** An arrow, Home or End that has chosen its destination. */
  onKeyTo: (id: string, mods: SelectMods) => void
  onSelectAll: () => void
  /** Escape: back to the cursor's row. */
  onCollapseSelection: () => void
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
  /** Space over a multi-row selection: tick every selected row at once. */
  onToggleCheckSelected: () => void
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
  /**
   * Unversioned paths dropped on a changelist row: added to git, then filed.
   *
   * Separate from `onMovePaths` because it is a different operation on the repository — see
   * `dragDrop.ts`'s `track` outcome. Absent makes *that* drop inert on its own, leaving the
   * ordinary move working, which is why the hook checks them per row kind rather than
   * together.
   */
  onTrackPaths?: ((repo: RepoId, changelist: string, paths: string[]) => void) | undefined
  /** From the host's `useContextMenu`. Absent in a render with no window to open one in. */
  onContextMenu?: ((e: React.MouseEvent) => void) | undefined
  /** The portalled menu itself. It must be rendered or nothing appears. */
  menu?: ReactNode
}

/**
 * A plain move: no band, no ctrl.
 *
 * A module constant rather than an object literal at the call site, because `land` is in a
 * `useCallback` dependency list and a fresh `{}` on every render would rebuild it every render.
 */
const NO_KEY_MODS: SelectMods = { ctrl: false, shift: false }

const ARIA_CHECKED: Record<CheckState, 'true' | 'false' | 'mixed'> = {
  checked: 'true',
  partial: 'mixed',
  unchecked: 'false',
}

export function ChangesTree({
  rows,
  view,
  selected,
  selection,
  carried,
  expanded,
  current,
  onCurrent,
  onPress,
  onRelease,
  onKeyTo,
  onSelectAll,
  onCollapseSelection,
  diffOpen,
  iconTheme,
  onToggleCheck,
  onToggleCheckSelected,
  onToggleExpand,
  onOpenDiff,
  onMovePaths,
  onTrackPaths,
  onContextMenu,
  menu,
}: ChangesTreeProps) {
  const container = useRef<HTMLDivElement>(null)
  const drag = useChangesDrag({
    rows,
    view,
    carried,
    container,
    onMove: onMovePaths,
    onTrack: onTrackPaths,
  })
  /**
   * The row whose plain press deferred its collapse, until the mouseup answers for it.
   *
   * A ref rather than state: nothing renders differently in between, and a re-render inside a
   * mousedown handler is a re-render the drag's own threshold logic then has to survive.
   */
  const deferred = useRef<string | null>(null)
  /** The ids in flight right now, for the dimming. Not `carried`, which is the selection. */
  const inFlight = useMemo(
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

  /**
   * Move the cursor to a row index, taking the selection with it or not as the modifiers say.
   *
   * The focus move is imperative and unconditional; what happens to the selection is
   * `rowSelection.ts::keySelect`'s decision, reached through the model so the pointer and the
   * keyboard cannot end up with two sets of rules. `mods` is threaded from the keystroke
   * rather than read off a ref, because shift+↓ and ↓ differ in nothing else.
   */
  const move = useCallback(
    (next: number, mods: SelectMods) => {
      const row = rows[next]
      if (row === undefined) return
      onKeyTo(row.id, mods)
      container.current?.querySelector<HTMLElement>(`[data-index="${next}"]`)?.focus()
    },
    [rows, onKeyTo],
  )

  /**
   * Type-ahead over this tree's rows. (M15)
   *
   * The same hook and the same rules as the explorer's, which is the point: two sidebar trees
   * that filtered differently, or that disagreed about what Escape does, would be one feature
   * with two behaviours. Only the three adapters differ, and each is a fact about this tree:
   *
   *   * **`search`** goes to `tree_match_labels`, which is the *same* Rust rule the explorer's
   *     `fs_tree_match` uses — the `picker_rank` shape. Matching these rows in a local loop
   *     would have been cheaper by one round trip and would have been a second implementation
   *     of `cide_fs::speed`.
   *   * **`land`** is `move`, which already focuses the row and takes the roving tabindex with
   *     it.
   *   * **`revision`** is `rows` itself, which `GitPanel` rebuilds whenever a `git status`
   *     lands — so a match list computed against the previous walk is re-issued rather than
   *     drawn against rows that have moved.
   */
  const labels = useMemo(() => rows.map((row) => row.label), [rows])
  const speedSearch = useSpeedSearch({
    search: useCallback((query: string) => fsApi.matchLabels(query, labels), [labels]),
    land: useCallback((row: number) => move(row, NO_KEY_MODS), [move]),
    accept: useCallback(() => {
      const row = rows[at]
      if (row === undefined) return
      // Exactly what Enter does below, and reached the same way — a second spelling here would
      // be a second Enter, and the two would agree only until somebody edited one.
      if (row.expandable) onToggleExpand(row)
      else onOpenDiff(row, 'open')
    }, [rows, at, onToggleExpand, onOpenDiff]),
    count: rows.length,
    revision: rows,
  })

  const onKeyDown = useCallback(
    (e: KeyboardEvent, row: Row, index: number) => {
      /*
       * Speed search, and it is **first** — above Space, above Escape, above everything.
       *
       * Two collisions make the order load-bearing here rather than tidy:
       *
       *   * **Space is the tick.** With a query typed it continues the query (`check tree` has
       *     a space in it) and with nothing typed it stays the tick, untouched. `speedKey` owns
       *     that rule, which is why it takes the query rather than a boolean.
       *   * **Escape collapses the selection.** A user who typed three letters and pressed
       *     Escape to call the search off would otherwise have collapsed their multi-row
       *     selection instead — and would have to press it twice to get what they asked for.
       *
       * Ctrl+A is untouched: `speedKey` hands every modified chord back after ending the
       * search, so select-all still selects all.
       */
      if (speedSearch.onKeyDown(e)) return
      const isOpen = expanded.has(row.id)
      const mods: SelectMods = { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey }
      const last = rows.length - 1
      switch (e.key) {
        case 'ArrowDown':
          move(Math.min(index + 1, last), mods)
          break
        case 'ArrowUp':
          move(Math.max(index - 1, 0), mods)
          break
        case 'ArrowRight':
          if (row.expandable && !isOpen) onToggleExpand(row)
          else move(Math.min(index + 1, last), mods)
          break
        case 'ArrowLeft':
          if (row.expandable && isOpen) onToggleExpand(row)
          else move(parentOf(rows, index), mods)
          break
        case 'Home':
          move(0, mods)
          break
        case 'End':
          move(last, mods)
          break
        case 'a':
        case 'A':
          /*
           * Ctrl+A, and only with Ctrl — a bare `a` is a printable character and belongs to
           * the browser (the `default` arm below). Handled here rather than as a registry
           * command for the same reason Delete and Ctrl+R are in the file tree: the key gate's
           * window-capture listener resolves a global binding *before* the event reaches its
           * target, so `ctrl+a` bound globally would swallow select-all inside the commit
           * message box, every terminal and CodeMirror. `ui/src/keys/` has no editable-target
           * guard at all, so there is no version of that binding which is safe today.
           */
          if (!mods.ctrl || e.altKey || mods.shift) return
          onSelectAll()
          break
        case 'Escape':
          /*
           * Collapse to the cursor. Not `stopPropagation` as well: a drag in flight owns
           * Escape (`useChangesDrag` listens in the capture phase and stops it there), so by
           * the time this runs there is no gesture to cancel and the key is free.
           */
          onCollapseSelection()
          break
        case ' ':
          /*
           * Space ticks. Over a multi-row selection that includes this row it ticks all of it,
           * which is the IDEA behaviour and the only reason a keyboard user wants a multi-row
           * selection in this panel at all. Over anything else it ticks the row under the
           * cursor — including a row that is *not* in the selection, because the cursor is
           * what Space has always acted on and a Space that did nothing because the cursor had
           * drifted out of the selection would be the tree refusing a keystroke silently.
           */
          if (selection.ids.size > 1 && selection.ids.has(row.id)) onToggleCheckSelected()
          else onToggleCheck(row)
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
    [
      rows,
      expanded,
      move,
      selection,
      onCollapseSelection,
      onSelectAll,
      onToggleCheck,
      onToggleCheckSelected,
      onToggleExpand,
      onOpenDiff,
      speedSearch,
    ],
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
    <>
      {/* A sibling of the scroller, never a child: that element is `role="tree"` and its
          children have to be tree items. `GitPanel.module.css`'s `.body` is the containing
          block — see the `position: relative` there. */}
      {speedSearch.active && (
        <SpeedSearchBar
          query={speedSearch.query}
          summary={speedSearch.summary}
          audit="gitTreeSpeedSearch"
        />
      )}
    <div
      ref={container}
      className={styles.tree}
      role="tree"
      aria-label="Changes"
      /* True since `rowSelection.ts` landed. It was here before that, on a tree whose
         `aria-selected` tracked a single cursor — a claim to assistive tech that the app could
         not honour, which `check:render` now pins from the other end. */
      aria-multiselectable="true"
      data-audit="gitTree"
      /* One flag for the whole box while a drag is in flight: it turns off the hover wash,
         which would otherwise follow the pointer *and* the drop outline — two bands saying two
         different things. Text selection is refused unconditionally on `.tree`, not here; that
         rule and the two reasons it could never have worked from this attribute are in
         `ChangesTree.module.css`. */
      {...(drag.state === null ? {} : { 'data-dragging': '' })}
      /* The two exits that are not keystrokes, and the same pair the explorer installs: a
         press is the user choosing a row with the pointer, and a blur would otherwise leave a
         query armed to swallow the first letters typed on the way back. Capture phase, so a
         press that moves focus is caught either way. */
      onPointerDownCapture={speedSearch.exit}
      onBlur={(e) => {
        // Only when focus really left the tree — `onBlur` is the bubbling
        // `focusout`, so landing on a match fires it too. See `blurLeftTheTree`.
        if (blurLeftTheTree(e.currentTarget, e.relatedTarget)) speedSearch.exit()
      }}
      onContextMenu={onContextMenu}
    >
      {rows.map((row, index) => {
        const state = checkState(row, selected)
        const open = expanded.has(row.id)
        const isCurrent = index === found
        const isSelected = selection.ids.has(row.id)
        return (
          <div
            key={row.id}
            data-index={index}
            /* The two halves of the drag's feedback. `data-drag` dims what is in flight —
               without it a drag of four files looks exactly like a drag of one — and
               `data-drop` says whether the row under the pointer will take it. The verdict is
               drawn on the target as well as on the ghost because the pointer is where the eye
               is, and the ghost is 12px away from it. */
            {...(inFlight !== null && inDrag(inFlight, row) ? { 'data-drag': '' } : {})}
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
            /* The row *selection*, not the cursor. It used to be `isCurrent`, which made the
               tree tell a screen reader that exactly one row was ever selected while claiming
               `aria-multiselectable` two lines up. `aria-checked` beside it is the tick, and
               the two now say different things because they are different things. */
            aria-selected={isSelected}
            {...(row.expandable ? { 'aria-expanded': open } : {})}
            tabIndex={index === at ? 0 : -1}
            className={rowClass(row, isCurrent, isSelected)}
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
              /*
               * Refuse the native text selection at the engine, not only in CSS.
               *
               * `.tree` sets `-webkit-user-select: none`, which is the real fix and is why the
               * drag stopped painting the panel blue. This is the second lock, and it is not
               * redundant: `preventDefault` on the mousedown is the engine-level "this press
               * does not begin a selection", it is independent of which spellings of
               * `user-select` the engine happens to implement, and it is what stops a
               * *shift*-click from extending a selection that began somewhere else on the page
               * — the commit message box, a diff pane — which no rule on this subtree can
               * reach.
               *
               * It costs the automatic focus, which the roving tabindex and the `onFocus →
               * onCurrent` path below both depend on, so the focus is taken by hand. Nothing
               * else on this row relies on the default action of a press.
               */
              e.preventDefault()
              e.currentTarget.focus()
              const gesture = gestureOf(e.detail)
              const mods: SelectMods = { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey }
              const action = gitTreeClick({
                gesture,
                expandable: row.expandable,
                isDir: row.kind === 'dir',
                onTwisty: pressOnTwisty(e, row),
                diffOpen,
              })
              deferred.current = null
              // `action.select` is false only on the second half of a double-click on a
              // header or a twisty, whose first half has already moved the cursor and set
              // the selection.
              if (action.select && onPress(row.id, mods)) deferred.current = row.id
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
               *
               * Gated on an *unmodified* press, which is new and belongs here rather than in
               * `gitTreeClick`: that rule is shared with the file tree and the search results,
               * and neither of those has a multi-row selection to build. A ctrl-click means
               * "add this row" and a shift-click means "extend to here" — either one also
               * throwing a diff on screen would open a tab per row while the user assembles a
               * selection, which is the thirty-tabs bug arriving through a different door.
               */
              if (action.open && !mods.ctrl && !mods.shift) onOpenDiff(row, diffOpenMode(gesture))
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
             * The rule itself lives in `gitTreeClick`, asked again with the same
             * `detail`-derived gesture (a mouseup carries the click count too): a header folds
             * on the first release and refuses the second, a directory folds on the second
             * release or on a twisty's first — the file tree's gesture, ported on request —
             * and a file row toggles nothing. Diff-opening stays on the press, where it feels
             * immediate.
             *
             * The *selection's* collapse is here for the same reason the fold is, and it is
             * the detail that makes dragging a multi-row selection possible at all. A plain
             * press on a row that is already one of several selected must not collapse the
             * selection to that row on mousedown — the press is also how a drag of the whole
             * selection begins, and collapsing first would destroy the set before the pointer
             * had moved a pixel. `drag.dragged()` in the guard above is exactly the signal
             * needed, and it was already being read one line down for the fold.
             */
            onMouseUp={(e) => {
              if (e.button !== 0) return
              if (drag.dragged()) {
                // The gesture became a drag, so the deferred collapse is abandoned: the whole
                // selection has just been moved and it is still the right selection.
                deferred.current = null
                return
              }
              if (deferred.current === row.id) {
                deferred.current = null
                onRelease(row.id)
              }
              const action = gitTreeClick({
                gesture: gestureOf(e.detail),
                expandable: row.expandable,
                isDir: row.kind === 'dir',
                onTwisty: pressOnTwisty(e, row),
                diffOpen,
              })
              if (action.toggle) onToggleExpand(row)
            }}
            onKeyDown={(e) => onKeyDown(e, row, index)}
          >
            <span
              className={styles.twisty}
              /* The twisty is a control, not a handle — on a directory row one click on it is
                 the fold, so a hand that shifts three pixels while clicking it must not pick
                 the row up instead. The *click* still reaches the row (only the pointer press
                 is stopped), so `gitTreeClick`'s `onTwisty` rule is untouched. Same rule, same
                 comment as the explorer's twisty and this tree's own checkbox. */
              onPointerDown={(e) => e.stopPropagation()}
              aria-hidden="true"
            >
              {row.expandable ? (
                <Icon name={open ? 'chevron-down' : 'chevron-right'} size={1} />
              ) : null}
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
              /* The cursor moves, the *selection* does not, and that is the rule the whole
                 feature rests on: a tick is a statement about a commit and a selection is a
                 statement about what you are pointing at. A box that also selected its row
                 would mean ticking a file silently changed what the next drag carries — which
                 is the bug that made ticks the wrong thing to widen a drag by in the first
                 place, rebuilt from the other end. */
              onClick={(e) => {
                e.stopPropagation()
                onCurrent(row.id)
                onToggleCheck(row)
              }}
            >
              <TriCheckbox state={state} />
            </span>

            {row.kind === 'file' ? (
              <FileLabel row={row} iconTheme={iconTheme} match={speedSearch.spanFor(index)} />
            ) : (
              <GroupLabel
                row={row}
                open={open}
                iconTheme={iconTheme}
                match={speedSearch.spanFor(index)}
              />
            )}
          </div>
        )
      })}
      {/* Portals out of this box; where it sits in the caller's tree does not move it. */}
      {menu}
      {drag.state !== null && <DragGhost state={drag.state} />}
    </div>
    </>
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
 * Whether a press landed on the row's twisty — `gitTreeClick`'s `onTwisty`.
 *
 * `row.expandable` guards it the way `hasTwisty` guards the file tree's, and the guard is
 * load-bearing rather than tidy: the `.twisty` span is drawn on *every* row — it is part of
 * the layout, so a file row has one too, empty and 9px wide — and without the guard those 9px
 * would be a strip down the left of each file where a double-click resolved to "second half
 * of a twisty gesture", which is `NOTHING`: the file would simply refuse to open its diff.
 */
function pressOnTwisty(e: { target: EventTarget }, row: Row): boolean {
  return (
    row.expandable && e.target instanceof Element && e.target.closest(`.${styles.twisty}`) !== null
  )
}

/**
 * Four orthogonal facts about a row, so the ternary chain does not have to nest.
 *
 * `groupRow` is the `--panel-2` header ground, and a *directory* deliberately does not get it:
 * a folder inside a changelist is part of the list's contents, and painting it like a header
 * would make a changelist look as though it contained several changelists.
 *
 * `rowSelected` and `rowCurrent` are two classes because they are two facts, and a multi-row
 * selection is exactly where they come apart: every selected row gets the band, and only one
 * of them — the one the arrows would move from — gets the accent rule down its leading edge.
 * They composed into one class for as long as selection meant "the single row you clicked".
 */
function rowClass(row: Row, isCurrent: boolean, isSelected: boolean): string {
  const parts = [styles.row]
  if (row.kind === 'group' || row.kind === 'repo') parts.push(styles.groupRow)
  if (isSelected) parts.push(styles.rowSelected)
  if (isCurrent) parts.push(styles.rowCurrent)
  return parts.join(' ')
}

/**
 * A changelist, a repository or a directory: name, then its file count.
 *
 * The active changelist is the one a commit defaults to, so it is the one name in this
 * tree that is drawn in `--text` rather than `--dim`. A repository row carries its absolute
 * work tree as a tooltip — the row itself shows only the last path component, and in a
 * workspace with two roots called `core` that is the only way to tell them apart.
 *
 * A **directory** row draws the explorer's folder icon, open or closed with the row, because
 * without one it was the only folder in the app drawn as bare text — the same argument that
 * put the file icons on the leaves below it. The icon is looked up by the *deepest* segment
 * of `row.path`, not by the label: a compacted row reads `crates/cide-git/src` on one line,
 * and the directory that row actually is — the one its id and its files hang off — is `src`,
 * which is also what the explorer shows an icon for at that level. Headers draw none: a
 * changelist is not a folder, and a folder icon on it would say it is one.
 */
function GroupLabel({
  row,
  open,
  iconTheme,
  match,
}: {
  row: Row
  /** Expanded, per the tree's `expanded` set — drives the open/closed folder mark. */
  open: boolean
  iconTheme: IconTheme
  match: { start: number; end: number } | undefined
}) {
  return (
    <>
      {row.kind === 'dir' && (
        <FileIcon
          row={{ name: splitPath(row.path ?? row.label).name, kind: 'dir', expanded: open }}
          theme={iconTheme}
          className={styles.icon}
        />
      )}
      <span
        className={row.kind === 'dir' ? styles.dirName : styles.groupName}
        data-kind={row.kind}
        data-active={row.active === true ? '' : undefined}
        /* One tooltip, from whichever row kind has something to say: a repository's absolute
           work tree, or a directory's full path — a compacted row reads `ui/src/sidebar`, and
           where that sits is the question compaction raises and cannot answer in 420px. */
        {...title(row)}
      >
        <SpeedName name={row.label} span={match} />
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
function FileLabel({
  row,
  iconTheme,
  match,
}: {
  row: Row
  iconTheme: IconTheme
  match: { start: number; end: number } | undefined
}) {
  const entry = row.entry
  if (entry === undefined) return null
  /*
   * `row.label`, not `splitPath(entry.path).name`.
   *
   * They are the same string — `buildRows` sets the label with that very call — and that is
   * exactly the problem: it was two derivations of one name, and speed search would have made
   * it three. The query is matched against `Row.label` in Rust, so the *span* is an offset into
   * that string; rendering a differently-derived one would put the highlight over the wrong
   * glyphs the first time the two came apart. The icon still needs a bare filename, which is
   * what `splitPath` is left doing.
   */
  const name = row.label
  return (
    <>
      <FileIcon
        row={{ name: splitPath(entry.path).name, kind: 'file' }}
        theme={iconTheme}
        className={styles.icon}
      />
      <span className={styles.fileName} data-status={entryStatus(entry)} title={entry.path}>
        <SpeedName name={name} span={match} />
      </span>
      {entry.origPath !== null && (
        <span className={styles.dir}>← {splitPath(entry.origPath).name}</span>
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
