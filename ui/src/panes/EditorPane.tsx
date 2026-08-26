/**
 * A file tab's pane: the editor, wired to the disk and to the workspace.
 *
 * The split follows `DiffPane` / `ClaudeDiffPane`. `editor/EditorSurface.tsx` is the pure
 * component — text in, text out — and this is the piece that knows a file exists: it reads
 * one, writes one back, keeps the tab's dirty dot honest, and decides what to do when
 * something else changes the file underneath it.
 *
 * # Splitting
 *
 * There is nothing here about splits, and that is the point. A file tab carries a
 * `PaneTree` like every other tab, so "split editor right" is `pane.split` on the focused
 * pane — the same command, the same domain code and the same `SplitTree` that splits a
 * terminal. Each pane in the tree renders its own `EditorPane` over the same path, which is
 * two views of one file, which is what a split editor is. They are two *buffers*, not one
 * shared document, and that is a real limitation rather than a design: an edit in the left
 * half does not appear in the right until one of them saves and the other reloads. Sharing
 * would mean lifting the `EditorState` out of the component and into a per-path store, and
 * the milestone asks for the split path to exist, not for collaborative buffers.
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react'
import { EditorSurface, type SaveCause } from '@/editor/EditorSurface'
import {
  claude as claudeApi,
  diag,
  events,
  fileChanged,
  file as fileApi,
  toolWindow as toolWindowApi,
  history as historyApi,
  revisionFile,
} from '@/ipc/client'
import { registerBuffer, unregisterBuffer } from '@/editor/openBuffers'
import { registerPaneFocus } from './paneFocus'
import {
  fetchOutline,
  forgetOutline,
  scheduleOutline,
  subscribeOutlines,
  symbolsOf,
} from '@/editor/outlineStore'
import { closeDoc, openDoc, resetDoc, savedDoc, scheduleDoc } from '@/editor/docSync'
import {
  blameState,
  forgetBlame,
  isAnnotated,
  registerDirtyBuffer,
  revision as blameRevision,
  subscribe as subscribeBlame,
  toggleBlame,
} from '@/editor/blameStore'
import { collapseRuns } from '@/editor/blameModel'
import { requestLogReveal } from '@/gitlog/LogTab'
import type { FileView } from '@/editor/position'
import { levelFor, subscribeHighlightLevels } from '@/editor/highlightLevel'
import { basename, isMarkdownPath, languageIdFor } from '@/editor/languages'
import { noteActiveEditor } from '@/ext/editorBridge'
import { MarkdownFrame } from '@/editor/markdown/MarkdownFrame'
import type { MdView } from '@/editor/markdown/types'
import { useDiagnostics } from '@/sidebar/diagnosticsStore'
import { useWorkspace } from '@/store/workspace'
import { visible, type DiagnosticFilters } from '@/sidebar/ProblemsPanel/model'
import { AUTOSAVE_CEILING_MS, AUTOSAVE_IDLE_MS, shouldAutosave } from '@/editor/autosave'
import { describe, notify } from '@/chrome/notices'
import { contextMenuOpen } from '@/menus/menuState'
import { overlayOpen } from '@/overlays/store'
import type { FileStamp } from '@/ipc/generated'
import type { ProjectId } from '@/ipc/client'
import styles from './EditorPane.module.css'

export interface EditorPaneProps {
  /** Absolute path of the file this tab shows. */
  path: string
  /** The project root, so the status bar's trail is repo-relative. */
  root?: string | undefined
  /** Set when this pane is inside a project; without it the dirty dot cannot be reported. */
  project?: string | undefined
  /** The file tab that owns this pane, for the same reason. */
  tab?: string | undefined
  /**
   * Whether this pane's tab is the one in front. Passed straight through to `EditorSurface`,
   * which says what reads it and why an inactive tab is otherwise indistinguishable from the
   * active one. (M16)
   */
  onScreen?: boolean | undefined
  /**
   * The pane this editor fills, for the keyboard registry (`panes/paneFocus.ts`).
   *
   * Optional because a caller outside the pane tree — a fixture, the markdown frame's
   * standalone use — has no pane to name; without it the editor simply cannot be focused by
   * `pane.navigate.*`, which is the state every editor was in before the registry existed.
   */
  pane?: string | undefined
}

/**
 * The settings enum into the editor's vocabulary.
 *
 * Two spellings for one idea, and they are not merged because they belong to different layers:
 * `InspectionSettings` is a persisted wire type and `HighlightLevel` is what the CodeMirror
 * compartment and the context menu speak. `App.tsx` carries the same three-line mapping for the
 * same reason.
 *
 * `undefined` — settings not loaded yet — is `all`, which is the safe direction: showing
 * everything for one frame is recoverable, hiding a real error is not.
 */
function levelOf(
  level: 'none' | 'syntaxOnly' | 'allProblems' | undefined,
): 'none' | 'syntax' | 'all' {
  switch (level) {
    case 'none':
      return 'none'
    case 'syntaxOnly':
      return 'syntax'
    default:
      return 'all'
  }
}

/** What the pane is currently showing instead of, or as well as, a buffer. */
type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; text: string; writable: boolean; at: FileView | null }
  | { kind: 'failed'; why: string }

/**
 * How long a burst of scrolling accumulates before one IPC call.
 *
 * The second rung of the ladder in `crates/cide-app/src/positions_state.rs`. Longer than the
 * 80 ms `reportSelection` uses below, and the difference is what each notification is *for*:
 * that one drives a status line a human is watching in another process, so staleness is
 * visible; this one is a durable record nobody reads until the file is reopened, so the only
 * thing that matters is that the last gesture of a burst lands.
 */
const POSITION_DEBOUNCE_MS = 500

/**
 * How many editor panes are mounted over each path, so the last one out can drop that file's
 * annotation. (M18)
 *
 * A module-level map for the reason `editor/openBuffers.ts` and `layout/paneHosts.ts` are ones:
 * the fact is about a *file*, and no component owns a file. It has to be a count and not a flag
 * because splitting a File tab gives two `EditorPane`s over one path — `blameStore` is keyed per
 * path on purpose, so both show one column and one toggle moves both — and a plain
 * `forgetBlame` in the unmount of either would take the surviving pane's column away with it.
 *
 * The count is per *webview*: a detached window is a separate JavaScript realm with its own copy
 * of this module and its own store, which is the same reason `paneHosts.ts` gives for its map.
 */
const mountedPanes = new Map<string, number>()

