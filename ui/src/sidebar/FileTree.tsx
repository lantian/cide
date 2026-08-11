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
 *
 * # Selection is not "what is open"
 *
 * > *"In file tree when i do one click on element - we should select it, but not open the
 * > file. Open only by double click"*
 *
 * So a click moves a *selection* and a double-click opens. The rules themselves are in
 * `clickSemantics.ts`, where a check script can hold them; what is here is the wiring, and
 * one decision worth stating at the call site: **there is no timer.** The obvious way to tell
 * one click from two is to wait a quarter of a second for the second, and that makes every
 * single click in the tree feel late. `MouseEvent.detail` already counts them, so the first
 * mousedown selects and the second — `detail === 2` — opens. See `gestureOf`.
 *
 * The selection lives in `treeStore`, not here, because it has to outlive both a refresh of
 * the rows underneath it and an unmount of this panel; see the field's comment there.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useShallow } from 'zustand/react/shallow'
import { useFileTree } from './treeStore'
import { useGitStatus } from './gitStatusStore'
import { letterFor, statusAt } from './treeStatus'
import { enterOn, fileTreeClick, gestureOf, moveIndex, type RowAction } from './clickSemantics'
import { copyText } from './copyText'
import {
  diag,
  fs as fsApi,
  fsReveal,
  isDegraded,
  type ProjectId,
  type TreeRow,
  type TreeStatus,
  type TreeStatusMap,
} from '@/ipc/client'
import { FileIcon, useIconTheme, type IconTheme } from '@/icons'
import { useContextMenu, type MenuEntry } from '@/menus'
import { useWorkspace } from '@/store/workspace'
import styles from './FileTree.module.css'

/** 21px rows, from the mock. */
const ROW_HEIGHT = 21

/** The `fs_*` calls the panel makes, in the order the notice should prefer to name them. */
const PENDING_COMMANDS = ['fs_tree_count', 'fs_tree_rows', 'fs_expand', 'fs_collapse'] as const

/** 12px per depth, from the mock. */
const INDENT = 12

/**
 * How many rows either side of a keyboard jump are asked for before reading one.
 *
 * `End` on a 100k-row tree lands in a chunk nobody has fetched, and `rowAt` answers
 * `undefined` there — so without this the selection would refuse to move to the one place the
 * key exists to reach. Half a screen is enough to cover the landing and the next few presses.
 */
const JUMP_MARGIN = 12

/** A stable empty list, so a project-less panel does not hand `useShallow` a new array. */
const NO_ROOTS: readonly string[] = []

/**
 * The root a path lives under, or `null` when none of them holds it.
 *
 * The longest match wins, because a project may be opened over both `~/work/cide` and
 * `~/work/cide/ui` — with the shortest match, the second root's files would be copied as
 * `ui/src/…` from one row and `src/…` from another, which is the kind of inconsistency that
 * makes people stop trusting *Copy Relative Path*.
 *
 * Segment-aware: `~/work/cide-old` is not under `~/work/cide`, and a plain `startsWith`
 * says it is.
 */
function rootOf(path: string, roots: readonly string[]): string | null {
  let best: string | null = null
  for (const root of roots) {
    if (path !== root && !path.startsWith(`${root}/`)) continue
    if (best === null || root.length > best.length) best = root
  }
  return best
}

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
  /** The active project. Needed by the menu's file operations, not by the rows. */
  project: ProjectId | null
  /** Called on a file row's double-click, and on Enter. Directory rows expand instead. */
  onOpen?: ((path: string) => void) | undefined
  /**
   * Open a file in a pane beside the current one.
   *
   * Optional, and the menu item says so when it is missing rather than pretending: nothing in
   * the sidebar can split a pane, so the only honest thing to do without a host is to show
   * the item disabled with the reason attached. See the note in `Explorer`.
   */
  onOpenToSide?: ((path: string) => void) | undefined
}

