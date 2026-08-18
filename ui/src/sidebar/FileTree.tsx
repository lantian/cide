/**
 * The virtualized 24px file tree.
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
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { blurLeftTheTree } from '@/sidebar/speedSearch'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useShallow } from 'zustand/react/shallow'
import { useFileTree } from './treeStore'
import { useGitStatus } from './gitStatusStore'
import { letterFor, statusAt } from './treeStatus'
import {
  enterOn,
  fileTreeClick,
  gestureOf,
  moveIndex,
  treeKeyAction,
  type RowAction,
} from './clickSemantics'
import {
  actionScope,
  collapseTo,
  NO_MODS,
  pressMenu,
  type SelectMods,
} from './treeSelection'
import { inDrag, inDropBand, type DragRow } from './treeDrag'
import { useSpeedSearch } from './useSpeedSearch'
import { SpeedName, SpeedSearchBar } from './SpeedSearchBar'
import { useTreeDrag, type TreeDragState } from './useTreeDrag'
import { clearFocusRequest, useFocusRequested } from '@/chrome/focusRequests'
import { copyText } from './copyText'
import { creationRefusal, isRootPath, mutationRefusal, relativeTo, rootOf } from './rowPaths'
import { groupIcon, groupIdOf, isSyntheticPath, rowVerbs } from './groupRows'
import { basenameOf, checkName, nameToSend, targetFor, type NewEntryTarget } from './newEntry'
import { pasteEntries, planEntries, useFileClipboard } from './fileClipboard'
import {
  clipboardText,
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
import { ConfirmDestructive, type ConfirmState } from '@/chrome/ConfirmDestructive'
import { PasteConfirm } from '@/chrome/PasteConfirm'
import {
  answerAsk,
  askDecisions,
  askIsDone,
  cancelledNote,
  moveCancelledNote,
  startAsk,
  type PasteAnswer,
  type PasteAsk,
} from '@/chrome/pasteConfirmModel'
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

/**
 * 24px rows — IDEA's `Tree.rowHeight` at 100% scale, measured off `idea.png`.
 *
 * **This deliberately departs from the mock**, which specifies 21px rows, and it is the same
 * override as the mono→proportional face change two lines below: `Grount IDE.dc.html` is not
 * the reference for this panel any more, `idea.png` is. Do not "restore the mock" here
 * without reading that comment first. The 24 is measured, not chosen — IDEA's selection band
 * is 24px tall and its seven-row `target` subtree tint is exactly 168px.
 *
 * Three numbers move together and nothing but this comment enforces it: `SearchPanel.tsx`
 * uses the same row height for the same reason, `ChangesTree.module.css` states it in CSS
 * because that tree is not virtualized, and `icons/FileIcon.module.css` sizes the icon box
 * to centre on a whole pixel *in a row of this parity*. Changing this to an odd number
 * without revisiting the icon box puts every icon in all three lists back on a half-pixel
 * grid — see that file, which exists because of exactly that bug.
 */
const ROW_HEIGHT = 24

/** The `fs_*` calls the panel makes, in the order the notice should prefer to name them. */
const PENDING_COMMANDS = ['fs_tree_count', 'fs_tree_rows', 'fs_expand', 'fs_collapse'] as const

/**
 * 19px per depth, measured off IDEA rather than taken from the mock's 12.
 *
 * This was the single clearest miss in the side-by-side, and it is exact rather than
 * estimated: in `idea.png` the *same* closed-folder glyph at depths 0/1/2 has its ink left
 * edge at x = 73, 92, 111 — a step of 19, twice — and the chevrons (75→94) and the label ink
 * (91→110→129) agree to the pixel. At 12px a nested tree reads as a flat list, which is most
 * of why our explorer looked wrong next to IDEA's even before the face changed.
 *
 * `GitPanel/ChangesTree.tsx` uses the same 19. IDEA indents both its trees by one number and
 * ours used two (12 here, 14 there), which is a difference nobody chose.
 */
const INDENT = 19

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

/*
 * Why a project root refuses each of the two verbs, in the words the user reads.
 *
 * Constants because each sentence now has two surfaces — the context menu greys the row and
 * shows it as the reason, the key handler prints it into the problem strip after the fact —
 * and two gestures that refuse the same thing for two differently-worded reasons is a small
 * lie about there being two rules. `cide_fs::ops::check_not_root` refuses both in Rust as
 * well; these say it early enough to be useful, which is the only thing they add.
 */
const ROOT_NOT_RENAMED = 'A project root is renamed where the project was opened'
const ROOT_NOT_DELETED = 'A project root is closed, not deleted'

/**
 * `data-row-kind`, as read back out of the DOM.
 *
 * A cast rather than a validated parse, and narrowed to the generated union so a variant added
 * in Rust reaches `rowVerbs`'s exhaustive switch. That switch has a `default` arm precisely
 * because this cast is a claim about a string attribute rather than a proof.
 */
type RowKindAttr = TreeRow['kind']

/**
 * Some paths, where they are going, and how they were picked.
 *
 * The one shape behind both gestures that put files in a folder — Ctrl+V and a drop — so that the
 * collision question, the dialog, the cancellation note and the reveal-what-landed are written
 * once. They were not, at first: the drag arrived with the paste's four handlers already in
 * place, and copying them would have produced a second confirmation flow that agrees with the
 * first only for as long as nobody edits either.
 *
 * `fromClip` is the whole of the difference, and it is a fact about *provenance*, not about the
 * operation: a clipboard cut is consumed by the paste it was made for, a dropped set never
 * touched the clipboard at all.
 */
interface Transfer {
  readonly sources: readonly string[]
  readonly destDir: string
  readonly mode: ClipMode
  /** The sources came off the clipboard, so a successful cut consumes it. */
  readonly fromClip: boolean
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
  /**
   * A **pinned** row was opened — *Project Notes*. The argument is the pin's id, not a path.
   *
   * An id and not a path because a pin's `path` is a `cide://group/…` sentinel that names
   * nothing on disk: the file behind it is created and named by Rust (`fs_notes_ensure`), and
   * handing the sentinel to `onOpen` would reach `tab_open_file` with a string `check_within`
   * refuses. The host turns the id into a **command**, which is what keeps the double-click, the
   * palette row and any future chord one code path — the same routing `onSelectOpened` uses.
   *
   * Optional for the reason `onOpenToSide` is: a host that cannot open tabs must not have this
   * panel pretend otherwise. Without it the row's click does nothing and its menu item is the
   * only thing that would have — see the menu below.
   */
  onOpenPin?: ((id: string) => void) | undefined
}