export function EditorPane({
  path,
  root,
  project,
  tab,
  onScreen,
  pane,
}: EditorPaneProps): ReactNode {
  /*
   * This file's structure, for the status bar's symbol trail.
   *
   * `useSyncExternalStore` because `outlineStore` is module-level — the same shape `GitPanel`
   * uses for its partial-selection store, and for the same reason: three unrelated surfaces read
   * it, one of them (`keys/dispatch.ts`) from outside React entirely.
   *
   * `symbolsOf` must return a **referentially stable** value — `useSyncExternalStore` compares
   * with `Object.is`, and a fresh `[]` per call is an infinite render loop that unmounts the
   * whole React tree. It returns a shared frozen constant for the empty case; see `NONE` in
   * `outlineStore.ts`. This comment previously *claimed* that property without the code
   * having it, which is how it shipped.
   */
  const outline = useSyncExternalStore(
    subscribeOutlines,
    () => symbolsOf(path),
    () => symbolsOf(path),
  )

  /*
   * This file's diagnostics, and how much of them to draw.
   *
   * The snapshot is read raw here and filtered against *this editor's* level — which is the one
   * axis that is genuinely per-buffer. The severity and source axes were already applied by
   * `App.tsx` before the snapshot reached the panel, and re-applying them here would be a second
   * place for them to disagree; `visible` is called with everything-on for those two so only the
   * level does any work.
   */
  const snapshot = useDiagnostics((s) => s.snapshot)
  /*
   * The workspace default this buffer falls back to when it has no override of its own.
   *
   * Read from settings rather than hardcoded. It was `levelFor(path, 'all')` in both readers,
   * which made `Settings ▸ Inspections ▸ default highlighting level` inert for every editor —
   * the one surface it is defined for — because a file with no per-file override always resolved
   * to `all` regardless of what the setting said.
   */
  const defaultLevel = useWorkspace((s) =>
    levelOf(s.boot?.workspace.settings.inspections.defaultHighlightLevel),
  )
  const level = useSyncExternalStore(
    subscribeHighlightLevels,
    () => levelFor(path, defaultLevel),
    () => levelFor(path, defaultLevel),
  )
  const diagnostics = useMemo(() => {
    const items = snapshot.kind === 'unavailable' ? [] : (snapshot.items ?? [])
    const filters: DiagnosticFilters = {
      severities: { error: true, warning: true, info: true, hint: true },
      sources: {},
      level,
    }
    return items.filter((item) => item.absPath === path && visible(item, filters))
  }, [snapshot, path, level])
  /**
   * `settings.editor.autosave`, from the mirror. Defaults to **on** before bootstrap, which is
   * the value `EditorSettings::default()` carries and the value `persist::v2_to_v3` writes into
   * every upgraded workspace — so the three answers to "is autosave on" cannot disagree.
   */
  const autosaveOn = useWorkspace((s) => s.boot?.workspace.settings.editor.autosave ?? true)
  /**
   * `settings.editor.completion` and `.completionOnTyping`, from the mirror. (M25)
   *
   * Both default to **on** before bootstrap, matching `EditorSettings::default()`.
   *
   * A `useMemo` over two scalars rather than one selector returning an object, and that is not a
   * style choice: `check:selectors` exists because a `useWorkspace` selector that *builds* a fresh
   * object re-renders for ever and ends at *Maximum update depth exceeded*, which unmounts the
   * whole root. Two scalar reads and a memo is the shape that cannot do that.
   */
  const completionOn = useWorkspace((s) => s.boot?.workspace.settings.editor.completion ?? true)
  const completionOnTyping = useWorkspace(
    (s) => s.boot?.workspace.settings.editor.completionOnTyping ?? true,
  )
  const completion = useMemo(
    () => ({ enabled: completionOn, onTyping: completionOnTyping }),
    [completionOn, completionOnTyping],
  )
  /**
   * Is a Claude Code diff of **this file** on screen, with the agent blocked on it?
   *
   * `crates/cide-app/src/ide.rs`: *"`openDiff` blocks an agent turn. The CLI sends it and waits;
   * nothing else happens in that turn."* The collision is the ordinary gesture, not an edge
   * case: the user has unsaved edits in `foo.rs`, Claude proposes a diff of `foo.rs`, and the
   * user clicks the diff tab to look at it — which is a tab switch, which is a blur, which is a
   * save of `foo.rs` underneath a proposal computed against the old bytes.
   *
   * Selected as a **boolean**, deliberately: the selector runs on every workspace snapshot, and
   * returning the matching tabs would hand `useSyncExternalStore` a fresh array every time and
   * re-render this pane on every keystroke in every window.
   *
   * `claudeMcp` only. A diff the *user* opened blocks nobody and is not a reason to stop saving
   * the file it came from.
   */
  const agentDiff = useWorkspace((s) =>
    (project === undefined ? [] : (s.boot?.workspace.projects[project]?.tabs ?? [])).some(
      (t) =>
        t.kind.kind === 'diff'
        && t.kind.spec.origin.kind === 'claudeMcp'
        && (t.kind.spec.newPath === path || t.kind.spec.oldPath === path),
    ),
  )
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [reloadKey, setReloadKey] = useState(0)
  /** Set when the file changed on disk while the buffer had unsaved edits. */
  const [conflict, setConflict] = useState(false)
  const dirtyRef = useRef(false)
  /**
   * What the file was when this buffer last agreed with the disk.
   *
   * Set by every read and moved by every successful write, so an autosave compares against the
   * bytes it last produced rather than against the file as it stood when the tab opened. `null`
   * means "no precondition available" — a filesystem that would not answer — which Rust reads as
   * "write anyway"; see `cide_ipc::FileStamp`.
   *
   * A ref, not state: nothing renders differently for it, and a re-render per save would be a
   * re-render per minute of typing.
   */
  const stampRef = useRef<FileStamp | null>(null)
  /**
   * Whether the file on disk carries write permission, as `file_read` answered.
   *
   * A ref because `load` is state that the autosave gate would otherwise have to be rebuilt for
   * on every read; the value only ever arrives with a new file, exactly as `EditorSurface`'s own
   * `readOnly` prop does.
   */
  const writableRef = useRef(true)
  /**
   * Reads the buffer as it is *now*. Handed out by `EditorSurface` on every change. (M18)
   *
   * A function and not a string, so nothing holds a copy of a document still being typed into —
   * the contract `blameStore.registerDirtyBuffer` states. `null` until the first edit, which is
   * exactly the window in which the tab is clean and the store wants nothing.
   */
  const readTextRef = useRef<(() => string) | null>(null)
  /**
   * The path this pane last rendered, so a *rename* can be told apart from a first mount. (M25)
   *
   * The two arrive identically — `path` is a prop — and they want opposite things: a first mount
   * reads the file, a rename must keep the buffer that is already on screen. See the block below
   * `lastViewRef`, which is the only writer.
   */
  const openedPath = useRef(path)
  /** Whether the load in flight is a retarget whose buffer must not be replaced. See below. */
  const carriedRef = useRef(false)

  /*
   * The markdown preview's three wires, and none of them is a prop that changes per keystroke.
   *
   * `docListeners` is notified from `onDocChanged` below; `readText` reads the buffer when asked,
   * falling back to the text on disk before the editor has reported a change; `scrollTo` is the
   * handle `EditorSurface` hands out when its view is built. All three are refs because the
   * alternative is a `setState` on the typing path, which would re-render this component — and
   * therefore every editor in the app — sixty times a second.
   */
  const docListeners = useRef(new Set<() => void>())
  const diskTextRef = useRef('')
  const scrollToRef = useRef<((line: number) => void) | null>(null)
  const followRef = useRef<((topLine: number) => void) | null>(null)
  /*
   * The last view the editor reported, kept after `sendPosition` has cleared the pending one.
   * Switching layout has to write a *whole* `ViewPosition`, and writing one built from
   * defaults would reset the file's remembered scroll to line 1 — the exact data loss the
   * position store exists to prevent, arriving through the feature that reads it.
   */
  const lastViewRef = useRef<FileView | null>(null)

  /*
   * The file was renamed under this tab. Keep the buffer and keep the viewport. (M25)
   *
   * `path` changing on a mounted `File` tab means exactly one thing: the file moved on disk and
   * `cide_core::workspace::retarget_paths` moved the tab with it. Two things have to survive that,
   * and neither survives on its own:
   *
   * * **Unsaved edits.** A rename copies nothing, so the bytes on disk are the buffer the user
   *   has already edited past. Letting the load below bring them back would replace their work at
   *   a moment when nothing on screen said a buffer was about to be thrown away.
   * * **Where they were reading.** `EditorSurface` restores from an observation stamped with the
   *   document's *identity*, which is the path — so after a rename it falls back to the `at` prop,
   *   which is where the file was when the tab opened. Renaming a file you are four hundred lines
   *   into and being thrown back to line 1 is the visible half of the same bug.
   *
   * # Why this is in the render pass and not in an effect
   *
   * React runs a child's effects **before** its parent's. `EditorSurface`'s build effect is keyed
   * on `[path, reloadKey]`, so on the very commit that carries the new path it tears the
   * `EditorView` down and rebuilds it from the `doc` prop — before any effect here could have
   * captured what was in it. By then `readTextRef` reads the *rebuilt* view and answers with the
   * text the carry exists to replace. Adjusting state during render is React's documented way to
   * respond to a changed prop, and it is what makes the surface's one rebuild the correct one:
   * React discards this render, re-runs this component, and the child sees the new path and the
   * carried buffer together.
   *
   * Guarded on the ref rather than on a comparison with `load`, so it runs exactly once per
   * rename — a `setLoad` during render that could re-trigger itself is an infinite render loop,
   * which in this application means the whole root unmounting.
   */
  if (openedPath.current !== path) {
    openedPath.current = path
    // The live buffer, and only when it holds something the disk does not: a clean tab has
    // nothing to carry and is better served by the ordinary read below.
    const live = dirtyRef.current ? (readTextRef.current?.() ?? null) : null
    const seen = lastViewRef.current
    carriedRef.current = live !== null
    setLoad((prev) =>
      prev.kind !== 'ready'
        ? prev
        : {
            ...prev,
            ...(live === null ? {} : { text: live }),
            // Stamped with the new path because that is the name the surface will compare
            // against — `viewTracker` keys its observation on the same value.
            ...(seen === null ? {} : { at: { ...seen, path } }),
          },
    )
  }

  const subscribeText = useCallback((listener: () => void) => {
    docListeners.current.add(listener)
    return () => {
      docListeners.current.delete(listener)
    }
  }, [])
  const readText = useCallback(() => readTextRef.current?.() ?? diskTextRef.current, [])
  const scrollEditorTo = useCallback((line: number) => {
    scrollToRef.current?.(line)
  }, [])
  /**
   * Choose a markdown layout, and record it now rather than on the scroll debounce.
   *
   * A click is a discrete intention; `worthNoting` and the 500 ms timer exist to keep a 60 Hz
   * scroll off the IPC thread and have nothing to say about this one. Noting immediately is also
   * what makes closing the tab straight after switching keep the switch.
   */
  const chooseView = useCallback((next: MdView) => {
    setMdView(next)
    mdViewRef.current = next
    const at = pendingPosition.current ?? lastViewRef.current
    if (at === null) return
    void fileApi.notePosition({ ...at, folds: [...at.folds], markdownView: next }).catch(() => {})
  }, [])

  const onScrollHandle = useCallback((scrollTo: ((line: number) => void) | null) => {
    scrollToRef.current = scrollTo
  }, [])
  const onSyncHandle = useCallback((follow: ((topLine: number) => void) | null) => {
    followRef.current = follow
  }, [])

  /*
   * The keyboard half of `pane.navigate.*`. (M27)
   *
   * The surface hands `view.focus` in through `onFocusHandle` and hands `null` back when the
   * view is torn down or rebuilt, so the ref never holds a destroyed view; the registry entry
   * closes over the ref rather than the closure, so a `reloadKey` rebuild does not need to
   * re-register. While the markdown preview has replaced the surface entirely the ref is
   * `null` and the entry answers by doing nothing — `focusPaneDom` still reports `true`, which
   * is honest enough: the pane took the gesture, and the preview has no caret to give.
   */
  const focusViewRef = useRef<(() => void) | null>(null)
  const onFocusHandle = useCallback((focus: (() => void) | null) => {
    focusViewRef.current = focus
  }, [])
  useEffect(() => {
    if (pane === undefined) return undefined
    return registerPaneFocus(pane, () => focusViewRef.current?.())
  }, [pane])

  /*
   * Who wrote each line — subscribe here, and read the store directly below. (M18)
   *
   * `useSyncExternalStore` over `blameStore`, the same shape `outlineStore` is read with a few
   * lines up and for the same reason: the writers are outside React — the command palette, the key
   * gate and the editor's own context menu all toggle it, and none of them has a component to call
   * `setState` on.
   *
   * The return value — the store's revision counter — is deliberately not bound. It exists to give
   * `useSyncExternalStore` a stable snapshot to compare with `Object.is`, which is the trap
   * `symbolsOf`'s comment above records: returning a `BlameState` object would be a fresh identity
   * per call and an infinite render loop that unmounts the whole tree. What this line buys is the
   * re-render; the answers come from `isAnnotated`/`blameState`, which are the store's own
   * definitions and the only place they should live.
   */
  useSyncExternalStore(subscribeBlame, blameRevision, blameRevision)
  const blameOn = project !== undefined && isAnnotated(project as ProjectId, path)
  const blameNow = project === undefined ? null : blameState(project as ProjectId, path)
  /**
   * The answer itself, or `null`. **Identity-stable across unrelated store writes**, which is why
   * it is pulled out rather than folded into the memo below: the counter bumps whenever *any*
   * buffer's annotation moves, and keying the collapse on it would rebuild this file's whole
   * marker array — and, one effect later, its whole `RangeSet` — because somebody toggled a
   * column in another tab.
   */
  const blameFile = blameNow !== null && blameNow.kind === 'ready' ? blameNow.blame : null
  /**
   * One marker per line.
   *
   * `Date.now()` is sampled **here**, once per answer, and handed to `collapseRuns` — never read
   * inside it. That is what makes the model checkable, and it is also what keeps one paint's
   * buckets consistent: a clock read per line could straddle a bucket edge and tint two lines of
   * the same run differently. Seconds, because that is the unit the whole blame wire is in.
   */
  const blame = useMemo(
    () => (blameFile === null ? null : collapseRuns(blameFile, Math.floor(Date.now() / 1000))),
    [blameFile],
  )

  /**
   * Say why the column did not appear.
   *
   * The toggle is a user gesture — a palette row, a menu item — and a gesture that silently does
   * nothing is the failure this project has paid for repeatedly. The store's `reason` is already a
   * sentence written for a person ("This file is not inside any repository in this project."), so
   * it is shown as it stands.
   *
   * Keyed on the reason itself, so it fires once per *transition* rather than once per render.
   * Two panes over one file both fire, and `notices.admit` collapses them: it drops a notice whose
   * text is already on screen, so no cross-pane dedupe is needed here.
   *
   * `info`, not `error`: "this file is not in a repository" is an answer about the project, not a
   * failure of anything.
   */
  const blameFailure = blameNow !== null && blameNow.kind === 'failed' ? blameNow.reason : null
  useEffect(() => {
    if (blameFailure !== null) notify(blameFailure, { kind: 'warn' })
  }, [blameFailure])

  /*
   * The last pane over this file drops its annotation; the others leave it alone. See `mountedPanes`.
   *
   * Keyed on the path and not on the tab, because the store is: the same file open in two panes
   * shows one column, so "is anybody still looking at this file" is the only question worth
   * asking here.
   */
  useEffect(() => {
    mountedPanes.set(path, (mountedPanes.get(path) ?? 0) + 1)
    return () => {
      const left = (mountedPanes.get(path) ?? 1) - 1
      if (left > 0) {
        mountedPanes.set(path, left)
        return
      }
      mountedPanes.delete(path)
      registerDirtyBuffer(path, null)
      if (project !== undefined) forgetBlame(project as ProjectId, path)
    }
  }, [path, project])

  /**
   * A reload replaces the buffer, so the column is re-fetched rather than merely cleared.
   *
   * `reloadKey` bumps when the agent edits the open file or the user picks *Reload from disk*, and
   * the bytes the blame was computed against are gone. Clearing alone would be a column that
   * silently disappears every time Claude touches the file, which reads as the feature breaking;
   * `forgetBlame` followed by `toggleBlame` is the re-fetch, and it keeps the buffer's own text out
   * of it — the tab is clean immediately after a reload, so Rust reads the file it can see.
   *
   * The first run is skipped: the effect fires on mount, where there is nothing to re-fetch and
   * `toggleBlame` would turn a column *on* that nobody asked for.
   */
  const reloadSeen = useRef(reloadKey)
  useEffect(() => {
    if (reloadKey === reloadSeen.current) return
    reloadSeen.current = reloadKey
    if (project === undefined) return
    const id = project as ProjectId
    if (!isAnnotated(id, path)) return
    forgetBlame(id, path)
    toggleBlame(id, path)
  }, [reloadKey, project, path])

  /**
   * A gutter cell, or the card's *Show in log*, was clicked.
   *
   * Opens the tool window on its Log tab **and selects the commit**, through the reveal seam
   * `gitlog/LogTab.tsx` exports. Until that seam existed this ended in a notice naming the oid,
   * because the log kept its selection in component state with no way in — a panel that opens at
   * HEAD after a click on a line from 2019, saying nothing, is a gesture that looks broken, and
   * naming the commit at least made it one the user could finish by hand. That notice is gone:
   * the log now answers all three outcomes itself, including the common one where the commit is
   * older than the loaded page.
   *
   * # Three round trips, in this order, and the order is the point
   *
   * `git_locate` first, because the request carries a `RepoId` and this pane holds an absolute
   * path and nothing else. Resolving it in Rust is the rule `git_locate` exists for: `canonical`
   * resolves symlinks and the webview cannot, so a prefix test here would be correct until the
   * first symlinked checkout. It is the same call `onAnnotateParent` below makes, on a gesture
   * nobody makes twice a second.
   *
   * Then the tool window is opened and the Log tab activated, and only **then** is the request
   * parked. A History tab may be the mounted one at this instant, and it must not consume a
   * request meant for the Log tab; requesting after `activate` keeps the window between the two
   * as small as it can be, and `LogTab` refuses to answer on a History tab in any case.
   *
   * A file outside every repository resolves to `null` and says so, rather than opening an empty
   * log: there is no commit to reveal, and the blame column that produced this click cannot exist
   * for such a file anyway.
   *
   * Fire-and-forget with a reported failure, not a swallowed one: this is a click.
   */
  const onShowCommit = useCallback(
    (oid: string) => {
      if (project === undefined) return
      const id = project as ProjectId
      void historyApi
        .locate(id, path)
        .then((found) => {
          if (found === null) {
            notify('This file is not inside any repository in this project.', { kind: 'warn' })
            return undefined
          }
          return toolWindowApi
            .setLayout(id, { open: true })
            .then(() => toolWindowApi.activate(id, null))
            .then(() => {
              // The file too, so the log's details pane lands on the line's own file rather
              // than on a commit and forty paths. `found.path` is repo-relative, which is what
              // `CommitFile.path` is — see `RevealRequest.file`.
              requestLogReveal(id, found.repo, oid, found.path)
            })
        })
        .catch((error: unknown) => {
          notify(describe(error), { kind: 'error' })
        })
    },
    [project, path],
  )

  /**
   * *Annotate previous revision* — the hop the blame popup's second button makes. (M18)
   *
   * It opens a **read-only revision tab** for the file as the parent commit left it, rather than
   * re-annotating this buffer at that revision. That distinction is the whole design and not a
   * shortcut: `cide_git::blame::blame_with` documents that `newest` blames the file **as it was
   * at that revision**, so its runs cover a different line count and would sit one line further
   * out of step with the text on screen for every line the two versions differ by. Nothing is
   * worse in a blame gutter than a column that is confidently misaligned.
   *
   * # Why this is not the revision *diff* it used to open
   *
   * It opened `tab_open_revision_diff` — `parent.rev` against its first parent — for the whole
   * time `TabKind::Revision` had no pane to render it in, and that was the wrong tab for the
   * gesture rather than a lesser version of the right one. The button says *annotate the
   * previous revision*: the user is asking to **read that file**, with the same gutter they were
   * reading this one with, so they can hop again from a line that is still attributed to a
   * reformat. A diff answers a different question — *what did this one commit change* — and it
   * cannot be hopped from at all, because a patch has no lines to blame. Two concrete losses,
   * both reachable in one click:
   *
   * * A commit that did not touch the path answers `NoSuchChange`, so a hop landing on a merge
   *   or a parent chosen by rename-following showed a refusal where a file was asked for. The
   *   blob read has no such failure mode — the file is in that tree or it is not.
   * * `RevSide::FirstParent` is not the parent `git_blame_parent` just resolved. It is *the
   *   commit's own first parent*, so the diff drawn was `parent.rev` against something the walk
   *   never chose, which for a merge is the wrong side entirely.
   *
   * The revision diff is still exactly right where it is reached from: the tool window's
   * changed-file list, where a commit is the subject and the file is the detail. Here the file
   * is the subject.
   *
   * `git_blame_parent` answers with the parent that *touched the path*, following a rename at
   * that boundary — not `rev^`, which is the first parent and is the wrong commit for a merge
   * that took the file from its second, and which names the *new* path across a rename. `null`
   * is the end of the walk (a root commit, or the commit that introduced the file), and it is
   * reported rather than swallowed so the button can say so.
   *
   * # The chain this hop starts
   *
   * `[oid]` and not `[]`, which is the trail growing by the commit the hop was made *from*. The
   * user is looking at the working tree and clicked a card about `oid`; the version they are
   * being shown is the one *before* it, so `oid` is a real waypoint between the two and Back
   * from the new tab lands on "the revision this line was actually written in" — which is the
   * next thing anyone asks. An empty chain would be defensible on the grounds that the user
   * never had a tab open on `oid`, and it loses that hop for nothing: the crumb strip is a route
   * through history, not a browser history of tabs. Rust normalises what is passed
   * (`cide_core::workspace::revision_chain`), so a hop that circles back cannot grow it.
   */
  const onAnnotateParent = useMemo(() => {
    if (project === undefined) return null
    const id = project as ProjectId
    return (oid: string) => {
      // `locate` rather than a `repo` held in state: this pane has an absolute path and nothing
      // else, and resolving it in Rust is the rule `git_locate` exists for — `canonical` resolves
      // symlinks and the webview cannot, so a prefix test here is correct until the first
      // symlinked checkout. One round trip on a gesture nobody makes twice a second.
      void historyApi
        .locate(id, path)
        .then((found) => {
          if (found === null) return undefined
          return historyApi.blameParent(id, found.repo, found.path, oid).then((parent) => {
            if (parent === null) {
              notify('This is where the file was introduced — there is no earlier revision.', {
                kind: 'warn',
              })
              return undefined
            }
            // `parent.path` and not `found.path`: the parent may spell the file differently, and
            // the hop is most often taken across exactly the rename that makes the two differ.
            return revisionFile
              .openTab(id, found.repo, parent.path, parent.rev, [oid])
              .then(() => {})
          })
        })
        .catch((error: unknown) => {
          notify(describe(error), { kind: 'error' })
        })
    }
  }, [project, path])

  /**
   * Tell the workspace whether this file has unsaved edits.
   *
   * Deduplicated here as well as in the surface because there are two callers: the editor's
   * own transitions, and a reload from disk. A reload has to report too — it replaces the
   * buffer with the file's contents, so the tab is clean afterwards even though nothing in
   * the editor transitioned. Without this the dot stayed lit for the life of the tab after
   * a conflict was resolved with "Reload from disk", which is the one moment the dot is
   * supposed to go out.
   */
  const reportDirty = useCallback(
    (dirty: boolean) => {
      if (dirtyRef.current === dirty) return
      dirtyRef.current = dirty
      /*
       * Offer — or withdraw — this buffer's text for the next blame. (M18)
       *
       * Registered **only while dirty**, which is the store's rule and is what keeps a copy of
       * every open file off the IPC wire on the common path: a clean tab registers nothing and
       * Rust reads the file it can already see. A dirty one has to hand its bytes over, because
       * omitting them attributes every line after an unsaved insertion to the wrong commit — a
       * per-line falsehood rather than a stale view, which is the distinction
       * `cmd::git::git_blame` draws.
       *
       * Beside the dirty *dot* rather than in an effect of its own, because the two are the same
       * transition and a second observer of it is a second thing that can be one keystroke behind.
       * `readTextRef` is already set by the time this runs: `EditorSurface`'s update listener
       * calls `onDocChanged` before `setDirty`, so the change that makes a tab dirty has handed
       * over its reader first.
       */
      registerDirtyBuffer(path, dirty ? () => readTextRef.current?.() ?? '' : null)
      if (project === undefined || tab === undefined) return
      // Fire-and-forget: the dot is a hint, and a failed report must not interrupt typing.
      // The authoritative answer to "are there unsaved changes" is the buffer itself, which
      // is why nothing here waits for the round trip.
      void fileApi.setDirty(project, tab, dirty).catch(() => {})
    },
    [path, project, tab],
  )

  /**
   * Tell the project's Claude sessions where the caret is.
   *
   * Debounced, and that is the whole reason this is not a straight call: a selection drag
   * fires the editor's update listener once per animation frame, and one IPC round trip per
   * frame during a drag is the kind of cost that only shows up on someone else's machine.
   * 80 ms is below the threshold where a status readout feels stale and well above a frame.
   *
   * Fire-and-forget: the notification is advisory — Claude Code shows it in its status line
   * — and a failed send must never interrupt typing. A project with no IDE server, or no
   * connected `claude`, drops it on the Rust side with a debug log.
   */
  const selectionTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const reportSelection = useCallback(
    (sel: { text: string; startLine: number; endLine: number }) => {
      if (project === undefined) return
      if (selectionTimer.current !== undefined) clearTimeout(selectionTimer.current)
      selectionTimer.current = setTimeout(() => {
        void claudeApi.selectionChanged(project, path, sel.text, sel.startLine, sel.endLine)
      }, 80)
    },
    [project, path],
  )

  // A pending report for a pane that has gone would fire against an unmounted editor.
  useEffect(
    () => () => {
      if (selectionTimer.current !== undefined) clearTimeout(selectionTimer.current)
    },
    [],
  )

  /**
   * Remember where the user is in this file.
   *
   * Debounced here rather than in the surface, for the reason `reportSelection` above is: the
   * surface is documented as pure — "no IPC, no store, no knowledge of tabs" — and the moment it
   * knows what a debounce is for, it knows about the wire.
   *
   * The trailing edge matters and the leading edge does not. Nobody needs the *first* frame of a
   * scroll recorded; what has to survive is where the gesture ended, which is why this resets
   * the timer on every report rather than rate-limiting.
   */
  /**
   * Which markdown layout this file is in. (M20)
   *
   * Held here rather than inside `MarkdownFrame` because it is **persisted**, and the record it
   * rides is `ViewPosition` — the same per-file store as the scroll position and the folds, which
   * this component already reads at mount and writes on a debounce. A layout kept inside the
   * frame would be a second lifetime for one of that record's fields.
   *
   * `text` for every file that is not markdown, where it is never read. See Rust's `MarkdownView`
   * for why one field on one record beats a second store keyed on "only the markdown ones".
   */
  const [mdView, setMdView] = useState<MdView>('text')
  /*
   * Read on every note, so an ordinary scroll does not overwrite the layout with a default.
   * A ref and not the state value: `sendPosition` is a `useCallback` with an empty dependency
   * list — it is held by a timer, and re-creating it on every layout change would restart the
   * debounce — so it must read the mode when it fires rather than when it was made.
   */
  const mdViewRef = useRef<MdView>('text')
  mdViewRef.current = mdView

  const positionTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const pendingPosition = useRef<FileView | null>(null)
  const sendPosition = useCallback(() => {
    const at = pendingPosition.current
    pendingPosition.current = null
    if (positionTimer.current !== undefined) {
      clearTimeout(positionTimer.current)
      positionTimer.current = undefined
    }
    if (at === null) return
    // Fire-and-forget: a lost position costs a scroll offset, and a rejected promise on the
    // scroll path would reach `Failures` and put a notice on screen for something the user did
    // not ask for and cannot act on.
    //
    // `folds` is copied out of the readonly array the editor reported: the DTO's field is a
    // `Vec<u32>` and therefore a mutable `number[]` on this side, and handing the same array to
    // a caller that could sort it is how a "pure observation" quietly stops being one.
    void fileApi
      .notePosition({ ...at, folds: [...at.folds], markdownView: mdViewRef.current })
      .catch(() => {})
  }, [])
  const reportPosition = useCallback(
    (at: FileView) => {
      /*
       * Split-mode scroll sync, on the signal that already exists. `viewTracker.ts` publishes at
       * most once an animation frame, which is exactly the cadence a follower wants, so the
       * preview needs no listener of its own on the buffer's scroller — and the *note* keeps its
       * own 500 ms debounce below, untouched.
       */
      followRef.current?.(at.topLine)
      lastViewRef.current = at
      pendingPosition.current = at
      if (positionTimer.current !== undefined) clearTimeout(positionTimer.current)
      positionTimer.current = setTimeout(sendPosition, POSITION_DEBOUNCE_MS)
    },
    [sendPosition],
  )

  /*
   * On the way out this **flushes**, where the selection cleanup above **cancels** — and the
   * asymmetry is the whole point rather than an oversight.
   *
   * A pending `selection_changed` for a pane that has gone is noise: it describes a caret that
   * is no longer on screen, in a notification whose only consumer is a status line. A pending
   * *position* for a pane that has gone is the single most valuable one there is — closing the
   * tab, switching project and detaching a pane all arrive here as an unmount, and every one of
   * them is a moment the user expects to come back to. Cancelling would mean the last half
   * second of every reading session was the half that got lost.
   *
   * A window closed by the compositor does not run React cleanup at all. The 500 ms debounce
   * will normally have fired long before; what covers the rest is the store's own flush in
   * `lifecycle::shutdown`, which writes unconditionally.
   */
  useEffect(() => () => sendPosition(), [sendPosition])

  /**
   * Publish this buffer's save function so `CloseConfirm` can offer *Save and close*.
   *
   * Keyed on the tab, not the pane: the dirty flag the confirmation reads is per tab, and a
   * File tab has exactly one editor (`PaneBody` dispatches on the pane kind for this reason),
   * so the two agree by construction.
   */
  const registerSaveHandle = useCallback(
    (save: (() => Promise<void>) | null) => {
      if (tab === undefined) return
      if (save) registerBuffer(tab, save)
      else unregisterBuffer(tab)
    },
    [tab],
  )

  const read = useCallback(
    (bump: boolean) => {
      let cancelled = false
      /*
       * Whether the render pass above already put this buffer on screen — a rename. (M25)
       *
       * Read once and cleared, because it describes *this* load: the retarget block runs on the
       * commit that changes `path`, this effect runs on the same one, and every later load — a
       * reload from disk, a conflict resolved — must go back to trusting the file.
       */
      const carried = carriedRef.current
      carriedRef.current = false
      /*
       * The remembered position is fetched **beside** the text, in one `Promise.all`, and the
       * surface is not rendered until both have landed.
       *
       * Sequencing it after the read would put the editor on screen at line 1 and then jump it,
       * which is worse than the bug: a visible jump reads as the app losing your place and
       * finding it again. `catch(() => null)` because there is no position for most files and a
       * store that could not answer must not stop the file from opening.
       *
       * `bump` — a reload from disk — deliberately re-fetches it and deliberately does *not*
       * use it: `EditorSurface` prefers its own live observation over this prop, which is the
       * only value that is current after the user has been scrolling. See `observedRef` there.
       */
      void Promise.all([fileApi.read(path), fileApi.position(path).catch(() => null)])
        .then(([doc, at]) => {
          if (cancelled) return
          /*
           * A carried buffer takes the refs and nothing else.
           *
           * `stamp` above all: it is what the *next* autosave compares against, and the file it
           * has to describe is the one under the new name. Leaving the old file's token there
           * would make every autosave after a rename refuse with a conflict the user cannot
           * explain — a feature that stops working the first time somebody uses another one.
           *
           * `setLoad` is deliberately not called: the text on screen is the user's, the viewport
           * was stamped a moment ago, and replacing either with what is on disk is the whole of
           * what this branch exists to prevent.
           */
          if (!carried) {
            setLoad({
              kind: 'ready',
              text: doc.text,
              writable: doc.writable,
              at:
                at === null
                  ? null
                  : {
                      path: at.path,
                      line: at.line,
                      column: at.column,
                      topLine: at.topLine,
                      folds: at.folds,
                    },
            })
            /*
             * The remembered layout, applied on the same frame as the remembered scroll. A tick
             * later would open every previewed file as a buffer and then swap it, which reads as
             * the setting not having been saved.
             */
            setMdView(at?.markdownView ?? 'text')
          }
          diskTextRef.current = doc.text
          stampRef.current = doc.stamp
          writableRef.current = doc.writable
          setConflict(false)
          if (!carried) reportDirty(false)
          else {
            /*
             * Still dirty, and the *new* path has to be the one offering the bytes.
             *
             * `reportDirty` is the single writer of this pairing and it deduplicates against
             * `dirtyRef` — the flag has not transitioned, so calling it here would do nothing at
             * all. The registration is keyed on the path and the unmount above already withdrew
             * the old one, so without this line a blame over the renamed file would read the disk
             * and attribute every line after an unsaved insertion to the wrong commit.
             */
            registerDirtyBuffer(path, () => readTextRef.current?.() ?? '')
          }
          if (bump) setReloadKey((n) => n + 1)
        })
        .catch((error: unknown) => {
          if (cancelled) return
          setLoad({ kind: 'failed', why: String(error) })
        })
      return () => {
        cancelled = true
      }
    },
    [path, reportDirty],
  )

  useEffect(() => read(false), [read])

  /**
   * Write the buffer back.
   *
   * The promise is returned rather than swallowed, because the surface uses it to decide
   * when the buffer is clean — a failed write must leave the tab dirty. The failure is
   * logged rather than raised into a dialog: this pane has no modal surface, and the tab
   * staying dirty is itself the signal that nothing was written.
   */
  const onSave = useCallback(
    (text: string, cause: SaveCause) =>
      /*
       * `ifUnchanged` for an autosave and **not** for a Ctrl+S.
       *
       * The gap this closes is one autosave opens. The conflict bar below is raised from
       * `cide://session-tool`, which arrives as a Claude Code tool call completes and names the
       * file exactly — and which a `sed -i`, a `cargo fmt` or a `git checkout` never sends.
       * Before autosave, clobbering one of those took a deliberate Ctrl+S; with autosave-on-blur
       * it takes switching to the terminal, running `cargo fmt`, and clicking back into the
       * editor. Three things nobody decides to do.
       *
       * So a background write carries what the file was when the buffer last agreed with it, and
       * Rust refuses if the disk no longer matches. An explicit Ctrl+S passes `null` and forces,
       * because that is the user deciding — the same asymmetry `write_if_unchanged` documents
       * from the other end.
       */
      fileApi.write(path, text, cause === 'autosave' ? stampRef.current : null).then(
        (stamp) => {
          // The token moves with the write. Without this the *next* autosave would compare
          // against the file as it stood when the tab opened and refuse for ever after the
          // first save — a feature that works exactly once is worse than one that does not.
          stampRef.current = stamp
          setConflict(false)
          // After the write, not before: `didSave` makes rust-analyzer re-run `cargo check`, and
          // checking a file that is still mid-write is how you get a diagnostic for a truncated
          // buffer. This is also the notification the whole panel depends on — see `docSync.ts`.
          savedDoc(path)
        },
        (error: unknown) => {
          void diag.log(`could not save ${path}: ${String(error)}`)
          if (fileChanged(error)) {
            /*
             * The precondition refused: something else wrote this file. That is not a failure,
             * it is the *question* the conflict bar exists to ask — and raising it here is what
             * gives the `sed -i` path the same bar the agent path has had since M12.
             */
            setConflict(true)
          } else if (cause === 'autosave') {
            /*
             * A silent failed autosave is the worst outcome this feature can have.
             *
             * A failed Ctrl+S is a keystroke the user watched not work, and the tab staying
             * dirty plus the close guard is arguably report enough. A failed autosave happened
             * on a timer while they were looking somewhere else — at a browser, at a terminal —
             * and `void diag.log(...)` writes it to a file nobody opens. This population is
             * exactly where writes fail, too: a root-owned mode-644 file reports
             * `writable: true` (the mode bits, not "can *you* write it"), so `/etc/hosts` opened
             * through the out-of-project confirmation is a buffer that types fine and cannot be
             * saved.
             *
             * `notices.admit` dedupes by text, so a full disk gives one toast rather than sixty.
             * The timers are not rearmed after a failure either — see `EditorSurface` — so the
             * retries stop until the user touches the file again.
             */
            notify(`cide could not save ${basename(path)}: ${describe(error)}`, {
              kind: 'error',
              hint: 'Your changes are still in the buffer.',
            })
          }
          // Rethrown either way: the surface leaves the tab dirty on a rejected promise, and the
          // dirty flag is what puts the close confirmation in front of the user. Swallowing it
          // here would make a tab that looks saved over a buffer that is not.
          throw error
        },
      ),
    [path],
  )

  /**
   * Everything autosave refuses, assembled and handed to the one function that decides.
   *
   * The *facts* are gathered here because this is where they live — the setting is in the
   * mirror, the conflict bar is local state, the agent diff is a selector, the DOM half comes
   * from the surface — and the *decision* is `shouldAutosave`, which is pure and import-free so
   * `check:editor` can drive its whole truth table. That split is the point: every failure mode
   * in this feature is a refusal that was not made, and a refusal spelled inside a `useEffect`
   * is a refusal no check script can compile.
   *
   * `overlayOpen()` and `contextMenuOpen()` are read **at call time**, not captured, because
   * they are module-level getters over transient chrome and the answer at the moment of the blur
   * is the only one that matters. `cide_core::commands` already lists both as host context
   * flags — "transient chrome that only the React tree knows about" — so this is the
   * application's existing vocabulary for exactly this question rather than a new one.
   *
   * `readOnly` is passed even though a read-only buffer can never become dirty and therefore
   * never arms a timer. Stated rather than inferred: "unreachable" here is a property of two
   * other modules agreeing, and External Libraries sources are read-only by design.
   */
  const allowAutosave = useCallback(
    (
      reason: 'blur' | 'idle',
      dom: { windowFocused: boolean; focusInsideEditor: boolean },
    ): boolean =>
      shouldAutosave(reason, {
        enabled: autosaveOn,
        dirty: dirtyRef.current,
        readOnly: !writableRef.current,
        conflict,
        agentDiff,
        windowFocused: dom.windowFocused,
        focusInsideEditor: dom.focusInsideEditor,
        overlayOpen: overlayOpen(),
        contextMenuOpen: contextMenuOpen(),
      }),
    [autosaveOn, conflict, agentDiff],
  )

  /**
   * Follow the file on disk.
   *
   * `cide://session-tool` is the fast path — it arrives as the agent's tool call completes
   * and names the file exactly, well before any watcher would. It is explicitly *not* the
   * whole story: a `sed -i` in a shell pane or a `cargo fmt` never goes through a Claude
   * Code tool, and those need the fs watcher that lands with the file index. Wiring the
   * half that exists is still worth it — the agent editing a file the user has open is the
   * common case by a wide margin.
   *
   * Clean buffer reloads silently. A dirty one does not: overwriting unsaved edits because
   * something else touched the file is the one outcome that loses work, so it asks.
   */
  useEffect(() => {
    let unlisten: (() => void) | null = null
    // `dropped` and not just the null check: `listen` resolves a tick or more after it is
    // called, and a pane closed or split in that window would otherwise leave a subscription
    // nobody can cancel, holding this closure — and the path it captured — for the life of
    // the window. One editor pane per split per file makes that add up.
    let dropped = false
    void events
      .onSessionTool((_session, paths) => {
        if (!paths.includes(path)) return
        if (dirtyRef.current) setConflict(true)
        else read(true)
      })
      .then((fn) => {
        if (dropped) fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      dropped = true
      unlisten?.()
    }
  }, [path, read])

  /**
   * Follow the file across a git operation. (M20)
   *
   * The reported bug: pull a branch that changes a file you have open, and the pane goes on
   * showing what it showed before. `cide://session-tool` above covers the agent's edits and
   * nothing else, and a pull is not an agent edit.
   *
   * # Why this re-reads rather than reloading
   *
   * `cide://git-status` fires after **every** git mutation cide makes — a stage, an unstage, a
   * changelist move — and a `read(true)` on each of those would rebuild the `EditorView` and take
   * the user's scroll position, selection and undo history with it, several times per commit.
   * So the file is read and its **stamp compared**; only a stamp that actually moved gets the
   * reload. A read is cheap, and the rebuild is the part that costs something.
   *
   * A dirty buffer is never overwritten: it raises the same conflict bar an agent's edit does,
   * which is the one outcome here that could lose work.
   */
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let dropped = false
    void events
      .onGitStatus(() => {
        void fileApi
          .read(path)
          .then((doc) => {
            if (dropped || doc.stamp === stampRef.current) return
            if (dirtyRef.current) setConflict(true)
            else read(true)
          })
          .catch(() => {})
      })
      .then((fn) => {
        if (dropped) fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      dropped = true
      unlisten?.()
    }
  }, [path, read])

  if (load.kind === 'loading') {
    return <div className={styles.notice}>Opening {path}…</div>
  }
  if (load.kind === 'failed') {
    return (
      <div className={styles.notice}>
        <div className={styles.noticeTitle}>This file could not be opened</div>
        <div className={styles.noticeWhy}>{load.why}</div>
      </div>
    )
  }

  return (
    <div className={styles.pane}>
      {conflict && (
        <div
          className={styles.conflict}
          role="alert"
          /*
           * "I am pinned above this pane's content, and my height depends on what is in me."
           *
           * Read by exactly one selector — the `:has()` rule in `layout/PaneTitleBar.module.css`
           * that steps the floating ⊞ ⛶ ⧉ × cluster down past the find bar (M16). This bar's
           * height is *not* a constant: `.conflictText` is `flex: 1; min-width: 0` with no
           * `white-space`, so on a narrow pane the sentence wraps and the bar grows a line. A
           * fixed step over it would put the cluster through the middle of *Keep mine* — the very
           * button whose unclickability `EditorPane.module.css` writes up at length — so this bar
           * keeps reserving horizontally and the cluster stays where it is.
           *
           * It also settles the both-strips case: with this bar up, the find bar is the *second*
           * strip and none of it is under the cluster, so the cluster must not move — which is
           * what excluding this frame from that rule says.
           *
           * An attribute rather than a class because the reader is another CSS module and class
           * names are hashed per file. A *kind*, not a boolean: a future strip with a constant
           * height would say so and be stepped over instead.
           */
          data-pane-strip="fluid"
        >
          <span className={styles.conflictText}>
            This file changed on disk while you had unsaved changes.
          </span>
          <button type="button" className={styles.conflictButton} onClick={() => read(true)}>
            Reload from disk
          </button>
          <button
            type="button"
            className={styles.conflictButton}
            /*
             * Re-stamp, not just dismiss.
             *
             * `stampRef` still holds what the file looked like BEFORE whatever changed it, so
             * clearing the flag alone left every later autosave failing its own precondition:
             * the write is refused, the bar comes straight back, and the buffer is never written
             * again for the life of the tab. The user answered the question and the answer did
             * nothing.
             *
             * "Keep mine" means *my buffer is the truth now*, so the next write must be allowed
             * to land on top of what is there — which is exactly what dropping the guard says.
             * The following successful write records the new stamp, so the protection is back
             * one save later rather than gone.
             */
            onClick={() => {
              stampRef.current = null
              setConflict(false)
            }}
          >
            Keep mine
          </button>
        </div>
      )}
      <div className={styles.surface}>
        <OutlineFeed project={project} path={path} text={load.text} />
        <SyncFeed project={project} path={path} text={load.text} />
        <MaybeMarkdown
          path={path}
          project={project}
          view={mdView}
          onView={chooseView}
          readText={readText}
          subscribeText={subscribeText}
          onScreen={onScreen ?? true}
          scrollEditorTo={scrollEditorTo}
          onSyncHandle={onSyncHandle}
        >
        <EditorSurface
          path={path}
          root={root}
          project={project}
          onScreen={onScreen}
          doc={load.text}
          reloadKey={reloadKey}
          readOnly={!load.writable}
          onDirtyChange={reportDirty}
          onSave={onSave}
          /*
           * A fresh object every render, and that is safe *only* because `EditorSurface` holds
           * it in a ref and keeps it out of the build effect's dependency list. It has to be
           * fresh: `allow` closes over the conflict flag and the agent-diff selector, both of
           * which change while the user is typing, and a stale closure here would be a save
           * refused for a reason that stopped being true.
           */
          autosave={{
            idleMs: AUTOSAVE_IDLE_MS,
            ceilingMs: AUTOSAVE_CEILING_MS,
            allow: allowAutosave,
          }}
          onSelection={reportSelection}
          at={load.at}
          onView={reportPosition}
          onSaveHandle={registerSaveHandle}
          symbols={outline}
          onDocChanged={(read) => {
            // The same reader, kept for whoever asks next. `registerDirtyBuffer` above hands out
            // a closure over this ref rather than over `read` itself, so a blame started ten
            // minutes into an editing session gets the buffer as it is then. (M18)
            readTextRef.current = read
            // The markdown preview, if there is one. A `Set` and not a single callback: a
            // detached pane and its original are two components over one path.
            for (const listener of docListeners.current) listener()
            // Debounced in the store, and the text is read when the timer fires — so the popup,
            // the breadcrumb and the member walk follow the buffer rather than the last save.
            if (project !== undefined) scheduleOutline(project as ProjectId, path, read)
            // The same reader on its own timer: this is what gives the language server the
            // *unsaved* text, which is the whole of what it has over `cargo check` in a terminal.
            scheduleDoc(path, read)
            // And the extension workers, on a third timer of their own. (M22)
            //
            // Not folded into `scheduleOutline`: that one debounces a *round trip to Rust* and
            // this one debounces a `postMessage` to every worker with `editor:read`, and they are
            // sized for different costs. `noteActiveEditor` reads the text only when its timer
            // fires, so a burst of typing is one message rather than one per keystroke — which on
            // a megabyte file is the single most likely way an extension host makes typing slow.
            noteActiveEditor({
              path,
              languageId: languageIdFor(path),
              line: 1,
              read,
            })
          }}
          diagnostics={diagnostics}
          highlight={level}
          completion={completion}
          blame={blame}
          blameOn={blameOn}
          onShowCommit={onShowCommit}
          onScrollHandle={onScrollHandle}
          onFocusHandle={onFocusHandle}
          {...(onAnnotateParent === null ? {} : { onAnnotateParent })}
        />
        </MaybeMarkdown>
      </div>
    </div>
  )
}

