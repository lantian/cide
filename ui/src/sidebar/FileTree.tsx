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
import { isRootPath, relativeTo } from './rowPaths'
import { basenameOf, checkName, nameToSend, targetFor, type NewEntryTarget } from './newEntry'
import { useFileClipboard } from './fileClipboard'
import {
  copyLabel,
  escapeCancels,
  isCutPending,
  pasteLabel,
  pasteRefusal,
  pasteTargetFor,
  pendingNote,
  type ClipMode,
} from './clipboardModel'
import { fsMessage } from './fsError'
import { PasteConfirm } from '@/chrome/PasteConfirm'
import {
  answerAsk,
  askDecisions,
  askIsDone,
  cancelledNote,
  startAsk,
  type PasteAnswer,
  type PasteAsk,
} from '@/chrome/pasteConfirm'
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
  /**
   * The unnamed row being typed into, or `null`. See `treeStore`'s field for why it is a row
   * in the list rather than a dialog over it.
   */
  const draft = useFileTree((s) => s.draft)
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
  /**
   * What the draft row is currently saying about the name in it.
   *
   * Lifted out of the input so it can be drawn at the foot of the panel: the row is 21px of a
   * 252px column and there is no room in it for a sentence, and a refusal the user only meets
   * *after* pressing Enter is the thing this whole module is trying not to be.
   */
  const [draftName, setDraftName] = useState('')
  /**
   * Whether Enter has already been pressed on this draft and refused.
   *
   * The empty box deliberately shows its *destination* rather than "Type a file name." — a red
   * message on a field nobody has touched reads as a failure before anything was attempted.
   * But that made Enter on an empty box do nothing at all and say nothing at all, which is the
   * dead control this panel keeps shipping in a new costume. Once Enter has been pressed the
   * verdict is shown for what it is, empty name included.
   */
  const [draftTried, setDraftTried] = useState(false)
  /** A `fs_create_in` is in flight for the open draft. See `commitDraft`. */
  const creating = useRef(false)
  /**
   * Something true and non-alarming to say, or `null`.
   *
   * A second strip rather than a second use of `problem`, because a paste that renamed a file
   * is not a failure and must not be painted red — and because the two are dismissed by
   * different things: a note is stale as soon as the next gesture happens, a failure stays
   * until it is clicked. Sharing one state meant one wiping the other.
   */
  const [note, setNote] = useState<string | null>(null)
  /** What is on the tree's clipboard. Subscribed here because it dims rows and draws a strip. */
  const clip = useFileClipboard((s) => s.clip)
  /**
   * A paste is in flight, **or** waiting on the confirmation.
   *
   * Held for the whole gesture rather than only for the command: Ctrl+V held down must not
   * start a second paste, and it must not stack a second dialog behind the first either. The
   * flag is cleared by whatever ends the gesture — the paste settling, or the user cancelling.
   */
  const pasting = useRef(false)
  /**
   * The collisions the user is being asked about, and where they would land.
   *
   * `null` for every paste with nothing in its way, which is nearly all of them. The dialog
   * lives here rather than in the shell because this panel owns the gesture that opens it —
   * `App.tsx` never needs to know it exists.
   */
  const [pendingPaste, setPendingPaste] = useState<{
    readonly destDir: string
    readonly ask: PasteAsk
  } | null>(null)

  /** Report a rejected file command, in the panel and in the log. */
  const fail = useCallback(
    (what: string) => (error: unknown) => {
      // `fsMessage`, not `String(error)`. An `FsError` crosses the boundary as
      // `{ kind, detail }` with no `message` field, so the template literal that used to be
      // here printed `[object Object]` — a refusal the user could have acted on ("that name is
      // taken", "that is a project root"), rendered as a bug in the message. See `fsError.ts`.
      const line = `${what} failed: ${fsMessage(error)}`
      setProblem(line)
      void diag.log(`[cide] file tree: ${line}`).catch(() => {})
    },
    [],
  )

  /**
   * Put a row on the tree's clipboard.
   *
   * Cut refuses a project root here as well as in Rust, for the reason every other refusal is
   * doubled in this panel: `fs_paste` would reject it after the gesture, and a Ctrl+X that
   * *looks* like it worked and fails a minute later at the paste is the worse half of that
   * exchange — by then the user has forgotten what they cut.
   */
  const takeClip = useCallback(
    (mode: ClipMode, row: { path: string; isRoot: boolean }) => {
      if (project === null) return
      if (mode === 'cut' && row.isRoot) {
        setProblem('A project root is closed, not moved.')
        return
      }
      setProblem(null)
      setNote(null)
      useFileClipboard.getState().take(mode, project, [row.path])
    },
    [project],
  )

  /**
   * Send the paste, with whatever the user answered about the names that were taken.
   *
   * Split from `runPaste` because it is the second half of a gesture that may have paused for
   * a dialog in between: everything that has to happen once, before the question, is up there,
   * and everything that happens once the answer is in is here.
   */
  const commitPaste = useCallback(
    (project_: ProjectId, destDir: string, decisions: ReturnType<typeof askDecisions>) => {
      void useFileClipboard
        .getState()
        .paste(project_, destDir, decisions)
        .finally(() => {
          pasting.current = false
        })
        .then((result) => {
          setNote(result.note)
          const first = result.paths[0]
          // `reveal` rather than `refresh`: the row may be inside a folder that is collapsed,
          // and a paste whose result is not on screen is one the user cannot check. It
          // selects too, which is what makes Enter open the thing that was just pasted.
          if (first !== undefined) void useFileTree.getState().reveal(first)
          else void useFileTree.getState().refresh()
        })
        .catch(fail('Paste'))
    },
    [fail],
  )

  /**
   * Paste the clipboard into `target`, asking first about anything it would land on top of.
   *
   * > *"Paste collisions - yes, should be a confirmation"*
   *
   * `plan` reads and writes nothing, so the ordinary paste — nothing in the way — costs one
   * extra round trip and no dialog, and a paste that *would* overwrite is stopped before a
   * single byte is written. That ordering is why `PasteConfirm` can promise that cancelling
   * leaves everything as it was: there is no partial paste to report, because none started.
   *
   * The refusals are checked here rather than left to Rust so that they arrive as a sentence
   * about folders — "“src” cannot be pasted into itself" — instead of as a rejected command.
   * Rust checks all of it again, in `plan` as well as in the paste; see
   * `clipboardModel.pasteRefusal`.
   */
  const runPaste = useCallback(
    (target: NewEntryTarget | null) => {
      if (project === null) return
      const refusal = pasteRefusal(useFileClipboard.getState().clip, project, target)
      if (refusal !== null || target === null) {
        setProblem(refusal ?? 'There is nowhere to paste into.')
        return
      }
      // One paste per gesture, however long Ctrl+V is held. A directory paste is seconds of
      // work, and a second one launched into it would race the first for the same names —
      // both would succeed, and the user would get `src` and `src copy` from one keystroke.
      // The flag also covers the time the dialog is open, so a held key cannot stack questions.
      if (pasting.current) return
      pasting.current = true
      setProblem(null)
      setNote(null)
      const destDir = target.parent
      void useFileClipboard
        .getState()
        .plan(project, destDir)
        .then((collisions) => {
          if (collisions.length === 0) {
            commitPaste(project, destDir, [])
            return
          }
          setPendingPaste({ destDir, ask: startAsk(collisions) })
        })
        .catch((error: unknown) => {
          pasting.current = false
          fail('Paste')(error)
        })
    },
    [commitPaste, fail, project],
  )

  /**
   * Back out. The clipboard is left alone so the gesture can simply be repeated.
   *
   * The note is not decoration: a dialog that vanishes with nothing said is indistinguishable
   * from a paste that silently failed, and the one thing worth saying here is the property the
   * whole plan-then-paste ordering was built for — nothing was written.
   *
   * Above `answerPaste` because that one depends on it, and a `const` named in a dependency
   * array is read *during* the render that declares it.
   */
  const cancelPaste = useCallback(() => {
    setPendingPaste(null)
    pasting.current = false
    setNote(cancelledNote())
  }, [])

  /** One answer from the dialog. The last one sends the paste. */
  const answerPaste = useCallback(
    (answer: PasteAnswer, applyToRest: boolean) => {
      if (pendingPaste === null) return
      // The project closed while the question was on screen. Backing out rather than returning
      // is the difference between a dialog and a picture of one: an early `return` here left
      // both answers dead on a card the user could only dismiss with Escape, and the paste flag
      // set behind it, so the next Ctrl+V did nothing either.
      if (project === null) {
        cancelPaste()
        return
      }
      const next = answerAsk(pendingPaste.ask, answer, applyToRest)
      if (!askIsDone(next)) {
        setPendingPaste({ destDir: pendingPaste.destDir, ask: next })
        return
      }
      setPendingPaste(null)
      commitPaste(project, pendingPaste.destDir, askDecisions(next))
    },
    [cancelPaste, commitPaste, pendingPaste, project],
  )

  const scrollRef = useRef<HTMLDivElement>(null)
  /**
   * The draft occupies a row that Rust does not know about, so the list is one longer than the
   * tree while it is open and every index from `draft.index` on is shifted down by one.
   *
   * `-1` rather than `null` for "no draft" so the two comparisons below are plain numeric ones
   * — `virtual > -1` is never true for the first row, which is the only case that matters.
   */
  const draftAt = draft?.index ?? -1
  // `max` rather than a plain `+ 1`: a watcher burst can shrink the tree under an open draft
  // — a `git checkout` that removed the folder above it — and a virtualizer sized shorter
  // than the row it is being asked to place would put the box past the bottom of the panel.
  const rowCount = Math.max(count + (draft === null ? 0 : 1), draftAt + 1)
  /** A virtual row index translated back to the flattened index Rust uses. */
  const toReal = useCallback(
    (virtual: number) => (draftAt >= 0 && virtual > draftAt ? virtual - 1 : virtual),
    [draftAt],
  )

  const virtualizer = useVirtualizer({
    count: rowCount,
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
  //
  // The range is translated out of virtual space first. Asking for the *virtual* range would
  // request one row past the end of the tree while a draft is open, which is harmless, and
  // would be off by one for every row below the draft, which is not.
  useEffect(() => {
    if (count > 0) useFileTree.getState().ensure(toReal(first), toReal(last) + 1)
  }, [count, first, last, chunks, toReal])

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
      // While a row is being renamed — or a new one is being named — the `<input>` owns every
      // key, including Enter and Escape. Handling them here as well would rename the file and
      // move the selection, and would answer Escape by cancelling the draft *and* jumping the
      // cursor to whatever row the arrows were last on.
      if (renaming !== null || draft !== null) return
      const store = useFileTree.getState()
      const at = cursor()

      /*
       * Ctrl+C, Ctrl+X, Ctrl+V and Escape — the tree's clipboard.
       *
       * Handled on this element rather than as three entries in `crates/cide-core/src/keymap.rs`,
       * and the reason is the one the brief asks about: these chords are **focus-scoped**, not
       * context-scoped. Ctrl+C in a terminal pane is SIGINT and Ctrl+C in the editor copies the
       * selection, so the question a binding has to answer is "does the file tree have the
       * caret right now" — and the flag that exists for this, `sidebarFiles`, means the panel
       * is *visible*, which it is while the user is typing in a terminal beside it. A global
       * binding would swallow the keystroke there and the pty would never see it. A handler on
       * the scroller is asked only when the scroller is focused, which is the actual question;
       * nothing in the default keymap binds these three, so the window-level gate passes them
       * through untouched and there is no conflict to resolve.
       *
       * `!e.altKey && !e.shiftKey` so `ctrl+shift+c` stays free for whoever wants it.
       */
      const key = e.key.toLowerCase()
      // Only these three chords are taken. Every other modified key falls through to the
      // navigation below, which is deliberate: `moveIndex` reads `e.key` alone, so Ctrl+End
      // has always moved the selection here and a blanket `return` for modified keys would
      // have quietly removed that.
      const clipboardKey = key === 'c' || key === 'x' || key === 'v'
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && clipboardKey) {
        // Rows are ordinary selectable text. When the user has actually dragged a selection
        // across one, Ctrl+C means *that* — copying the row's path instead would be taking a
        // gesture the webview already handles correctly.
        const text = typeof document === 'undefined' ? null : document.getSelection()
        if (key === 'c' && text !== null && !text.isCollapsed) return

        const row = at < 0 ? undefined : store.rowAt(at)
        if (key === 'v') {
          if (row === undefined && store.selected !== null) {
            // The selected row has been scrolled out of the row cache, so its *kind* is
            // unknown — and the kind is what decides whether the paste lands in that folder or
            // beside that file. Refused rather than guessed: putting a directory somewhere the
            // user was not looking is the one outcome worth a round trip to avoid, and this
            // costs a scroll instead.
            setProblem('Scroll back to the selected row before pasting — it is no longer loaded.')
          } else {
            const anchor = row === undefined ? null : { path: row.path, isDir: row.kind === 'dir' }
            runPaste(pasteTargetFor(anchor, roots))
          }
          e.preventDefault()
          return
        }

        const path = row?.path ?? store.selected
        if (path !== null && path !== undefined) {
          takeClip(key === 'x' ? 'cut' : 'copy', { path, isRoot: isRootPath(path, roots) })
        }
        e.preventDefault()
        return
      }
      /*
       * Escape calls off a cut — a *cut*, and only one this panel is currently showing.
       *
       * The condition used to be `clip !== null`, which cancelled two things the user had no
       * way to know were there. A **copy** is deliberately silent (`pendingNote` returns null
       * for it and no row is dimmed), so Escape threw the clipboard away with nothing on
       * screen having changed, and the next Ctrl+V answered "nothing has been copied yet" —
       * indistinguishable from a Copy that never worked, which is the report this panel keeps
       * getting. And a cut belonging to **another project** is announced in that project's
       * panel, not this one: the strip is gated on `clip.project === project` and so are the
       * faded rows, so Escape here was cancelling something drawn somewhere else.
       *
       * So: cancel exactly what this panel is drawing as pending. Anything else falls through,
       * which also keeps Escape available to whoever wants it next — a cut that cannot be
       * cancelled is a trap, and one that eats Escape for the rest of the app is a different
       * trap.
       */
      if (e.key === 'Escape' && escapeCancels(useFileClipboard.getState().clip, project)) {
        useFileClipboard.getState().clear()
        e.preventDefault()
        return
      }

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
    [apply, count, cursor, draft, moveTo, project, renaming, roots, runPaste, takeClip],
  )

  /**
   * Open the inline editor for a new file or folder.
   *
   * The target is decided by `newEntry.ts` — a file row means its *parent*, a folder row means
   * itself, no row means the project's first root — and everything about placing the editor
   * (expanding the folder, finding the anchor row, scrolling to it) is `beginDraft`'s.
   */
  const startDraft = useCallback((target: NewEntryTarget, directory: boolean) => {
    setDraftName('')
    setDraftTried(false)
    // A create still in flight belongs to the draft being replaced, not to this one.
    creating.current = false
    setProblem(null)
    void useFileTree
      .getState()
      .beginDraft(target.parent, directory, target.atTop)
      .then((opened) => {
        if (opened) return
        // No row to anchor the editor to, so nothing opened. Said out loud: a menu item that
        // draws no box and reports nothing is exactly the dead control this panel has already
        // shipped twice.
        setProblem(`${target.label} is not in the tree any more, so nothing can be created in it.`)
      })
      .catch(() => {
        // `beginDraft` only reads the tree, and every read inside it already falls back rather
        // than rejecting. Caught anyway so a future one cannot become an unhandled rejection
        // in a panel with no error surface of its own.
      })
  }, [])

  /**
   * Commit the draft, or refuse it and leave the box open.
   *
   * Refusing *without closing the box* is the point: the name is still there to be fixed. The
   * alternative — close, then complain — throws away the typing that caused the complaint.
   */
  const commitDraft = useCallback(
    (raw: string) => {
      const current = useFileTree.getState().draft
      if (current === null) return
      const name = nameToSend(raw, current.siblings, current.directory)
      // Refused locally. The note strip is driven by the same `checkName` over the same text,
      // so for every *typed* name it is already showing the reason — but the EMPTY box shows
      // its destination instead, on purpose, and without this flag Enter on an empty box did
      // nothing and said nothing. `draftTried` makes the strip stop being polite.
      if (name === null) {
        setDraftTried(true)
        return
      }
      // One create per draft, however many times Enter is pressed.
      //
      // `treeStore.commitDraft` only clears `draft` *after* its round trip, so a second Enter
      // during it passed this guard's absence and issued a second `fs_create_in` for the same
      // name. The second one loses the race it started and reports “already exists” about the
      // file the first one had just made for you — a refusal that is true, useless, and
      // indistinguishable from the gesture having failed.
      if (creating.current) return
      creating.current = true
      const what = current.directory ? 'New Folder' : 'New File'
      void useFileTree
        .getState()
        .commitDraft(name)
        .finally(() => {
          creating.current = false
        })
        .then((created) => {
          if (created !== null) return
          // Created, and no row for it. Almost always a dot-file the project's ignore rules
          // hide. Said out loud rather than left as a menu item that appeared to do nothing.
          setProblem(
            `Created “${name}”, but this project’s ignore rules keep it out of the tree.`,
          )
        })
        .catch((error: unknown) => {
          // The box stays open on a rejection for the same reason it stays open on a local
          // refusal: `already exists` and `is no longer a directory` are both fixable from
          // here, and closing would make the user start the gesture again to find out how.
          fail(what)(error)
        })
    },
    [fail],
  )

  const cancelDraft = useCallback(() => {
    useFileTree.getState().cancelDraft()
    setDraftName('')
    setDraftTried(false)
  }, [])

  /**
   * The line under the draft box: where the entry is going, or why the name will not do.
   *
   * The **empty** box is deliberately not an error. It is empty the moment it opens, and a red
   * "Type a file name." on an untouched field reads as a failure before anything was
   * attempted — so an *untouched* box says where the file is going instead, which is the thing
   * the user cannot otherwise check.
   *
   * "Untouched" has to include "Enter has not been pressed", or the politeness turns into
   * silence: `checkName` refuses the empty name, `commitDraft` declines to close the box, and
   * the strip goes on cheerfully naming a destination — a keypress that does nothing and says
   * nothing. `draftTried` is set by that refusal and shows the real verdict from then on.
   */
  const draftMessage: { text: string; bad: boolean } = (() => {
    if (draft === null) return { text: '', bad: false }
    const where = isRootPath(draft.parent, roots)
      ? basenameOf(draft.parent)
      : relativeTo(draft.parent, roots)
    const destination = `New ${draft.directory ? 'folder' : 'file'} in ${where}`
    if (draftName.trim() === '' && !draftTried) return { text: destination, bad: false }
    const verdict = checkName(draftName, draft.siblings, draft.directory)
    if (verdict.error !== null) return { text: verdict.error, bad: true }
    if (verdict.note !== null) return { text: verdict.note, bad: false }
    return { text: destination, bad: false }
  })()

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
      return {
        path,
        isDir: el.dataset['rowKind'] === 'dir',
        expanded: el.dataset['rowExpanded'] === 'true',
        /** A project root: `fs_rename` and `fs_delete` both refuse one, so the menu does too. */
        isRoot: isRootPath(path, roots),
        /** What *Copy Relative Path* copies. See `rowPaths.ts` for why it is not inline. */
        rel: relativeTo(path, roots),
      }
    },
    [roots],
  )

  /**
   * The clipboard as of *now*, not as of the last render.
   *
   * `items` below is called at open time, so it must read through the store: the subscribed
   * `clip` is correct today only because this component happens to re-render on every change to
   * it, and a menu whose *Paste* item describes a clipboard from two gestures ago is the exact
   * class of bug this panel keeps fixing.
   */
  const clipNow = () => useFileClipboard.getState().clip

  const { onContextMenu, menu } = useContextMenu({
    label: 'File tree',
    items: ({ target }) => {
      const row = rowFacts(target)
      if (project === null) return []

      /*
       * *New File…* and *New Folder…*, which are the two items this menu was missing.
       *
       * Built before the early return below, because the empty space under the last row is
       * the one place they are the *whole* menu: every other item needs a row and these two
       * need only a project. Right-clicking there used to open nothing at all, which was the
       * right answer when there was nothing to offer and is the wrong one now.
       *
       * The destination is named in the label whenever it is not obvious from what was
       * clicked — the empty-space case, and the multi-root case where "the project root" is
       * several different directories. `targetFor` decides which root; this only says so.
       */
      const target_ = targetFor(row === null ? null : { path: row.path, isDir: row.isDir }, roots)
      // Name the destination whenever it is not the thing that was clicked. A **file** row is
      // the case that matters: *New File* on `src/main.rs` creates a sibling in `src`, not
      // something "inside" a file, and the label is where the user finds that out — before the
      // gesture rather than by looking for the row afterwards.
      const named = row === null || !row.isDir
      const create: MenuEntry[] =
        target_ === null
          ? []
          : [
              {
                id: 'newFile',
                label: named ? `New File in ${target_.label}…` : 'New File…',
                run: () => startDraft(target_, false),
              },
              {
                id: 'newFolder',
                label: named ? `New Folder in ${target_.label}…` : 'New Folder…',
                run: () => startDraft(target_, true),
              },
            ]

      /*
       * *Paste*, built alongside them and for the same reason: it needs a destination, not a
       * row, so the empty space under the last row can offer it too. Disabled **with the
       * reason on it** rather than hidden — an item that appears only sometimes teaches the
       * user nothing about why, and "nothing has been copied yet" is the answer to the
       * question they are actually asking.
       */
      const refusal = pasteRefusal(clipNow(), project, target_)
      const paste: MenuEntry = {
        id: 'paste',
        label: pasteLabel(clipNow(), target_),
        ...(refusal === null ? { run: () => runPaste(target_) } : { disabledReason: refusal }),
      }

      // Right-clicking the empty space under the last row offers the three items above and
      // nothing else. An empty box at the pointer says "this surface is broken";
      // `useContextMenu` declines on `[]`, which is still the answer for a project-less panel.
      if (row === null) return [...create, { kind: 'separator' }, paste]

      const store = useFileTree.getState()
      const at = store.indexOf(row.path)
      // Right-click selects too. The menu reads the DOM and does not need it, but a menu that
      // acts on a row the tree is not visibly pointing at is a menu the user cannot check
      // before clicking.
      if (at !== null) store.select(row.path, at)

      const openLabel = row.isDir ? (row.expanded ? 'Collapse' : 'Expand') : 'Open'
      const entries: MenuEntry[] = [
        // First, as every editor puts them. They are the only items here that make something
        // rather than acting on what is already there, which is also why they take the
        // separator below rather than sharing a group with Open.
        ...create,
        { kind: 'separator' },
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
        /*
         * Cut / Copy / Paste of the **files**, above the two items that copy their *names*.
         *
         * The order is every file manager's, and the grouping is what keeps the four apart:
         * *Copy* and *Copy Path* are one keystroke and one menu row from each other and mean
         * completely different things, so they do not share a group. Ctrl+C in the tree is
         * this one — the files — because that is what Ctrl+C means in every other list of
         * files a user has ever met.
         */
        { id: 'cut', label: copyLabel('cut', 1), ...cutAction(row.isRoot, row.path, takeClip) },
        {
          id: 'copy',
          label: copyLabel('copy', 1),
          run: () => takeClip('copy', { path: row.path, isRoot: row.isRoot }),
        },
        paste,
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

  /**
   * The pending-cut strip, or `null`.
   *
   * Gated on the clipboard belonging to *this* project. The clipboard deliberately survives a
   * project switch (see `fileClipboard.ts`), so without this the panel for project B would
   * carry a strip about a file in project A — one that `pasteRefusal` will not let it paste.
   */
  const pending = clip !== null && clip.project === project ? pendingNote(clip) : null

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
            // The draft's slot is left empty here and filled below, outside the window. See
            // the note on the `<DraftRow>` element for why it is not drawn from this map.
            if (item.index === draftAt) return null
            const row = useFileTree.getState().rowAt(toReal(item.index))
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
                index={toReal(item.index)}
                statuses={statuses}
                iconTheme={iconTheme}
                top={item.start}
                height={item.size}
                selected={row.path === selected}
                cut={isCutPending(clip, row.path)}
                renaming={row.path === renaming}
                project={project}
                onAct={apply}
                onEndRename={() => setRenaming(null)}
                onFail={fail('Rename')}
              />
            )
          })}
          {/*
            * The one row in this list that is not in the tree, drawn *outside* the virtual
            * window rather than from the map above.
            *
            * Every row size here is `ROW_HEIGHT`, so its offset is arithmetic and needs no
            * virtual item — and being outside the window is the whole point: the virtualizer
            * unmounts what scrolls past its overscan, and unmounting a focused `<input>`
            * fires `blur`, which cancels the draft. A user who scrolled the tree while
            * choosing a name would have watched their half-typed filename disappear.
            *
            * It is positioned at the parent's first-child slot with the parent's depth plus
            * one, so it lines up with the siblings it is about to join.
            */}
          {draft !== null && (
            <DraftRow
              directory={draft.directory}
              depth={draft.depth}
              iconTheme={iconTheme}
              top={draftAt * ROW_HEIGHT}
              height={ROW_HEIGHT}
              value={draftName}
              onChange={setDraftName}
              onCommit={commitDraft}
              onCancel={cancelDraft}
            />
          )}
        </div>
        {/* `{menu}` must be rendered or nothing appears. It portals to a sibling of `#root`, so
            this scroll container's `overflow: hidden` cannot clip it. */}
        {menu}
      </div>
      {/*
        * What the draft row is about to do, and what is wrong with the name in it.
        *
        * At the foot of the panel rather than in the row, because the row is 21px of a 252px
        * column and every one of these sentences is longer than that. It also has to say
        * *where* — a new file's destination is the whole thing the user cannot check by
        * looking at a name box, and in a multi-root project "the project root" is several
        * different directories.
        *
        * A separate strip from `.problem` below on purpose: this one is live and disappears
        * with the draft, that one is a failure that stays until it is dismissed. Sharing an
        * element would mean a rejected `fs_create_in` was wiped by the next keystroke.
        */}
      {draft !== null && (
        <div
          className={draftMessage.bad ? styles.draftError : styles.draftNote}
          data-audit="fileTreeDraftNote"
          role="status"
        >
          {draftMessage.text}
        </div>
      )}
      {/*
        * A cut waiting to be pasted.
        *
        * The dimmed rows say *which* files; this says what will happen to them and how to call
        * it off. Only for a cut — see `pendingNote`: a copy changes nothing until it is pasted
        * and announcing it would be a permanent strip under a panel that is 252px wide.
        */}
      {pending !== null && (
        <div className={styles.draftNote} data-audit="fileTreeClipNote" role="status">
          {pending}
        </div>
      )}
      {/*
        * What a paste did, when it is not what was asked for.
        *
        * Its own strip and not `.problem`: a rename is a success, and painting it red would
        * teach the user that pasting a file is an error. `null` most of the time — the tree
        * scrolls to and selects what it made, so the ordinary paste needs no sentence at all.
        * Dismissed by clicking, like the failure strip below.
        */}
      {note !== null && (
        <div
          className={styles.info}
          data-audit="fileTreePasteNote"
          role="status"
          title="Dismiss"
          onClick={() => setNote(null)}
        >
          {note}
        </div>
      )}
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
      {/*
        * The paste confirmation.
        *
        * Mounted by the panel that owns the gesture rather than by the shell: it is opened only
        * by a Ctrl+V or a *Paste* menu item in this tree, its state has exactly this panel's
        * lifetime, and putting it here is what makes the feature reachable without an `App.tsx`
        * edit. `OverlayCard`'s scrim is `position: fixed`, so it covers the window from here
        * exactly as it would from the root — and being a sibling of the scroller rather than a
        * child keeps it out of the `role="tree"` subtree.
        *
        * Nothing has been written when this is on screen; see `pasteConfirm.ts`.
        */}
      {pendingPaste !== null && (
        <PasteConfirm ask={pendingPaste.ask} onAnswer={answerPaste} onCancel={cancelPaste} />
      )}
    </>
  )
}