export function FileTree({ project, onOpen, onOpenToSide }: FileTreeProps) {
  const count = useFileTree((s) => s.count)
  const chunks = useFileTree((s) => s.chunks)
  const degraded = useFileTree((s) => s.degraded)
  const revealTo = useFileTree((s) => s.revealTo)
  const selected = useFileTree((s) => s.selected)
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
  /*
   * Subscribed here and threaded down for exactly the reason above: the icon set has a second
   * *file* per icon for the white theme rather than a CSS filter, so every row needs the theme
   * — and a `useIconTheme()` inside `Row` would be one more store listener per visible row
   * mounted and torn down on every scroll tick. One listener for the panel; the value is a
   * string, so passing it costs nothing.
   */
  const iconTheme = useIconTheme()
  /*
   * The project's root *paths*.
   *
   * Not `TreeRow.root`, which is an index into `Project::roots` and not a path at all —
   * reading it as one made *Copy Relative Path* copy the absolute path (identical to *Copy
   * Path*, so the item looked implemented and was not) and made `isRoot` always false, which left
   * Rename and Move to Trash enabled on a project root for `fs_rename`/`fs_delete` to refuse.
   *
   * From the workspace store rather than resolved through the index, because `Project::roots`
   * is where the paths are and this module already reaches that store through `useIconTheme`.
   * `useShallow` over the mapped array: the store hands back a fresh `ProjectRoot[]` on every
   * accepted mutation in any window, and identity alone would re-render the tree on each one.
   */
  const roots = useWorkspace(
    useShallow((s) =>
      project === null
        ? NO_ROOTS
        : (s.boot?.workspace.projects[project]?.roots.map((r) => r.path) ?? NO_ROOTS),
    ),
  )
  /** The path whose name is currently an `<input>`. At most one row at a time. */
  const [renaming, setRenaming] = useState<string | null>(null)
  /**
   * The last failed file operation, or `null`.
   *
   * Shown rather than only logged. Every verb in the menu below is a Tauri command that can
   * reject — `fs_delete` refuses a root and reports a `PartialDelete`, `fs_rename` refuses a
   * name that already exists, `fs_show_in_manager` fails when the desktop has no handler —
   * and a rejection swallowed by a bare `.then()` is indistinguishable from a menu item wired
   * to nothing, which is the failure this codebase has already shipped twice.
   */
  const [problem, setProblem] = useState<string | null>(null)

  /** Report a rejected file command, in the panel and in the log. */
  const fail = useCallback(
    (what: string) => (error: unknown) => {
      const line = `${what} failed: ${String(error)}`
      setProblem(line)
      void diag.log(`[cide] file tree: ${line}`).catch(() => {})
    },
    [],
  )

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

  /** Perform whatever `clickSemantics` decided, on a row we already have in hand. */
  const apply = useCallback(
    (action: RowAction, row: TreeRow, index: number) => {
      const store = useFileTree.getState()
      if (action.select) store.select(row.path, index)
      if (action.toggle && row.kind === 'dir') void store.toggle(row)
      if (action.open && row.kind === 'file') onOpen?.(row.path)
    },
    [onOpen],
  )

  /**
   * Where the arrows start from.
   *
   * The live index is re-derived from the resident rows rather than trusted from the store:
   * the selection is a *path*, and a watcher burst that inserted a file above it moved every
   * row below without the selection changing at all. `selectedIndex` is the fallback for a
   * selection scrolled far out of the cache; `-1` means nothing is selected yet.
   */
  const cursor = useCallback((): number => {
    const store = useFileTree.getState()
    if (store.selected === null) return -1
    return store.indexOf(store.selected) ?? store.selectedIndex
  }, [])

  const moveTo = useCallback(
    (index: number) => {
      const store = useFileTree.getState()
      // A jump lands where nothing has been fetched. Ask first, then read — an unfetched row
      // cannot be named, and a selection that silently refuses to move on `End` is worse
      // than a frame's delay.
      store.ensure(Math.max(0, index - JUMP_MARGIN), index + JUMP_MARGIN + 1)
      const row = store.rowAt(index)
      if (row !== undefined) store.select(row.path, index)
      virtualizer.scrollToIndex(index, { align: 'auto' })
    },
    [virtualizer],
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // While a row is being renamed the `<input>` owns every key, including Enter and
      // Escape. Handling them here as well would rename the file and move the selection.
      if (renaming !== null) return
      const store = useFileTree.getState()
      const at = cursor()

      if (at < 0) {
        // Nothing selected. Any navigation key means "start at the top" — `moveIndex` from a
        // notional -1 would answer 0 for Home and 1 for ArrowDown, which skips a row.
        if (moveIndex(e.key, 0, count) === null) return
        moveTo(0)
        e.preventDefault()
        return
      }

      const next = moveIndex(e.key, at, count)
      if (next !== null) {
        moveTo(next)
        e.preventDefault()
        return
      }

      const row = store.rowAt(at)
      if (row === undefined) return
      switch (e.key) {
        case 'Enter':
          apply(enterOn({ expandable: row.kind === 'dir' }), row, at)
          break
        case 'ArrowRight':
          if (row.kind === 'dir' && !row.expanded) void store.toggle(row)
          else moveTo(Math.min(at + 1, count - 1))
          break
        case 'ArrowLeft':
          /*
           * Collapse, or step up one row.
           *
           * IDEA's Left goes to the *parent* on an already-collapsed row. That needs a walk
           * back through the flattened list looking for a smaller depth, and in a windowed
           * cache the rows it walks over may not be resident — so it would be an IPC round
           * trip per keystroke, in a handler that has to feel instant. One row up reaches the
           * parent for the common case (the first child) and never stalls.
           */
          if (row.kind === 'dir' && row.expanded) void store.toggle(row)
          else moveTo(Math.max(at - 1, 0))
          break
        default:
          return
      }
      e.preventDefault()
    },
    [apply, count, cursor, moveTo, renaming],
  )

  /**
   * The row a menu gesture landed on, read back out of the DOM.
   *
   * `items` is called at open time with the element under the pointer, so the row is found by
   * walking up to the nearest one rather than by remembering what was last hovered — which is
   * the difference between a menu that acts on what you right-clicked and one that acts on
   * what you clicked before that.
   */
  const rowFacts = useCallback(
    (target: HTMLElement | null) => {
      const el = target?.closest<HTMLElement>('[data-row-path]')
      const path = el?.dataset['rowPath']
      if (el === null || el === undefined || path === undefined) return null
      const root = rootOf(path, roots)
      return {
        path,
        isDir: el.dataset['rowKind'] === 'dir',
        expanded: el.dataset['rowExpanded'] === 'true',
        /** A project root: `fs_rename` and `fs_delete` both refuse one, so the menu does too. */
        isRoot: root === path,
        /**
         * What *Copy Relative Path* copies. The absolute path when no root claims this row,
         * which is the honest answer rather than a relative path against nothing.
         */
        rel: root === null || root === path ? path : path.slice(root.length + 1),
      }
    },
    [roots],
  )

  const { onContextMenu, menu } = useContextMenu({
    label: 'File tree',
    items: ({ target }) => {
      const row = rowFacts(target)
      // Right-clicking the empty space under the last row opens nothing. An empty box at the
      // pointer says "this surface is broken"; `useContextMenu` declines on `[]`.
      if (row === null || project === null) return []
      const store = useFileTree.getState()
      const at = store.indexOf(row.path)
      // Right-click selects too. The menu reads the DOM and does not need it, but a menu that
      // acts on a row the tree is not visibly pointing at is a menu the user cannot check
      // before clicking.
      if (at !== null) store.select(row.path, at)

      const openLabel = row.isDir ? (row.expanded ? 'Collapse' : 'Expand') : 'Open'
      const entries: MenuEntry[] = [
        {
          id: 'open',
          label: openLabel,
          run: () => {
            // A file needs nothing but its path, so it does not go through the row cache at
            // all — an `Open` that quietly did nothing because the row had been evicted
            // between the right-click and the click is the exact failure this app keeps
            // fixing. Only the folder needs the live `TreeRow`, because `toggle` reads
            // `expanded` off it to choose between `fs_expand` and `fs_collapse`.
            if (!row.isDir) {
              onOpen?.(row.path)
              return
            }
            const live = at === null ? undefined : store.rowAt(at)
            if (live !== undefined) void store.toggle(live)
          },
        },
        {
          id: 'openSide',
          label: 'Open to the Side',
          ...sideAction(row.isDir, row.path, onOpenToSide),
        },
        { kind: 'separator' },
        {
          id: 'reveal',
          label: 'Reveal in File Manager',
          run: () => void fsReveal.showInManager(project, row.path).catch(fail('Reveal')),
        },
        { id: 'copyPath', label: 'Copy Path', run: () => void copyText(row.path) },
        { id: 'copyRel', label: 'Copy Relative Path', run: () => void copyText(row.rel) },
        { kind: 'separator' },
        {
          id: 'rename',
          label: 'Rename…',
          ...(row.isRoot
            ? { disabledReason: 'A project root is renamed where the project was opened' }
            : { run: () => setRenaming(row.path) }),
        },
        {
          id: 'delete',
          label: 'Move to Trash',
          danger: true,
          ...(row.isRoot
            ? { disabledReason: 'A project root is closed, not deleted' }
            : {
                run: () => {
                  // No confirmation, deliberately: `fs_delete` moves to the freedesktop trash
                  // and never unlinks, so the desktop's own undo is one keystroke away. A
                  // dialog here would be a second confirmation of a reversible act.
                  void fsApi
                    .delete(project, [row.path])
                    .then(() => useFileTree.getState().refresh())
                    .catch(fail('Move to Trash'))
                },
              }),
        },
      ]
      return entries
    },
  })

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
    <>
      <div
        className={styles.scroll}
        ref={scrollRef}
        data-audit="fileTreeScroll"
        /*
         * One tab stop for the whole tree, with the arrows moving inside it — the ARIA tree
         * pattern, and what every editor does. Per-row `tabIndex` would make Tab walk 100 000
         * stops, and a roving one would have to move focus imperatively on every refresh, which
         * is how a background watcher event steals the caret out of a terminal pane.
         */
        role="tree"
        aria-label="Files"
        tabIndex={0}
        onKeyDown={onKeyDown}
        onContextMenu={onContextMenu}
      >
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
                index={item.index}
                statuses={statuses}
                iconTheme={iconTheme}
                top={item.start}
                height={item.size}
                selected={row.path === selected}
                renaming={row.path === renaming}
                project={project}
                onAct={apply}
                onEndRename={() => setRenaming(null)}
                onFail={fail('Rename')}
              />
            )
          })}
        </div>
        {/* `{menu}` must be rendered or nothing appears. It portals to a sibling of `#root`, so
            this scroll container's `overflow: hidden` cannot clip it. */}
        {menu}
      </div>
      {/*
        * A rejected file command, at the foot of the panel until it is dismissed.
        *
        * A sibling of the scroller and not a child of it, for two reasons: the scroller is
        * `role="tree"`, whose children have to be tree items, and its viewport is as tall as
        * the whole flattened repository — a strip inside it would sit 100 000 rows down.
        *
        * Dismissed by clicking, not on a timer. A message that removes itself is a message the
        * user who looked away never saw, and "the menu item did nothing" is the report this is
        * here to prevent.
        */}
      {problem !== null && (
        <div
          className={styles.problem}
          data-audit="fileTreeProblem"
          role="status"
          title="Dismiss"
          onClick={() => setProblem(null)}
        >
          {problem}
        </div>
      )}
    </>
  )
}