/**
 * The markdown wrapper, or nothing at all. (M20)
 *
 * For a file the editor does not call Markdown this renders its children and nothing else, so a
 * `.rs` pane's tree is exactly what it was before this feature existed — no wrapper element, no
 * observer, no subscription.
 *
 * # What is and is not protected here
 *
 * React reconciles by position, so this branch flipping *does* re-parent the `EditorSurface` and
 * rebuild it. That is not worth avoiding, because the only thing that flips it is `path` changing
 * — a rename from `notes.txt` to `notes.md` — and `EditorSurface`'s build effect is keyed on
 * `[path, reloadKey]`, so a path change rebuilds the view in any case.
 *
 * The re-parenting that would matter is **changing layout**, and that one is genuinely avoided:
 * `MarkdownFrame` keeps the surface in the same `.buffer` element in all three layouts and only
 * changes its class, so switching text ⇄ split ⇄ preview does not touch the editor at all. See
 * that component's header for why preview mode hides the buffer rather than unmounting it.
 */
function MaybeMarkdown({
  path,
  project,
  view,
  onView,
  readText,
  subscribeText,
  onScreen,
  scrollEditorTo,
  onSyncHandle,
  children,
}: {
  path: string
  project: ProjectId | undefined
  view: MdView
  onView: (next: MdView) => void
  readText: () => string
  subscribeText: (listener: () => void) => () => void
  onScreen: boolean
  scrollEditorTo: (line: number) => void
  onSyncHandle: (follow: ((topLine: number) => void) | null) => void
  children: ReactNode
}): ReactNode {
  const openPath = useCallback(
    (target: string) => {
      if (project === undefined) {
        notify('cide cannot open a link from a file that is not in a project', { kind: 'warn' })
        return
      }
      void fileApi.open(project, target).catch((error: unknown) => {
        notify(describe(error), { kind: 'warn' })
      })
    },
    [project],
  )

  if (!isMarkdownPath(path)) return children
  return (
    <MarkdownFrame
      path={path}
      view={view}
      onView={onView}
      readText={readText}
      subscribeText={subscribeText}
      onScreen={onScreen}
      scrollEditorTo={scrollEditorTo}
      onSyncHandle={onSyncHandle}
      openPath={openPath}
    >
      {children}
    </MarkdownFrame>
  )
}