export function FileTree({ project, onOpen, onOpenToSide, onOpenPin }: FileTreeProps) {
  const count = useFileTree((s) => s.count)
  const chunks = useFileTree((s) => s.chunks)
  const degraded = useFileTree((s) => s.degraded)
  const revealTo = useFileTree((s) => s.revealTo)
  const selected = useFileTree((s) => s.selected)
  /**
   * Every highlighted row. `selected` above is the *cursor* — see the two fields in
   * `treeStore`, which are one thing for a plain click and two the moment Ctrl is held.
   */
  const selection = useFileTree((s) => s.selection)
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
  /**
   * Where the disk-changing verbs may act: the roots, **plus** whatever `fs_writable_roots`
   * added — today, the *Scratches* drawer.
   *
   * A second list rather than a wider `roots`, because the two answer different questions and
   * three call sites below depend on the difference. `relativeTo` and `isRootPath` mean
   * *project roots* — a scratch shown with a relative path would read as a file in the project,
   * and the drawer counted as a root would be refused by `check_not_root` for the wrong reason —
   * while `pasteTargetFor` picks the root a paste with no anchor lands in, which must never be
   * the drawer. So `roots` keeps its meaning and this is the containment set, matching
   * `ProjectFs::writable_paths` exactly.
   *
   * The union is taken here rather than in Rust's answer so that the roots are right from the
   * first frame: `writable` is `[]` until an IPC round trip lands, and a menu that greyed
   * *Rename…* on every project file for that frame would be a flicker in the one control this
   * change is about.
   */
  const extraWritable = useFileTree((s) => s.writable)
  const writable = useMemo(() => [...roots, ...extraWritable], [roots, extraWritable])

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
   * Lifted out of the input so it can be drawn at the foot of the panel: the row is 24px of a
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
   * The collisions the user is being asked about, and the transfer they belong to.
   *
   * `null` for every transfer with nothing in its way, which is nearly all of them. The dialog
   * lives here rather than in the shell because this panel owns both gestures that open it —
   * `App.tsx` never needs to know it exists.
   */
  const [pendingPaste, setPendingPaste] = useState<{
    readonly transfer: Transfer
    readonly ask: PasteAsk
  } | null>(null)

  /**
   * The pending *Move to Trash*, or `null`.
   *
   * State rather than a call straight to `fsApi.delete`, because there are now **two** gestures
   * that ask for it — the menu item and the Delete key — and they must not be able to disagree
   * about whether a confirmation appears. Both build this; the dialog is rendered once at the
   * foot of the panel beside `PasteConfirm`.
   */
  const [pendingDelete, setPendingDelete] = useState<ConfirmState | null>(null)

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
   * Ask before moving a row to the trash — the one path both gestures take.
   *
   * # Why there is a confirmation now, when there deliberately was not one before
   *
   * The menu item shipped without one and the reasoning was sound: `fs_delete` moves to the
   * freedesktop trash and never unlinks, so the desktop's own undo is one keystroke away, and a
   * dialog is a second confirmation of a reversible act. That argument is about the *act*, and
   * it survives. What did not survive is its unstated premise about the *gesture*: right-click,
   * travel to the bottom of a nine-item menu, click a red row — a sequence nobody performs by
   * accident.
   *
   * Delete is one key, unmodified, sitting a finger away from the arrow keys this same handler
   * uses to move the selection, in a panel whose whole job is being navigated by keyboard. The
   * cost of a misfire goes from "impossible by accident" to "trivially likely", and the recovery
   * is a desktop trash the user has to go and find — which `cide_fs::ops` itself calls "not an
   * undo anyone wants to need". So the key got a confirmation, and the menu item was routed
   * through the same one rather than left as it was: two gestures for one act that disagree
   * about whether it is dangerous teach the user nothing, and the next person to touch either
   * one has to work out which is right.
   *
   * The body says the act is reversible, because it is. A dialog that implies otherwise about
   * a trash move is the kind of small lie that makes people stop reading dialogs.
   */
  const askDelete = useCallback(
    (paths: readonly string[]) => {
      if (project === null || paths.length === 0) return
      // One root anywhere in the selection refuses the whole gesture rather than quietly
      // deleting the rest around it. `fs_delete` would refuse that one path and report a
      // `PartialDelete` *after* the others were already in the trash, which is a confirmation
      // dialog that named more files than it acted on — the one thing this dialog exists to
      // rule out.
      if (paths.some((path) => isRootPath(path, roots))) {
        setProblem(`${ROOT_NOT_DELETED}.`)
        return
      }
      setProblem(null)
      setPendingDelete({
        title: paths.length > 1 ? `Move ${paths.length} items to Trash?` : 'Move to Trash?',
        body:
          'This goes to the desktop trash, not to nowhere — it can be restored from there. ' +
          'Nothing is removed from disk.',
        files: [...paths],
        confirmLabel: paths.length > 1 ? `Move ${paths.length} items` : 'Move to Trash',
        run: () => {
          void fsApi
            .delete(project, [...paths])
            .then(() => {
              // The selection named these rows and these rows are gone. Left standing it would
              // be a set of dead paths that the next Ctrl+X or Delete would send to Rust, which
              // is the one refusal the user has no way to act on.
              useFileTree.getState().clearSelection()
              return useFileTree.getState().refresh()
            })
            .catch(fail('Move to Trash'))
        },
      })
    },
    [project, roots, fail],
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
    (mode: ClipMode, paths: readonly string[]) => {
      if (project === null || paths.length === 0) return
      // Same all-or-nothing rule as the delete above: a Cut that silently dropped the root out
      // of a five-row selection would move four files and leave the fifth, with the strip
      // claiming five.
      if (mode === 'cut' && paths.some((path) => isRootPath(path, roots))) {
        setProblem('A project root is closed, not moved.')
        return
      }
      setProblem(null)
      setNote(null)
      useFileClipboard.getState().take(mode, project, paths)
    },
    [project, roots],
  )

  /**
   * Send the transfer, with whatever the user answered about the names that were taken.
   *
   * Split from `startTransfer` because it is the second half of a gesture that may have paused
   * for a dialog in between: everything that has to happen once, before the question, is up
   * there, and everything that happens once the answer is in is here.
   *
   * The two sources of a transfer part company on exactly one line. A **clipboard** paste goes
   * through the store, because a successful cut consumes the clip and only the store owns that;
   * a **drop** goes straight to `pasteEntries` with the paths it is carrying. Sending a drop
   * through the store would mean calling `take()` first — which overwrites the user's clipboard,
   * and the system clipboard with it, for a gesture that has nothing to do with either.
   */
  const commitTransfer = useCallback(
    (project_: ProjectId, transfer: Transfer, decisions: ReturnType<typeof askDecisions>) => {
      const sent = transfer.fromClip
        ? useFileClipboard.getState().paste(project_, transfer.destDir, decisions)
        : pasteEntries(project_, transfer.sources, transfer.destDir, transfer.mode, decisions)
      void sent
        .finally(() => {
          pasting.current = false
        })
        .then((result) => {
          setNote(result.note)
          const first = result.paths[0]
          // `reveal` rather than `refresh`: the row may be inside a folder that is collapsed,
          // and a transfer whose result is not on screen is one the user cannot check. It
          // selects too, which is what makes Enter open the thing that was just moved — and for
          // a drop it is the whole receipt, since the row left the place the user was looking at.
          if (first !== undefined) void useFileTree.getState().reveal(first)
          else void useFileTree.getState().refresh()
        })
        .catch(fail(transfer.fromClip ? 'Paste' : 'Move'))
    },
    [fail],
  )

  /**
   * Plan the transfer, ask about anything it would land on top of, then send it.
   *
   * > *"Paste collisions - yes, should be a confirmation"*
   *
   * `plan` reads and writes nothing, so the ordinary transfer — nothing in the way — costs one
   * extra round trip and no dialog, and one that *would* overwrite is stopped before a single
   * byte is written. That ordering is why `PasteConfirm` can promise that cancelling leaves
   * everything as it was: there is no partial paste to report, because none started.
   *
   * **A drag lands here too, deliberately.** The hazard is identical — a name already taken in
   * the destination — and the user asked for a confirmation on exactly this hazard once already.
   * Two dialogs for one question is how one of them ends up defaulting to *Replace*; there is one
   * dialog, and its default is *Keep both*, which is what makes a dropped folder recoverable in a
   * feature with no undo.
   */
  const startTransfer = useCallback(
    (transfer: Transfer) => {
      if (project === null) return
      // One transfer per gesture, however long Ctrl+V is held. A directory paste is seconds of
      // work, and a second one launched into it would race the first for the same names —
      // both would succeed, and the user would get `src` and `src copy` from one keystroke.
      // The flag also covers the time the dialog is open, so a held key cannot stack questions,
      // and it is what stops a second drop landing while the first is still being answered.
      if (pasting.current) return
      pasting.current = true
      setProblem(null)
      setNote(null)
      void planEntries(project, transfer.sources, transfer.destDir, transfer.mode)
        .then((collisions) => {
          if (collisions.length === 0) {
            commitTransfer(project, transfer, [])
            return
          }
          setPendingPaste({ transfer, ask: startAsk(collisions) })
        })
        .catch((error: unknown) => {
          pasting.current = false
          fail(transfer.fromClip ? 'Paste' : 'Move')(error)
        })
    },
    [commitTransfer, fail, project],
  )

  /**
   * Paste the clipboard into `target`.
   *
   * The refusals are checked here rather than left to Rust so that they arrive as a sentence
   * about folders — "“src” cannot be pasted into itself" — instead of as a rejected command.
   * Rust checks all of it again, in `plan` as well as in the paste; see
   * `clipboardModel.pasteRefusal`.
   */
  const runPaste = useCallback(
    (target: NewEntryTarget | null) => {
      if (project === null) return
      const clip = useFileClipboard.getState().clip
      const refusal = pasteRefusal(clip, project, target)
      if (refusal !== null || target === null || clip === null) {
        setProblem(refusal ?? 'There is nowhere to paste into.')
        return
      }
      startTransfer({
        sources: clip.paths,
        destDir: target.parent,
        mode: clip.mode,
        fromClip: true,
      })
    },
    [project, startTransfer],
  )

  /**
   * Drop `sources` into `destDir` — the drag's landing.
   *
   * Called by `useTreeDrag` only for a `move` verdict, so every refusal in `treeDrag.ts` has
   * already been applied and drawn on the ghost. It is still a *cut*, and it is still planned
   * first: the tree can change between the last pointer move and the release (a watcher burst, an
   * agent writing a file), and Rust re-checks every containment rule regardless. What this must
   * never do is call `take()` — see `commitTransfer`.
   */
  const runMove = useCallback(
    (sources: readonly string[], destDir: string) => {
      if (sources.length === 0) return
      startTransfer({ sources, destDir, mode: 'cut', fromClip: false })
    },
    [startTransfer],
  )

  /**
   * Back out. The clipboard is left alone so the gesture can simply be repeated.
   *
   * The note is not decoration: a dialog that vanishes with nothing said is indistinguishable
   * from a transfer that silently failed, and the one thing worth saying here is the property the
   * whole plan-then-send ordering was built for — nothing was written. It names the gesture the
   * user actually made, because "Paste cancelled" after a drag describes something that never
   * happened.
   *
   * Above `answerPaste` because that one depends on it, and a `const` named in a dependency
   * array is read *during* the render that declares it.
   */
  const cancelPaste = useCallback(() => {
    const fromClip = pendingPaste?.transfer.fromClip ?? true
    setPendingPaste(null)
    pasting.current = false
    setNote(fromClip ? cancelledNote() : moveCancelledNote())
  }, [pendingPaste])

  /** One answer from the dialog. The last one sends the transfer. */
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
        setPendingPaste({ transfer: pendingPaste.transfer, ask: next })
        return
      }
      setPendingPaste(null)
      commitTransfer(project, pendingPaste.transfer, askDecisions(next))
    },
    [cancelPaste, commitTransfer, pendingPaste, project],
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

  /*
   * *Select opened file* asked for the keyboard. Give it to the scroller and spend the request.
   *
   * The scroller is the tree's single tab stop (`role="tree"`, `tabIndex={0}` below), so
   * focusing it is the whole of "the arrows now move in the tree" — which is what was missing:
   * ⌃⇧E from a terminal lit up a row and left every subsequent arrow key going to the pty.
   *
   * A parked request rather than a `focus()` in the dispatcher, for the two reasons
   * `chrome/focusRequests.ts` gives: this panel is usually not mounted when the command runs
   * (the sidebar was on Git, or shut), and when it *is* mounted nothing new mounts, so an
   * `autoFocus` would silently do nothing on the second press. Consumed — not merely observed —
   * so a request cannot fire later on an unrelated mount, which for this panel would mean
   * clicking the Files icon in the rail snatching the caret out of a terminal.
   */
  const wantsFocus = useFocusRequested('fileTree')
  useEffect(() => {
    if (!wantsFocus) return
    scrollRef.current?.focus()
    clearFocusRequest('fileTree')
  }, [wantsFocus])

  /**
   * Show a refused selection gesture, or say nothing.
   *
   * Every selection call resolves to a sentence or to `null`, because the one thing that can go
   * wrong is a band wider than `RANGE_ROWS` — and a Shift-click that quietly selected nothing
   * would be indistinguishable from a Shift-click the tree ignored.
   */
  const reported = useCallback((refusal: string | null) => {
    if (refusal !== null) setProblem(refusal)
  }, [])

  /**
   * The row whose press deferred its collapse, waiting for the release.
   *
   * A ref and not state: nothing renders from it, and a `setState` on mousedown is a re-render
   * that the drag's own 4px threshold logic then has to survive — the same reasoning
   * `ChangesTree` gives for the identical ref one panel over.
   */
  const deferredPress = useRef<string | null>(null)

  /** Perform whatever `clickSemantics` decided, on a row we already have in hand. */
  const apply = useCallback(
    (action: RowAction, row: TreeRow, index: number, mods: SelectMods = NO_MODS) => {
      const store = useFileTree.getState()
      if (action.select) {
        /*
         * Every selecting press goes through `pressRow`, plain or modified, and that is a change
         * this feature needed. The plain press used to call `store.select`, which collapses
         * unconditionally — so pressing on one of five selected files destroyed the set before
         * the pointer had moved a pixel, and "drag the elements I selected" could only ever have
         * dragged one. `pressSelect` now answers *deferred* for that case and the collapse waits
         * for the mouseup, where it is skipped if the gesture became a drag.
         *
         * `select` is still the store's method for a reveal, a paste landing and a fresh file —
         * gestures with no release to wait for.
         */
        const press = store.pressRow(row.path, index, mods)
        deferredPress.current = press.deferred ? row.path : null
        void press.settled.then(reported)
      }
      if (mods.ctrl || mods.shift) {
        /*
         * A modified press assembles a selection and does **nothing else** — no fold, no open —
         * which is the same gate `ChangesTree` puts on its own diff-opening press.
         *
         * The folding half is the sharper of the two. Rows here are addressed by *index* while
         * a Shift-band is being resolved, and expanding a folder re-flattens everything below
         * it: a Ctrl-click that landed on a twisty would renumber the rows the next Shift-click
         * measures against, so the band would cover files nobody pointed at. The opening half is
         * merely obvious — Ctrl+double-click on four files should not leave four tabs open.
         */
        return
      }
      // `rowVerbs`, not `kind === 'dir'` / `kind === 'file'`. A group header folds; a note does
      // nothing at all; a dependency source opens exactly as a project file does — and the tab
      // it opens is read-only, because `file_read` clears `writable` for anything under a
      // toolchain's dependency cache. See `groupRows.ts` for why there is no confirmation here.
      const verbs = rowVerbs(row.kind, true)
      if (action.toggle && verbs.expandable) void store.toggle(row)
      if (action.open && verbs.openable) {
        /*
         * A **pin** is openable and has no path: its `path` is the same `cide://group/…`
         * sentinel a header carries, so handing it to `onOpen` would call `tab_open_file` with a
         * string `cide_fs::ops::check_within` refuses — a click that reports an error about a
         * row the user was told they could open. The id is what the host turns into a command,
         * and the command is what learns the real path from Rust.
         *
         * This is the single place clicks, Enter and a speed-search accept all open through, so
         * the branch is written once.
         */
        const pin = row.kind === 'pin' ? groupIdOf(row.path) : null
        if (pin !== null) onOpenPin?.(pin)
        else onOpen?.(row.path)
      }
    },
    [onOpen, onOpenPin, reported],
  )

  /**
   * The live row for a path — what the drag hit-tests against.
   *
   * Through the store rather than out of a captured array, because this tree is windowed: the
   * only thing the DOM can give a drag is `data-row-path`, and everything else the drop rules
   * need (is it a folder, is it open, does it have children, what is it called) lives on the
   * `TreeRow`. `indexOf` searches resident chunks only, which is exactly the right scope — a row
   * under the pointer is on screen, and a row on screen was rendered from a resident chunk.
   */
  const rowFor = useCallback((path: string): DragRow | null => {
    const store = useFileTree.getState()
    const at = store.indexOf(path)
    return at === null ? null : (store.rowAt(at) ?? null)
  }, [])

  /**
   * Unfold a folder the pointer has rested on for 600 ms — spring loading.
   *
   * You cannot drop into a folder you cannot see, and the alternative is to abandon the drag,
   * expand the folder and start again, which is the gesture people give up on. Which rows qualify
   * is `treeDrag.springTarget`; the timer is the hook's.
   */
  const springOpen = useCallback((path: string) => {
    const store = useFileTree.getState()
    const at = store.indexOf(path)
    const row = at === null ? undefined : store.rowAt(at)
    if (row !== undefined) void store.toggle(row)
  }, [])

  /**
   * Dragging rows into a folder.
   *
   * The rules are `treeDrag.ts` and the pointer state machine is `useTreeDrag.ts`; what is here
   * is the wiring, and the wiring is the half this project has shipped broken four times. Two
   * things make it reachable rather than merely present: `onMove` is passed (without it the hook
   * refuses to start a gesture at all rather than running a drag that lands nowhere), and the
   * rows below carry `onPointerDown` plus the three `data-` attributes the stylesheet draws from.
   */
  const drag = useTreeDrag({
    carried: selection.paths,
    roots,
    writable,
    container: scrollRef,
    rowFor,
    onSpring: springOpen,
    onMove: runMove,
  })

  /**
   * The release of a press whose collapse was deferred.
   *
   * Skipped when the gesture became a drag: the whole selection has just been moved and it is
   * still the right selection, so collapsing to the one row the pointer happened to be on would
   * undo the widening the drag existed to carry.
   */
  /**
   * The load in flight as a set, built once per drag rather than once per row per frame.
   *
   * `inDrag` asks it about a row's ancestors, so this is what keeps the dim independent of how
   * many rows were picked up — Ctrl+A can select 10 000 of them.
   */
  const inFlight = useMemo(
    () => (drag.state === null ? null : new Set(drag.state.drag.paths)),
    [drag.state],
  )

  const releasePress = useCallback(
    (path: string, index: number) => {
      if (drag.dragged()) {
        deferredPress.current = null
        return
      }
      if (deferredPress.current !== path) return
      deferredPress.current = null
      useFileTree.getState().releaseRow(path, index)
    },
    [drag],
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
    (index: number, mods: SelectMods = NO_MODS) => {
      const store = useFileTree.getState()
      // A jump lands where nothing has been fetched. Ask first, then read — an unfetched row
      // cannot be named, and a selection that silently refuses to move on `End` is worse
      // than a frame's delay.
      store.ensure(Math.max(0, index - JUMP_MARGIN), index + JUMP_MARGIN + 1)
      const row = store.rowAt(index)
      if (row !== undefined) {
        // Shift extends the band from the anchor and Ctrl moves the cursor without touching
        // what is highlighted; `keySelect` owns both rules, so this only has to hand them over.
        if (mods.ctrl || mods.shift) void store.keyToRow(row.path, index, mods).then(reported)
        else store.select(row.path, index)
      }
      virtualizer.scrollToIndex(index, { align: 'auto' })
    },
    [virtualizer, reported],
  )

  /**
   * Type-ahead over the rows that are on screen. (M15)
   *
   * The rules are in `speedSearch.ts` and in `cide_fs::speed`; the machinery is in
   * `useSpeedSearch`. What is decided *here* is the three adapters, and each is a fact about
   * this tree rather than about the feature:
   *
   *   * **`search`** goes to `fs_tree_match`, because these rows are not in the webview. The
   *     panel holds 200-row chunks of a flattening Rust owns, so a match at row 40,000 exists
   *     only if Rust is the one looking.
   *   * **`land`** is `moveTo`, unchanged and unwrapped. It already asks for the window around
   *     a far index before reading it and already scrolls — which is exactly why jumping to an
   *     arbitrary matched row needed no new code at all.
   *   * **`revision`** is the row cache. `refresh` installs a fresh `chunks` map precisely when
   *     a burst moved a row, and this panel already subscribes to it — so the staleness guard
   *     costs no new store subscription, which `check:tree-flicker` would otherwise see as a
   *     re-render per watcher burst.
   */
  const speedSearch = useSpeedSearch({
    search: useMemo(
      () => (project === null ? null : (query: string) => fsApi.treeMatch(project, query)),
      [project],
    ),
    land: moveTo,
    accept: useCallback(() => {
      const store = useFileTree.getState()
      const at = cursor()
      const row = at < 0 ? undefined : store.rowAt(at)
      if (row === undefined) return
      // The same call Enter already makes below, through the same rule — so accepting a match
      // opens a file, expands a directory and starts a group's resolution exactly as pressing
      // Enter on that row would. A second spelling here would be a second Enter.
      apply(enterOn({ expandable: rowVerbs(row.kind, true).expandable }), row, at)
    }, [apply, cursor]),
    count,
    revision: chunks,
  })

  /*
   * A project switch takes the search with it.
   *
   * `attach` throws away the selection and the draft for the same reason and says so at
   * length: a query's match list is a list of *indices into the old project's flattening*, so
   * a search that survived the switch would put the cursor on an unrelated file in a tree the
   * user has only just started looking at.
   */
  const exitSearch = speedSearch.exit
  useEffect(() => exitSearch(), [project, exitSearch])

  /**
   * Dismiss the trash confirmation, run whatever it was confirming, and give the tree back the
   * caret.
   *
   * The last part is the one worth explaining. `ConfirmDestructive` focuses its Cancel button
   * on mount, so answering it leaves the caret on a button that is about to be unmounted, i.e.
   * on `<body>` — and Delete is a *repeatable* gesture. Without this, deleting two files in a
   * row means the second Delete goes nowhere and the tree looks like it stopped listening,
   * which is the exact "the key does nothing" report this panel is being fixed for. The
   * scroller is the tree's single tab stop (`role="tree"`, `tabIndex={0}`), so it is the right
   * thing to hand back to.
   *
   * Focused *before* the dialog unmounts, deliberately: moving focus out of an element is safe,
   * while removing a focused element drops the caret on `<body>` and there is nothing to catch
   * it. The one ordering that cannot work is `setPendingDelete(null)` and then a `focus()` in
   * an effect, which is a frame of the tree not answering its own keys.
   */
  const closeDelete = useCallback((run: (() => void) | null) => {
    scrollRef.current?.focus()
    setPendingDelete(null)
    run?.()
  }, [])

  /**
   * Open the inline rename editor on a row, from either gesture that asks for it.
   *
   * `moveTo` before `setRenaming`, and that ordering is the whole of why this is a function
   * rather than a bare `setRenaming` at each call site. The editor is rendered *by the row*
   * (`renaming={row.path === renaming}` below), so a selection that has been scrolled out of
   * the windowed row cache has no row to render it — Ctrl+R would set the state and put nothing
   * on screen, which is this codebase's signature failure wearing a new hat. `moveTo` fetches
   * the window around the index and scrolls to it, so by the time the state lands there is a
   * row to hold the input. It is idempotent for a row already on screen.
   */
  const startRename = useCallback(
    (at: number, row: { path: string; isRoot: boolean }) => {
      if (row.isRoot) {
        setProblem(`${ROOT_NOT_RENAMED}.`)
        return
      }
      // A group header, a note, or a dependency source: `fs_rename` refuses all three via
      // `check_within`, and the box would open over a row nothing could rename. Said out loud
      // and in the same words the menu greys the item with — see `mutationRefusal`.
      const refusal = mutationRefusal([row.path], writable)
      if (refusal !== null) {
        setProblem(`${refusal}.`)
        return
      }
      setProblem(null)
      if (at >= 0) moveTo(at)
      setRenaming(row.path)
    },
    [moveTo, roots],
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      // While a row is being renamed — or a new one is being named — the `<input>` owns every
      // key, including Enter and Escape. Handling them here as well would rename the file and
      // move the selection, and would answer Escape by cancelling the draft *and* jumping the
      // cursor to whatever row the arrows were last on.
      if (renaming !== null || draft !== null) return
      /*
       * Speed search, and it is **first** — above the clipboard block, above `treeKeyAction`,
       * above the navigation.
       *
       * Order is the safety property here, not a preference. Two keys make it so:
       *
       *   * **Escape.** The branch below cancels a pending cut and then collapses a multi-row
       *     selection. A user who typed three letters and pressed Escape to call the search off
       *     would otherwise have thrown away their clipboard cut instead.
       *   * **Delete.** `treeKeyAction` answers a bare Delete with *Move to Trash* over the
       *     selection — and the selection is wherever the search has just moved it. `speedKey`
       *     answers `swallow` for that key while a query is armed, and a branch running after
       *     the clipboard block could not have.
       *
       * A key the search does not want comes back `false` and every rule below runs exactly as
       * it did before this feature existed, including the modified chords: Ctrl+C, Ctrl+X,
       * Ctrl+V, Ctrl+A and Ctrl+R end the search and then do their own job.
       */
      if (speedSearch.onKeyDown(e)) return
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
        /*
         * When the user has a live document selection, Ctrl+C means *that* — copying the
         * row's path instead would be taking a gesture the webview already handles correctly.
         *
         * This used to say "rows are ordinary selectable text", which was true only by
         * accident: `styles/tokens.css` has set `user-select: none` on the body since it was
         * written, and WebKitGTK dropped the declaration because it was spelled without the
         * `-webkit-` prefix. With that fixed, the rows of this tree are genuinely unselectable
         * and this branch is unreachable *from inside them*. It stays, because the check is on
         * `document`, not on the tree: the caret can be here while a selection is live in an
         * editor, a diff pane or the commit box, all of which opt back in with
         * `user-select: text`, and copying a path out from under one of those would be the
         * same theft in the other direction.
         */
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
          } else if (
            row !== undefined &&
            (mutationRefusal([row.path], writable) ?? creationRefusal(row.path, roots)) !== null
          ) {
            // Pasting *into* a dependency source, onto a group header, or into the scratch
            // drawer. The first two `fs_paste` refuses outright; the third it would accept,
            // and refusing it here is a policy — see `creationRefusal`. The honest place to
            // say so is before the copy dialog rather than after it. The same two rules the
            // context menu's Paste item greys itself with, in the same order.
            setProblem(
              `${mutationRefusal([row.path], writable) ?? creationRefusal(row.path, roots) ?? ''}.`,
            )
          } else {
            const anchor = row === undefined ? null : { path: row.path, isDir: row.kind === 'dir' }
            runPaste(pasteTargetFor(anchor, roots))
          }
          e.preventDefault()
          return
        }

        // Everything that is highlighted, which for a single click is the one row and after a
        // Ctrl- or Shift-click is the set. The row under the cursor is deliberately not a
        // fallback — see `actionScope`: the cursor can sit on a row that is *not* selected, and
        // cutting one of those would move a file with nothing on screen marking it.
        //
        // A **cut** is a mutation and a **copy** is not, which is why only one of them is gated:
        // copying a dependency's source into the project is a real thing to want, and `fs_paste`
        // reads the source and writes only into the destination. Moving one out of the registry
        // would break every other project on the machine that depends on it.
        const scope = actionScope(store.selection)
        const refusal = key === 'x' ? mutationRefusal(scope, writable) : null
        if (refusal !== null) setProblem(`${refusal}.`)
        else takeClip(key === 'x' ? 'cut' : 'copy', scope)
        e.preventDefault()
        return
      }

      /*
       * Ctrl+A — select every row in the tree.
       *
       * Handled here rather than as a `tree.selectAll` command in the registry, for the reason
       * the block above and `treeKeyAction` both give, and this is the case where it matters
       * most: the key gate resolves global bindings on a *window capture* listener, so a
       * registry `ctrl+a` would be swallowed before it reached the commit message box, the
       * rename input, CodeMirror or any terminal — every place where Ctrl+A means "select all
       * of this text" or "go to the start of the line". `ChangesTree` reached the same
       * conclusion for the same tree one panel over.
       *
       * `!e.shiftKey` leaves `ctrl+shift+a` free; it is IDEA's "Find Action" and belongs to
       * nobody here yet.
       */
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && key === 'a') {
        void store.selectAll().then(reported)
        e.preventDefault()
        return
      }
      /*
       * Delete and Ctrl+R — rename and move-to-trash on the selected row.
       *
       * Both were built, both worked end to end, and both were reachable from exactly one
       * place: the context menu. There has never been an `F2` anywhere in this repo and no
       * `tree.*` command in the registry, so a user who pressed the key every other file tree
       * answers got nothing, which is indistinguishable from the feature not existing.
       *
       * Handled here rather than in `crates/cide-core/src/keymap.rs` for the same reason as the
       * clipboard block above, and it is a sharper case: a global `delete` binding is resolved
       * by the key gate's *window capture* listener, which runs before the event reaches its
       * target — so it would swallow Delete inside the rename input two screens down, inside
       * the draft-name input, inside the commit message box (a bare `<textarea>` with no key
       * handler of its own), inside CodeMirror, and inside every terminal. `stopPropagation` in
       * those components cannot help; the capture listener has already run. And Ctrl+R in a
       * shell or a Claude pane is readline's reverse-i-search, which people use constantly.
       *
       * The decision itself — which chords, and which modifier combinations are *not* these
       * chords — is `treeKeyAction` in `clickSemantics.ts`, where `check-tree-status.mjs` can
       * hold it. Shift+Delete in particular is left alone rather than treated as a delete.
       *
       * Below the `renaming !== null || draft !== null` early return at the top of this
       * handler, so neither fires while an inline editor owns the keyboard — a Delete in the
       * rename box must delete a character, not the file being renamed.
       */
      const action = treeKeyAction(e.key, {
        ctrl: e.ctrlKey,
        meta: e.metaKey,
        alt: e.altKey,
        shift: e.shiftKey,
      })
      if (action !== null) {
        if (action === 'delete') {
          // Every highlighted row, which is what the confirmation then names one per line.
          // Nothing selected is not a refusal: the key is genuinely not ours in that state, so
          // it goes back to the browser rather than being swallowed with a shrug.
          const scope = actionScope(store.selection)
          if (scope.length === 0) return
          // A group header or a dependency source in the set. `fs_delete` refuses both, and a
          // confirmation dialog listing a file cide will then decline to trash is worse than
          // the refusal it is standing in for.
          const refusal = mutationRefusal(scope, writable)
          if (refusal !== null) setProblem(`${refusal}.`)
          else askDelete(scope)
          e.preventDefault()
          return
        }
        /*
         * Rename is the one verb in this panel that cannot take a list — there is one inline
         * `<input>` and one new name — so it acts on the **cursor** even when several rows are
         * highlighted, which is what every editor does with F2 over a multi-selection.
         *
         * The live row's path when it is resident, the store's remembered selection when it is
         * not. Unlike Paste, neither of these needs the row's *kind*, so a row evicted by
         * scrolling is not a reason to refuse — `startRename` scrolls back to it.
         */
        const path = (at < 0 ? undefined : store.rowAt(at)?.path) ?? store.selected
        if (path === null || path === undefined) return
        startRename(at, { path, isRoot: isRootPath(path, roots) })
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
      if (e.key === 'Escape') {
        if (escapeCancels(useFileClipboard.getState().clip, project)) {
          useFileClipboard.getState().clear()
          e.preventDefault()
          return
        }
        /*
         * Then, and only then, collapse a multi-row selection back to the cursor.
         *
         * Second in line deliberately: the cut is the thing this panel is visibly *announcing*
         * — a strip at the foot and a row of faded rows — so Escape answers that first and the
         * selection on the press after it. Both are one keystroke away, and the order matches
         * what the user can see.
         *
         * `> 1` keeps the ordinary state out of it. Escape with one row selected is not a
         * gesture anyone makes, and swallowing it there would take Escape away from whatever
         * wants it next — the same argument the cut branch above already makes.
         */
        if (store.selection.paths.size > 1) {
          store.setSelection(collapseTo(store.selected), store.selected, store.selectedIndex)
          e.preventDefault()
          return
        }
      }

      // Ctrl moves the cursor and leaves the selection alone; Shift extends the band from the
      // anchor. `moveTo` hands both to `keySelect`, which is the same rule the mouse takes.
      const mods: SelectMods = { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey }

      if (at < 0) {
        // Nothing selected. Any navigation key means "start at the top" — `moveIndex` from a
        // notional -1 would answer 0 for Home and 1 for ArrowDown, which skips a row.
        if (moveIndex(e.key, 0, count) === null) return
        moveTo(0, mods)
        e.preventDefault()
        return
      }

      const next = moveIndex(e.key, at, count)
      if (next !== null) {
        moveTo(next, mods)
        e.preventDefault()
        return
      }

      const row = store.rowAt(at)
      if (row === undefined) return
      const verbs = rowVerbs(row.kind, true)
      switch (e.key) {
        case 'Enter':
          // A group header is `expandable`, so Enter opens it — which is what starts the
          // dependency resolution, and is the keyboard's only route to it besides the palette.
          apply(enterOn({ expandable: verbs.expandable }), row, at)
          break
        case 'ArrowRight':
          if (verbs.expandable && !row.expanded) void store.toggle(row)
          else moveTo(Math.min(at + 1, count - 1), mods)
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
          if (verbs.expandable && row.expanded) void store.toggle(row)
          else moveTo(Math.max(at - 1, 0), mods)
          break
        default:
          return
      }
      e.preventDefault()
    },
    [
      apply,
      askDelete,
      count,
      cursor,
      draft,
      moveTo,
      project,
      renaming,
      reported,
      roots,
      runPaste,
      speedSearch,
      startRename,
      takeClip,
    ],
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
      const kind = (el.dataset['rowKind'] ?? 'file') as RowKindAttr
      return {
        path,
        kind,
        isDir: kind === 'dir',
        expanded: el.dataset['rowExpanded'] === 'true',
        /** A project root: `fs_rename` and `fs_delete` both refuse one, so the menu does too. */
        isRoot: isRootPath(path, roots),
        /**
         * What this row lets the user do. Read here, once, rather than by each item asking
         * about `kind` again — six call sites asking `kind === 'dir'` and meaning six different
         * things is the state `groupRows.ts` exists to end.
         *
         * `writable`, not `roots`: a scratch is outside every root and is still renamed, cut
         * and trashed, because cide owns the directory it is in. That is the *only* thing
         * `inProject` was ever asking, and answering it from the list Rust checks against is
         * what stops the menu and the handler drifting into two rules.
         */
        verbs: rowVerbs(kind, rootOf(path, writable) !== null),
      }
    },
    [roots, writable],
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
       * A **note** row offers nothing, and offers it by not opening a menu at all.
       *
       * It is a sentence — *`cargo` is not on PATH…* — and every item below is about a file.
       * Falling through to the empty-space branch would have offered *New File in cide…* from a
       * right-click on an error message, which is a menu acting on something the user was not
       * pointing at. `useContextMenu` declines on `[]`.
       */
      if (row !== null && row.kind === 'note') return []

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
      /*
       * Why the disk-changing verbs are off for *this row*, or null.
       *
       * One call, read by five items below, so the menu cannot end up offering *Cut* on a row
       * it refuses to *Rename*. A group header and a dependency source are the two populations
       * it catches; `mutationRefusal` is the same rule `check_within` applies in Rust, said
       * early enough to grey a row instead of erroring after the click.
       */
      const rowRefusal = row === null ? null : mutationRefusal([row.path], writable)
      /*
       * And the narrower one: a row cide may *rename* but whose directory nobody fills.
       *
       * A scratch is the only such row. `mutationRefusal` passes it — Rename, Cut and *Move to
       * Trash* all work — while `New File in 4f2a9c7b…` beside it would name a blake3 and drop
       * an ordinary file into the drawer. See `creationRefusal`.
       */
      const fillRefusal =
        rowRefusal ?? (row === null ? null : creationRefusal(row.path, roots))
      // `null` when nothing here may be created in: a header, a directory under
      // `~/.cargo/registry`, or the scratch drawer. Deliberately not "fall back to the project
      // root" — a *New File in cide…* item on a right-click over `serde` would create a file
      // somewhere the user was not pointing at, which is the failure the labelled destination
      // exists to prevent.
      const target_ =
        fillRefusal !== null
          ? null
          : targetFor(row === null ? null : { path: row.path, isDir: row.isDir }, roots)
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
       *
       * `fillRefusal` first: with a dependency source or a scratch under the pointer, `target_`
       * is null and `pasteRefusal` would say "this project has no folder to paste into", which
       * is a true sentence about the wrong thing.
       */
      const refusal = fillRefusal ?? pasteRefusal(clipNow(), project, target_)
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
      /*
       * Right-click selects too. The menu reads the DOM and does not need it, but a menu that
       * acts on a row the tree is not visibly pointing at is a menu the user cannot check
       * before clicking.
       *
       * `pressMenu` is what makes that true for more than one row: a right-click *inside* an
       * existing selection keeps it, and anywhere else collapses to the row under the pointer.
       * The plain `select` that used to be here destroyed the selection with the very gesture
       * that opens the menu meant to act on it, so *Move 5 items to Trash* could not be reached.
       */
      const marked = pressMenu(store.selection, row.path)
      store.setSelection(marked, row.path, at ?? store.selectedIndex)
      /**
       * The rows this menu's verbs act on: exactly what the tree is now showing as selected,
       * minus anything that is not a file.
       *
       * The filter is not cosmetic. `actionScope` answers with paths, and a Ctrl-click that
       * added the *External Libraries* header to the selection would otherwise put
       * `cide://group/externalLibraries` into *Copy 3 Paths* and into the trash confirmation's
       * list — a dialog naming a row that is not a file, about to call a command that refuses it.
       */
      const scope = actionScope(marked).filter((path) => !isSyntheticPath(path))
      const many = scope.length > 1

      /*
       * A **group header** offers one thing: the twisty it already has.
       *
       * Every item below is about a file, and this row is not one — it has no path on disk, no
       * git status, no name to copy and nothing to open in a pane. Returning a one-item menu
       * rather than a disabled version of the full one is the honest shape: fourteen greyed rows
       * with fourteen identical reasons teaches nothing and looks broken.
       */
      /*
       * A **pin** offers one thing too, and it is the opposite of the header's: *Open*.
       *
       * Same shape and same reasoning as the group branch below — every item past this point is
       * about a file, and this row is not one. It has no path to copy (its `path` is a
       * sentinel), nothing to reveal in a file manager, and nothing to rename: the row is pinned
       * to one path, so a rename would leave it pointing at nothing. Returning one live item
       * rather than fifteen greyed ones is the honest shape.
       *
       * `row.path` rather than a live `rowAt`, unlike the header: opening needs no `expanded`
       * flag off the current row, so an eviction between the right-click and the click cannot
       * make this quietly do nothing.
       */
      if (row.kind === 'pin') {
        const id = groupIdOf(row.path)
        return [
          {
            id: 'open',
            label: 'Open',
            ...(id === null || onOpenPin === undefined
              ? { disabledReason: 'This window cannot open editor tabs' }
              : { run: () => onOpenPin(id) }),
          },
        ]
      }

      if (row.kind === 'group') {
        const live = at === null ? undefined : store.rowAt(at)
        return [
          {
            id: 'open',
            label: row.expanded ? 'Collapse' : 'Expand',
            ...(live === undefined
              ? { disabledReason: 'Scroll back to the row before folding it' }
              : { run: () => void store.toggle(live) }),
          },
        ]
      }

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
        {
          id: 'cut',
          label: copyLabel('cut', scope.length),
          ...cutAction(scope, roots, writable, takeClip),
        },
        {
          id: 'copy',
          label: copyLabel('copy', scope.length),
          run: () => takeClip('copy', scope),
        },
        paste,
        { kind: 'separator' },
        {
          // The one verb here that is about a *place* rather than a set of files. A file
          // manager is asked to show one directory, so it gets the row that was clicked even
          // when several are selected — opening five windows is not what anyone means.
          //
          // Disabled for a dependency source rather than enabled by relaxing Rust's check.
          // `fs_show_in_manager` calls `check_within`, and widening a containment check so a
          // menu item looks better is the kind of change that should be argued on its own
          // rather than slipped into a feature — see the note in `cmd::fs`.
          id: 'reveal',
          label: 'Reveal in File Manager',
          ...(rowRefusal === null
            ? { run: () => void fsReveal.showInManager(project, row.path).catch(fail('Reveal')) }
            : {
                disabledReason:
                  'cide opens a file manager only on paths inside the project',
              }),
        },
        /*
         * The two that copy *names* rather than files, and they follow the selection: with
         * four rows highlighted, a *Copy Paths* that handed over one of them would be a menu
         * item that quietly ignored the other three.
         *
         * One absolute path per line, from the same `clipboardText` that Copy already puts on
         * the system clipboard — so the text a Copy leaves behind and the text these two write
         * are the same shape, and pasting either into a terminal works.
         */
        {
          id: 'copyPath',
          label: many ? `Copy ${scope.length} Paths` : 'Copy Path',
          run: () => void copyText(clipboardText(scope)),
        },
        {
          id: 'copyRel',
          label: many ? `Copy ${scope.length} Relative Paths` : 'Copy Relative Path',
          run: () => void copyText(clipboardText(scope.map((path) => relativeTo(path, roots)))),
        },
        { kind: 'separator' },
        /*
         * Rename and Move to Trash. Neither acts here: both hand off to the same function the
         * Delete and Ctrl+R keys call, which is the point.
         *
         * These two used to be the *only* way to reach either verb — no key anywhere in the app
         * renamed or deleted a file — and they still carry no key chip, because a chip is drawn
         * from resolving a command id through the keymap (`useContextMenu`'s `chipFor`) and
         * these chords have no command id: they are focus-scoped and handled on the scroller.
         * That is the same trade Cut/Copy/Paste make three groups up, and it is the accepted
         * one here; see `treeKeyAction` for why a registry command would be worse.
         *
         * The refusals stay `disabledReason` rather than becoming `run` calls that report,
         * because a menu can grey a row and say why *before* it is clicked. The key path has no
         * such surface, so it says the same sentence into the problem strip afterwards — same
         * words, from the same constant, so the two cannot drift into two different rules.
         */
        {
          // Always the row that was clicked, never the set: there is one inline `<input>` and
          // one new name. See the Ctrl+R branch in `onKeyDown`, which makes the same choice.
          id: 'rename',
          label: 'Rename…',
          // Two refusals, most specific first. A project root is refused for a reason about
          // *roots*; a dependency source is refused for a reason about *containment*, and
          // saying the wrong one of the two would send the user looking in the wrong place.
          ...(row.isRoot
            ? { disabledReason: ROOT_NOT_RENAMED }
            : rowRefusal !== null
              ? { disabledReason: rowRefusal }
              : { run: () => startRename(at ?? -1, row) }),
        },
        {
          id: 'delete',
          label: many ? `Move ${scope.length} Items to Trash` : 'Move to Trash',
          danger: true,
          // A root anywhere in the selection greys the item, not just a root under the pointer
          // — `askDelete` refuses the same set for the same reason, and the two must agree or
          // the menu enables something the handler then declines. Same for a path outside the
          // project: the Delete key checks the whole scope with the same function.
          ...(scope.some((path) => isRootPath(path, roots))
            ? { disabledReason: ROOT_NOT_DELETED }
            : (mutationRefusal(scope, writable) ?? null) !== null
              ? { disabledReason: mutationRefusal(scope, writable) ?? '' }
              : { run: () => askDelete(scope) }),
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
      {/*
        * What is being typed, and how many rows it found.
        *
        * A sibling of the scroller and never a child, for the two reasons every strip in this
        * panel is one: the scroller is `role="tree"`, whose children have to be tree items, and
        * its viewport is as tall as the whole flattened repository — a box inside it would sit a
        * hundred thousand rows down. It floats over the top-left corner of the tree, which is
        * IDEA's placement and the only spot that does not push the rows down as it appears.
        *
        * This is not decoration. In a windowed tree most matches are never rendered at all, so
        * the counter — not the highlight — is what makes the feature real; the `<mark>` is the
        * confirmation once the scroll has arrived.
        */}
      {speedSearch.active && (
        <SpeedSearchBar
          query={speedSearch.query}
          summary={speedSearch.summary}
          audit="fileTreeSpeedSearch"
        />
      )}
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
        /* Ctrl- and Shift-click build a set here, so the tree has to say so: a screen reader
           reads `aria-selected` on several rows as a bug in a tree that claims single select. */
        aria-multiselectable={true}
        tabIndex={0}
        /* One flag for the whole box while a drag is in flight: it turns off the hover wash,
           which would otherwise follow the pointer *and* the drop outline — two bands making two
           different claims about where the files are going. */
        {...(drag.state === null ? {} : { 'data-dragging': '' })}
        onKeyDown={onKeyDown}
        /*
         * Every other way out of a speed search, and each one is necessary.
         *
         * A **press** anywhere in the tree is the user choosing a row with the pointer, which is
         * an answer to the same question the query was asking; leaving the box up over a
         * selection it did not make would be the tree lying about what it is filtered to.
         *
         * A **blur** matters more. A query left armed while the caret goes to a terminal would
         * eat the first letters typed on the way back — the tree's `onKeyDown` only runs while
         * the scroller has focus, so the search would sit there invisible until it did. The
         * capture phase, so a press on a row that moves focus is caught either way.
         *
         * Not on the *toggle* separately: Left, Right and a click on a twisty all pass through
         * one of these two, and `speedKey` answers the keyboard half with `exitThenPass`.
         */
        onPointerDownCapture={speedSearch.exit}
        onBlur={(e) => {
          // Only when focus really left the tree — `onBlur` is the bubbling
          // `focusout`, so landing on a match fires it too. See `blurLeftTheTree`.
          if (blurLeftTheTree(e.currentTarget, e.relatedTarget)) speedSearch.exit()
        }}
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
            const flight = drag.state
            /*
             * The drop's three answers for *this* row, computed here rather than in `Row` so the
             * row stays a renderer. All three are `null`/`undefined` unless a drag is in flight,
             * which is the ordinary case and costs one comparison.
             *
             * `mark` is the destination folder for an accepted drop and the row under the pointer
             * for a refused one — see `TreeDropOutcome`. It is a *path*, so a destination with no
             * visible row (the hidden root of a single-root project, a folder scrolled off the
             * top) simply matches nothing, and its children still take the band.
             */
            const mark = flight?.outcome.mark ?? null
            return (
              <Row
                key={item.key}
                row={row}
                index={toReal(item.index)}
                statuses={statuses}
                iconTheme={iconTheme}
                top={item.start}
                height={item.size}
                selected={selection.paths.has(row.path)}
                current={row.path === selected}
                cut={isCutPending(clip, row.path)}
                dragging={inFlight !== null && inDrag(inFlight, row.path)}
                drop={mark === row.path && flight !== null ? flight.outcome.kind : undefined}
                dropBand={
                  mark !== null && flight !== null && flight.outcome.kind !== 'refuse'
                  && inDropBand(row.path, mark)
                    ? flight.outcome.kind
                    : undefined
                }
                renaming={row.path === renaming}
                project={project}
                onAct={apply}
                onPick={drag.onPointerDown}
                onRelease={releasePress}
                onEndRename={() => setRenaming(null)}
                onFail={fail('Rename')}
                match={speedSearch.spanFor(toReal(item.index))}
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
        * At the foot of the panel rather than in the row, because the row is 24px of a 252px
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
        * Nothing has been written when this is on screen; see `pasteConfirmModel.ts`.
        */}
      {pendingPaste !== null && (
        <PasteConfirm ask={pendingPaste.ask} onAnswer={answerPaste} onCancel={cancelPaste} />
      )}
      {/*
        * The trash confirmation, mounted here for the same reasons the paste one is: this panel
        * owns both gestures that raise it, and a dialog put here needs no `App.tsx` edit to be
        * reachable.
        *
        * The dialog is `chrome/ConfirmDestructive`, which the Git panel raises for its reverts.
        * It moved out of `GitPanel/` to be shared rather than being copied: this is a
        * *safeguard*, and two copies of a safeguard is how one of them quietly stops naming the
        * paths, or stops focusing Cancel, and nobody notices until it matters.
        */}
      {pendingDelete !== null && (
        <ConfirmDestructive
          state={pendingDelete}
          onCancel={() => closeDelete(null)}
          onConfirm={() => closeDelete(pendingDelete.run)}
        />
      )}
      {/*
        * What is being dragged and what will happen to it, under the pointer.
        *
        * Outside the scroller, like every other overlay here: it is `position: fixed`, the
        * scroller is `role="tree"` and its children have to be tree items, and its viewport is as
        * tall as the whole flattened repository.
        */}
      {drag.state !== null && <TreeDragGhost state={drag.state} />}
    </>
  )
}