/**
 * *Open to the Side*, or the reason it cannot be.
 *
 * Split out because it is three states and a ternary chain inside the item literal reads as
 * one. A directory has nothing to put in a pane; a build with no host has nowhere to put it.
 */
function sideAction(
  isDir: boolean,
  path: string,
  onOpenToSide: ((path: string) => void) | undefined,
): { disabledReason: string } | { run: () => void } {
  if (isDir) return { disabledReason: 'A folder has nothing to show in a pane' }
  if (onOpenToSide === undefined) {
    return { disabledReason: 'Nothing in this build can open a file beside another' }
  }
  return { run: () => onOpenToSide(path) }
}

interface RowProps {
  row: TreeRow
  index: number
  statuses: TreeStatusMap['statuses']
  /** Threaded from the panel — see the subscription there. */
  iconTheme: IconTheme
  top: number
  height: number
  selected: boolean
  renaming: boolean
  project: ProjectId | null
  onAct: (action: RowAction, row: TreeRow, index: number) => void
  onEndRename: () => void
  /** Where a rejected `fs_rename` goes. See `problem` in the panel. */
  onFail: (error: unknown) => void
}

function Row({
  row,
  index,
  statuses,
  iconTheme,
  top,
  height,
  selected,
  renaming,
  project,
  onAct,
  onEndRename,
  onFail,
}: RowProps) {
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
  const hasTwisty = isDir && row.hasChildren
  const twisty = hasTwisty ? (row.expanded ? '▾' : '▸') : ''

  return (
    <div
      className={selected ? `${styles.row} ${styles.rowSelected}` : styles.row}
      data-audit="fileTreeRow"
      data-depth={row.depth}
      /*
       * Read back by the context menu at open time; see `rowFacts`.
       *
       * `row.root` is deliberately NOT among these. It is an index into `Project::roots`, not
       * a path, and publishing it as `data-row-root` invited exactly the misreading that
       * shipped: the menu treated `"0"` as a directory prefix. The roots come from the
       * workspace store instead, where they are paths.
       */
      data-row-path={row.path}
      data-row-kind={row.kind}
      data-row-expanded={isDir ? String(row.expanded) : undefined}
      role="treeitem"
      aria-level={row.depth + 1}
      aria-selected={selected}
      {...(isDir ? { 'aria-expanded': row.expanded } : {})}
      style={{ height: `${height}px`, transform: `translateY(${top}px)` }}
      title={row.path}
      onMouseDown={(e) => {
        // Middle and right buttons are not this gesture. Right-click still selects, from the
        // menu's own `items` callback, so the row the menu acts on is the row that lights up.
        if (e.button !== 0 || renaming) return
        onAct(
          fileTreeClick({
            gesture: gestureOf(e.detail),
            isDir,
            // The twisty is a control of its own: one click on the arrow folds the folder,
            // because requiring a double-click on an 11px glyph to do the only thing it does
            // would be a worse tree than the one this change is fixing.
            onTwisty: e.target instanceof Element && e.target.closest(`.${styles.twisty}`) !== null,
          }),
          row,
          index,
        )
      }}
    >
      {/* Indentation is a margin on the twisty rather than padding on the row, so the
          selection band still spans the panel at any depth. */}
      <span
        className={styles.twisty}
        style={{ marginLeft: `${row.depth * INDENT}px` }}
        aria-hidden="true"
      >
        {twisty}
      </span>
      {/*
       * The Material Icon Theme, vendored as local files — the CSP names no external host, so
       * `@/icons` resolves to something in `public/icons/`. `TreeRow` satisfies `IconRow`
       * structurally, so the row goes in whole and a field renamed in `cide-fs` breaks here
       * rather than inside the icon module. `row.expanded` already drives the open folder.
       *
       * This replaced the literal `▤`/`▫`. The activity rail still draws `▤` for Files, which
       * is a different claim — that rail names a *panel*, not a directory.
       */}
      <FileIcon row={row} theme={iconTheme} />
      {renaming && project !== null ? (
        <RenameInput project={project} row={row} onDone={onEndRename} onFail={onFail} />
      ) : (
        <span
          className={
            status.nameClass === undefined ? styles.name : `${styles.name} ${status.nameClass}`
          }
        >
          {row.name}
        </span>
      )}
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

/**
 * The rename box, in place of the row's name.
 *
 * In place rather than in a dialog because the tree is 252px wide and the thing being renamed
 * is one word: a modal would cover the very list that gives the name its context.
 *
 * Enter commits, Escape abandons, and blur commits — the last because a rename box the user
 * has clicked away from has to resolve one way or the other, and abandoning silently loses
 * typing that looked accepted. `settled` is what keeps Escape from committing on the blur its
 * own unmount causes.
 */
function RenameInput({
  project,
  row,
  onDone,
  onFail,
}: {
  project: ProjectId
  row: TreeRow
  onDone: () => void
  onFail: (error: unknown) => void
}) {
  const settled = useRef(false)

  const finish = (name: string | null) => {
    if (settled.current) return
    settled.current = true
    onDone()
    if (name === null) return
    const trimmed = name.trim()
    if (trimmed === '' || trimmed === row.name) return
    // The directory is taken from the row's own path rather than re-derived from the tree: a
    // rename never moves a file between folders, and a typed `/` would otherwise silently do
    // exactly that.
    const dir = row.path.slice(0, row.path.length - row.name.length)
    void fsApi
      .rename(project, row.path, `${dir}${trimmed.replaceAll('/', '_')}`)
      .then(() => useFileTree.getState().refresh())
      // `fs_rename` refuses a root and refuses an existing name. Swallowing that would leave
      // the row showing its old name with no hint that anything was attempted.
      .catch(onFail)
  }

  return (
    <input
      className={styles.rename}
      defaultValue={row.name}
      spellCheck={false}
      autoComplete="off"
      aria-label={`Rename ${row.name}`}
      // Text inputs keep the webview's own menu (see `menus/native.ts`), which is what makes
      // Paste work in here at all — the app's menu cannot offer one.
      ref={(el) => {
        if (el === null) return
        el.focus()
        // The extension is left out of the selection: renaming `App.tsx` to `Shell.tsx` is
        // the common case and re-typing `.tsx` every time is the thing that makes people
        // stop using inline rename.
        const dot = row.name.lastIndexOf('.')
        el.setSelectionRange(0, dot > 0 ? dot : row.name.length)
      }}
      onMouseDown={(e) => e.stopPropagation()}
      onBlur={(e) => finish(e.target.value)}
      onKeyDown={(e) => {
        // Every key belongs to the box, not to the tree behind it — otherwise typing `e` in a
        // filename would also be an End key on its way past.
        e.stopPropagation()
        if (e.key === 'Enter') finish(e.currentTarget.value)
        else if (e.key === 'Escape') finish(null)
      }}
    />
  )
}