/**
 * Seed `outlineStore` for this buffer.
 *
 * A component with no markup rather than an effect inside `EditorPane`, because it needs to re-run
 * when the *loaded text* changes and `EditorPane`'s own effects are keyed on the path.
 *
 * This is the **load** path only. Keeping up with the user's typing is `onDocChanged` above, which
 * debounces through `scheduleOutline` — the two together are what make the breadcrumb, the File
 * Structure popup and the member walk follow the buffer rather than the last save.
 */
function OutlineFeed({
  project,
  path,
  text,
}: {
  project?: string | undefined
  path: string
  text: string
}): null {
  useEffect(() => {
    if (project === undefined) return
    fetchOutline(project as ProjectId, path, text)
  }, [project, path, text])

  // The extension workers get the same seed, on the same trigger. (M22)
  //
  // Here rather than in `EditorPane`'s own effects for this component's stated reason: it has to
  // re-run when the *loaded text* changes, and `EditorPane`'s effects are keyed on the path. A
  // worker told about a path and never about its text would have nothing to outline until the
  // user typed.
  useEffect(() => {
    noteActiveEditor({ path, languageId: languageIdFor(path), line: 1, read: () => text })
  }, [path, text])

  useEffect(() => () => forgetOutline(path), [path])
  return null
}