/**
 * The drag ghost: the load, and the verdict.
 *
 * A drag with no indicator is a gesture people abandon halfway — or worse, finish over the wrong
 * folder — so this says both facts *in words* rather than relying on a cursor: what is being
 * carried (`4 items`, `src/`) and what the thing under the pointer will do with it (`Move 4 items
 * to “sidebar”`, `Already in “src”`, or the reason it is refused). The refusal is the one that
 * earns the component: dropping a folder into its own descendant is a gesture with no undo, and
 * the sentence is what stops it being attempted twice.
 *
 * `position: fixed` at the pointer, offset down-right so it never sits under the cursor's own
 * hotspot, and `pointer-events: none` so it is never what `elementFromPoint` finds — which would
 * make the drop target flicker between the row and the ghost as the pointer moved.
 */
function TreeDragGhost({ state }: { state: TreeDragState }) {
  return (
    <div
      className={styles.ghost}
      data-outcome={state.outcome.kind}
      data-audit="fileTreeDragGhost"
      style={{ left: `${state.x + 12}px`, top: `${state.y + 14}px` }}
      aria-hidden="true"
    >
      <span className={styles.ghostLoad}>{state.drag.label}</span>
      <span className={styles.ghostHint}>{state.outcome.hint}</span>
    </div>
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
      // Both classes: the draft is the one row the user is working in, so it takes the band
      // *and* the cursor's leading rule — which is what a selected row looked like before the
      // two were split apart for multi-selection.
      className={`${styles.row} ${styles.rowSelected} ${styles.rowCurrent}`}
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
  scope: readonly string[],
  roots: readonly string[],
  /** Where cide may write: the roots plus the scratch drawer. See the panel's `writable`. */
  writable: readonly string[],
  take: (mode: ClipMode, paths: readonly string[]) => void,
): { disabledReason: string } | { run: () => void } {
  if (scope.some((path) => isRootPath(path, roots))) {
    return { disabledReason: 'A project root is closed, not moved' }
  }
  // A dependency source, or a group header. **Cut** is refused and *Copy* is not, deliberately:
  // copying a crate's source into the project is a real thing to want, while moving one out of
  // `~/.cargo/registry` would break every other project on the machine that depends on it —
  // which is also why `check_within` refuses it in Rust. A **scratch** is cut like any other
  // file: it is in `writable`, so this passes and `fs_paste` moves it.
  const outside = mutationRefusal(scope, writable)
  if (outside !== null) return { disabledReason: outside }
  return { run: () => take('cut', scope) }
}

interface RowProps {
  row: TreeRow
  index: number
  statuses: TreeStatusMap['statuses']
  /** Threaded from the panel — see the subscription there. */
  iconTheme: IconTheme
  top: number
  height: number
  /** In the selection: drawn with the band, and acted on by Cut, Copy and Move to Trash. */
  selected: boolean
  /**
   * The cursor row — where the arrows move from.
   *
   * Drawn with the 2px leading rule on top of the band, exactly as `ChangesTree` draws its own,
   * so one selection means one thing in both sidebars. A single click makes a row both, which
   * is why this looked like one state for as long as a selection was one row.
   */
  current: boolean
  /** On the clipboard for a **cut**: drawn faded, because it is about to move. */
  cut: boolean
  /**
   * In flight: this row, or a folder above it, is being dragged. Drawn dimmed.
   *
   * The single most important signal in the gesture — without it a drag of four files looks
   * exactly like a drag of one, which is the whole subject of the request.
   */
  dragging: boolean
  /** `move` / `noop` / `refuse` when this row is the drop's marked row, else `undefined`. */
  drop: string | undefined
  /** The same, for a row *inside* the destination folder. See `treeDrag.inDropBand`. */
  dropBand: string | undefined
  renaming: boolean
  project: ProjectId | null
  onAct: (action: RowAction, row: TreeRow, index: number, mods: SelectMods) => void
  /** The press that may become a drag. See `useTreeDrag`. */
  onPick: (e: React.PointerEvent, row: TreeRow) => void
  /** The release, which finishes a press whose collapse was deferred. */
  onRelease: (path: string, index: number) => void
  onEndRename: () => void
  /** Where a rejected `fs_rename` goes. See `problem` in the panel. */
  onFail: (error: unknown) => void
  /**
   * Where the speed-search query sits inside this row's name, or `undefined`.
   *
   * A prop, threaded down from the panel, and not something the row works out for itself. The
   * panel's stated rule (see the `statuses` and `iconTheme` subscriptions above) is that per-row
   * store subscriptions are one zustand listener per visible row torn down on every scroll tick
   * — and re-deriving the *match* here would be worse still: it would be a second matching rule
   * in TypeScript beside the one in `cide_fs::speed`, which is the exact duplication this
   * feature was built to avoid.
   */
  match: { start: number; end: number } | undefined
}

function Row({
  row,
  index,
  statuses,
  iconTheme,
  top,
  height,
  selected,
  current,
  cut,
  dragging,
  drop,
  dropBand,
  renaming,
  project,
  onAct,
  match,
  onPick,
  onRelease,
  onEndRename,
  onFail,
}: RowProps) {
  const isDir = row.kind === 'dir'
  /**
   * What this row answers. `true` for `inProject` because nothing the *renderer* draws depends
   * on containment — the twisty, the icon and the double-click are the same for a dependency
   * source as for a project file, and the verbs that do care are the context menu's, which asks
   * `rowVerbs` again with the real answer.
   */
  const verbs = rowVerbs(row.kind, true)
  const synthetic = row.kind === 'group' || row.kind === 'note' || row.kind === 'pin'
  /** A synthetic row's glyph, or `null` for one that draws none. See `groupRows.groupIcon`. */
  const stem = groupIcon(row.kind, row.expanded, groupIdOf(row.path))
  // A synthetic row has no path on disk, so it has no git status either. Asking anyway would
  // resolve `cide://group/externalLibraries` against the status map's ancestor walk, which
  // terminates but is work per row per frame for an answer that is always `clean`. A pin's path
  // is the same kind of sentinel, and the file behind it lives outside every repository, so it
  // could never carry a tag even if the row were asked about the real path.
  const tone = synthetic ? 'clean' : statusAt(statuses, row.path)
  const letter = letterFor(tone, isDir)
  /*
   * `?? CLEAN` even though `TreeStatus` says the lookup is total. The compiler is checking a
   * generated type against a value that arrived over IPC, not the value itself: a Rust variant
   * added to the enum without regenerating — or an older backend against a newer webview —
   * lands here as a plain string, and reading `.nameClass` off `undefined` throws *inside a
   * render*, which unmounts the whole tree rather than mis-drawing one row.
   */
  const status = STATUS[tone] ?? CLEAN
  // `verbs.expandable`, not `isDir`: a group header folds, and it is drawn before it has any
  // children at all — `has_children` is hard-coded true on it in Rust, because a header with no
  // twisty cannot be opened and opening it is what starts the resolution.
  const hasTwisty = verbs.expandable && row.hasChildren
  const twisty = hasTwisty ? (row.expanded ? '▾' : '▸') : ''

  // Composed rather than a ternary chain: a row can be selected, be the cursor *and* be pending
  // a cut all at once, which is the ordinary case — Ctrl+X acts on the selection.
  const rowClass = [
    styles.row,
    // A group header is a heading and reads like one; a note is a sentence and reads dim and
    // italic, so neither can be mistaken for a file whose name happens to be a sentence.
    row.kind === 'group' ? styles.rowGroup : null,
    row.kind === 'note' ? styles.rowNote : null,
    selected ? styles.rowSelected : null,
    current ? styles.rowCurrent : null,
    cut ? styles.rowCut : null,
  ]
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
      data-row-expanded={verbs.expandable ? String(row.expanded) : undefined}
      /*
       * The three channels of the drag's feedback, and none of them is polish: a drop moves the
       * user's files and there is no undo anywhere in `cide_fs::ops`.
       *
       * `data-drag` dims what is in flight. `data-drop` says what the marked row will do —
       * accent ring for a move, a neutral ring for a drop that changes nothing, `--red` for a
       * refusal. `data-drop-band` tints the rows *inside* the destination, because the
       * destination is very often above the fold: drop onto a file thirty rows into an expanded
       * folder and the folder's own row is off the top of a 252px panel, so a ring on it alone
       * would highlight nothing at all.
       */
      {...(dragging ? { 'data-drag': '' } : {})}
      {...(drop === undefined ? {} : { 'data-drop': drop })}
      {...(dropBand === undefined ? {} : { 'data-drop-band': dropBand })}
      role="treeitem"
      aria-level={row.depth + 1}
      aria-selected={selected}
      {...(verbs.expandable ? { 'aria-expanded': row.expanded } : {})}
      style={{ height: `${height}px`, transform: `translateY(${top}px)` }}
      /* The full path, because the panel is 252px wide and long names are truncated. A
         synthetic row's `path` is a `cide://…` sentinel that names nothing, so it gets its own
         text instead — a tooltip reading `cide://group/externalLibraries` would look like a
         leaked internal, which is what it is. */
      title={synthetic ? row.name : row.path}
      /*
       * First, and deliberately separate from the click handling below: a press only becomes a
       * drag after 4px of movement, so every click rule keeps running exactly as it did and a
       * press that never moves costs nothing at all.
       *
       * Nothing calls `preventDefault` here, unlike `ChangesTree`. Rows in this tree are not
       * focusable — the scroller is the panel's single tab stop — so a press whose default is
       * prevented never focuses it, and every arrow key afterwards goes nowhere. See the header
       * of `useTreeDrag.ts`.
       */
      onPointerDown={(e) => {
        if (renaming) return
        onPick(e, row)
      }}
      /*
       * The release: it finishes a press whose collapse was deferred, and it is skipped when the
       * gesture became a drag. Without it, pressing on one of five selected rows and *not*
       * dragging would leave all five selected — the deferral would never resolve.
       */
      onMouseUp={(e) => {
        if (e.button !== 0 || renaming) return
        onRelease(row.path, index)
      }}
      onMouseDown={(e) => {
        // Middle and right buttons are not this gesture. Right-click still selects, from the
        // menu's own `items` callback, so the row the menu acts on is the row that lights up.
        if (e.button !== 0 || renaming) return
        onAct(
          fileTreeClick({
            gesture: gestureOf(e.detail),
            // "does a double-click fold this rather than open it", which is what `expandable`
            // means here — a group header answers yes and is not a directory.
            isDir: verbs.expandable,
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
          // Ctrl on Linux and Windows, ⌘ on macOS — folded here so nothing downstream has to
          // know which platform it is on. `apply` gates the fold and the open on these.
          { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey },
        )
      }}
    >
      {/* Indentation is a margin on the twisty rather than padding on the row, so the
          selection band still spans the panel at any depth. */}
      <span
        className={styles.twisty}
        style={{ marginLeft: `${row.depth * INDENT}px` }}
        /*
         * The twisty is a control, not a handle: one click on it folds the folder, so a hand that
         * shifts three pixels while clicking it must not pick the row up instead. The *click*
         * still reaches the row — only the pointer press is stopped — so `fileTreeClick`'s
         * `onTwisty` rule is untouched. `ChangesTree` stops the same event on its checkbox for
         * the same reason.
         */
        onPointerDown={(e) => e.stopPropagation()}
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
      {/*
       * A synthetic row's glyph is decided by `groupRows.groupIcon` rather than by the icon
       * theme, and a note gets none at all: the Material table associates *filenames* with
       * icons, so `iconFor` would hand a sentence to the extension lookup and draw the default
       * document — a row that reads as a file called "cargo is not on PATH".
       *
       * A **pin** goes down the same branch and needs no arm of its own: `groupIcon` answers
       * `markdown` for it, so it draws the glyph of the file it opens rather than a folder. The
       * `stem === null` test is what separates the three — only a note has no stem.
       */}
      {synthetic ? (
        stem === null ? (
          // A note. The empty span keeps the sentence aligned with the names above it, so a
          // failure row reads as part of the list rather than as something that slipped left.
          <span className={styles.noteGap} aria-hidden="true" />
        ) : (
          <FileIcon row={row} theme={iconTheme} stem={stem} />
        )
      ) : (
        <FileIcon row={row} theme={iconTheme} />
      )}
      {renaming && project !== null ? (
        <RenameInput project={project} row={row} onDone={onEndRename} onFail={onFail} />
      ) : (
        <span
          className={
            status.nameClass === undefined ? styles.name : `${styles.name} ${status.nameClass}`
          }
        >
          {/* One element, three text nodes on a matched row and one on every other. `.name` is
              the block container, so `text-overflow: ellipsis` is unaffected by inline children
              inside it — the same arrangement `SearchPanel` already draws its hit lines with. */}
          <SpeedName name={row.name} span={match} />
        </span>
      )}
      {/*
       * The dim second column: a dependency's version, the group's count, `not downloaded`.
       *
       * A sibling of the name rather than part of it, so the icon lookup, the rename box and
       * the "already exists" sibling check keep reading a *name*. It shrinks before the name
       * does — see `.detail` in the stylesheet — because in a 252px panel `serde` matters more
       * than `1.0.229`.
       */}
      {row.detail !== null && row.detail !== '' && (
        <span className={styles.detail}>{row.detail}</span>
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