/**
 * The unnamed row a new file or folder is typed into.
 *
 * A sibling of [`RenameInput`] and deliberately not the same component. They look alike and
 * they are not the same thing: rename edits a row that exists — Escape leaves the file alone
 * and blur commits, because typing the user has walked away from must resolve — while this
 * stands in for a row that does *not* exist yet, so blur has to **cancel**. Committing on blur
 * here would mean clicking anywhere in the app created a file, which is the shape of accident
 * a file tree cannot afford. `Enter` is the only thing that creates anything.
 *
 * It draws no git tag. There is no status for a path that is not on disk, and an empty 9px
 * column keeps the input the same width it will be once the row is real, so nothing jumps when
 * the draft is replaced by the file it made.
 */
function DraftRow({
  directory,
  depth,
  iconTheme,
  top,
  height,
  value,
  onChange,
  onCommit,
  onCancel,
}: {
  directory: boolean
  depth: number
  iconTheme: IconTheme
  top: number
  height: number
  value: string
  onChange: (value: string) => void
  onCommit: (value: string) => void
  onCancel: () => void
}) {
  /**
   * Focus once, on the first element this ref sees.
   *
   * The input is *controlled* — the panel owns the text so it can validate it as it is typed
   * — so this component re-renders on every keystroke, and an unguarded `el?.focus()` in an
   * inline ref would run on each of them. Harmless today and a trap tomorrow: it is one
   * mis-ordered render away from stealing focus back from whatever the user moved to.
   */
  const focused = useRef(false)
  const input = useRef<HTMLInputElement | null>(null)

  /*
   * Take focus back when the *window* does.
   *
   * Paired with the `document.hasFocus()` guard on `onBlur` below, and pointless without it.
   * Alt-tabbing to a browser to copy a filename fires `blur` on this input exactly as clicking
   * off it does, so cancelling on every blur meant coming back to no box and no typing — work
   * thrown away by a gesture the user did not make. The guard keeps the draft; this puts the
   * caret back in it, because a mounted box nobody can type into is worse than either.
   *
   * Safe to focus unconditionally: this component only exists while a draft is open, and any
   * in-app click that could have moved focus somewhere the user wanted would have cancelled
   * the draft and unmounted it.
   */
  useEffect(() => {
    const back = () => input.current?.focus()
    window.addEventListener('focus', back)
    return () => window.removeEventListener('focus', back)
  }, [])

  return (
    <div
      className={`${styles.row} ${styles.rowSelected}`}
      data-audit="fileTreeDraftRow"
      data-depth={depth}
      role="treeitem"
      aria-level={depth + 1}
      aria-selected={true}
      style={{ height: `${height}px`, transform: `translateY(${top}px)` }}
    >
      {/* No twisty glyph even for a folder: it has no children to disclose and drawing one
          would invite a click that folds nothing. The span is still here because it is the
          indentation. */}
      <span className={styles.twisty} style={{ marginLeft: `${depth * INDENT}px` }} aria-hidden="true" />
      {/* The icon follows the *name being typed*, so `main.rs` turns into the Rust icon as it
          is spelled. `iconFor` reads only these three fields, which is why a literal works
          where the real rows pass a whole `TreeRow`. */}
      <FileIcon
        row={{ name: value, kind: directory ? 'dir' : 'file', expanded: false }}
        theme={iconTheme}
      />
      <input
        className={styles.rename}
        value={value}
        spellCheck={false}
        autoComplete="off"
        placeholder={directory ? 'folder name' : 'file name'}
        aria-label={directory ? 'Name for the new folder' : 'Name for the new file'}
        ref={(el) => {
          input.current = el
          if (el === null || focused.current) return
          focused.current = true
          el.focus()
        }}
        onChange={(e) => onChange(e.target.value)}
        onMouseDown={(e) => e.stopPropagation()}
        // Cancel, not commit — see the component's own note. A file created by looking away
        // is a file nobody meant to make.
        //
        // `document.hasFocus()` separates the two blurs that look identical here: the user
        // clicking off the box, which is the abandon this is for, and the *window* going away,
        // which is alt-tab and is not an answer to anything. It is false only in the second
        // case; the effect above puts the caret back when the window returns.
        onBlur={() => {
          if (document.hasFocus()) onCancel()
        }}
        onKeyDown={(e) => {
          // Every key belongs to the box. Without this, typing `e` in a filename is also an
          // End key on its way past the tree's own handler.
          e.stopPropagation()
          if (e.key === 'Enter') onCommit(e.currentTarget.value)
          else if (e.key === 'Escape') onCancel()
        }}
      />
      <span className={styles.tag} />
    </div>
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

/**
 * *Cut*, or the reason a root cannot be one.
 *
 * The same shape as [`sideAction`] and the same rule as Rename and Move to Trash: `fs_paste`
 * refuses to move a project root (`ops::check_not_root`), so the menu says so up front rather
 * than letting the user cut something and discover at the paste that it was never going
 * anywhere. Copy has no such restriction — duplicating a checkout beside itself is a real
 * thing to want.
 */
function cutAction(
  isRoot: boolean,
  path: string,
  take: (mode: ClipMode, row: { path: string; isRoot: boolean }) => void,
): { disabledReason: string } | { run: () => void } {
  if (isRoot) return { disabledReason: 'A project root is closed, not moved' }
  return { run: () => take('cut', { path, isRoot }) }
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
  /** On the clipboard for a **cut**: drawn faded, because it is about to move. */
  cut: boolean
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
  cut,
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

  // Composed rather than a ternary chain: a row can be selected *and* pending a cut, which is
  // the ordinary case — Ctrl+X acts on the selection.
  const rowClass = [styles.row, selected ? styles.rowSelected : null, cut ? styles.rowCut : null]
    .filter((name) => name !== undefined && name !== null)
    .join(' ')

  return (
    <div
      className={rowClass}
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
            /*
             * The twisty is a control of its own: one click on the arrow folds the folder,
             * because requiring a double-click on an 11px glyph to do the only thing it does
             * would be a worse tree than the one this change is fixing.
             *
             * `hasTwisty` guards it, and that guard is load-bearing rather than tidy. The
             * span is drawn on *every* row — it is the indentation, so a file has one too,
             * empty and 11px wide — and without the guard those 11px were a strip down the
             * left of each file where a double-click resolved to "second half of a twisty
             * gesture", which is `NOTHING`: the file simply refused to open.
             */
            onTwisty:
              hasTwisty &&
              e.target instanceof Element &&
              e.target.closest(`.${styles.twisty}`) !== null,
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