/**
 * Tell the language server this buffer exists, and keep telling it.
 *
 * A sibling of [`OutlineFeed`] rather than part of it, because the two answer to different owners:
 * the outline is ours and is derived on demand, while this is a *protocol* whose open/close pairs
 * have to balance across a process boundary. `docSync.ts` carries the reasoning, including the
 * measurement showing that without these notifications the panel freezes at the state the project
 * opened in.
 *
 * Typing is handled by `onDocChanged` above; the two effects here are the load path only.
 */
function SyncFeed({
  project,
  path,
  text,
}: {
  project?: string | undefined
  path: string
  text: string
}): null {
  /** The text this document was opened with, so the reload effect can recognise its own first run. */
  const opened = useRef<string | null>(null)

  /*
   * `text` is deliberately **not** a dependency of this effect.
   *
   * Including it would close and re-open the document on every reload from disk, and `didOpen` on
   * a URI that is already open is a protocol violation — the server would be holding two versions
   * of one file with no rule for which wins. A reload is a *change*, and that is the effect below.
   */
  useEffect(() => {
    if (project === undefined) return
    opened.current = text
    openDoc(project as ProjectId, path, text)
    return () => {
      opened.current = null
      closeDoc(path)
    }
  }, [project, path])

  useEffect(() => {
    if (project === undefined) return
    // Skips the run that pairs with the `openDoc` above — that text is already the server's.
    if (opened.current === null || opened.current === text) return
    opened.current = text
    resetDoc(path, text)
  }, [project, path, text])

  return null
}
