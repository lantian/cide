/**
 * The buffer, and everything CodeMirror needs to be one of this app's panes.
 *
 * The buffer and nothing else, since M11: the 28px breadcrumb bar that used to sit above it
 * is a line in the status bar now (`statusReadout.ts`), so the pane is all editor from the
 * tab strip down.
 *
 * Pure by design, in the same sense `panes/DiffPane.tsx` is: no IPC, no store, no knowledge
 * of tabs. Text comes in as a string and leaves through `onSave`, so the whole surface can
 * be driven from a fixture. `panes/EditorPane.tsx` is the piece that knows about files.
 *
 * Two things here are not the obvious implementation, and both are about not paying React
 * for something CodeMirror already does:
 *
 * * The cursor readout is pushed straight out of the update listener, through
 *   `statusReadout.ts`, into a DOM node the status bar owns. Holding `Ln 128, Col 24` in
 *   React state re-renders on every caret move, which on a held arrow key is 30 renders a
 *   second, each one re-running the effect guards below.
 * * The `EditorView` is created once per document and reconfigured afterwards. Rebuilding
 *   it on a prop change would drop the undo history, the scroll position and the selection
 *   — and, if the buffer were dirty, the user's edits.
 */
import { useCallback, useEffect, useMemo, useRef, type ReactNode } from 'react'
import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete'
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  moveLineDown,
  moveLineUp,
} from '@codemirror/commands'
import {
  bracketMatching,
  indentOnInput,
  indentUnit,
  syntaxHighlighting,
} from '@codemirror/language'
import { Compartment, EditorSelection, EditorState, type Extension } from '@codemirror/state'
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  rectangularSelection,
} from '@codemirror/view'
import { cideHighlightStyle } from './highlight'
import { useCodeMenu } from './codeMenu'
import { lineEditKeymap } from './editorKeys'
import { findExtensions } from './find'
import { minimap } from './minimap'
import { foldSpecFor, languageName, loadLanguage } from './languages'
import {
  foldAllRanges,
  foldEffectsFor,
  foldExtensions,
  foldHere,
  foldRecursive,
  toggleFoldHere,
  unfoldAllRanges,
  unfoldHere,
  unfoldRecursive,
} from './folding'
import { captureLineEndings, restoreLineEndings, type DocumentEndings } from './lineEndings'
import type { AutosaveReason } from './autosave'
import { exceedsBytes } from './byteSize'
import { pendingReveals, planReveal, registerReveal } from './revealRequest'
import { planRestore, type FileView } from './position'
import { viewTracker } from './viewTracker'
import { navRecorder } from './navRecorder'
import {
  claimStatusReadout,
  formatReadout,
  pathTrail,
  sameTrail,
  type ReadoutSlot,
} from './statusReadout'
import { claimCaret, type CaretSlot } from './caretTrack'
import { ctrlLink, wordTargetAt } from './ctrlLink'
import { trailNames, type OutlineNode } from './memberNav'
import { lintRanges, type LintSource } from './lintMap'
import { lintGutter, setDiagnostics } from '@codemirror/lint'
import { blameExtension, setBlame } from './blame'
import type { BlameMarker } from './blameModel'
import { useSendToClaude } from './useSendToClaude'
import styles from './EditorSurface.module.css'

/**
 * Past this size the buffer opens without a language, without wrapping and without the
 * bracket and indent helpers.
 *
 * The gate is size, not line count, and the reason is `StreamLanguage` specifically: a
 * stream parser has no random access, so reaching line 100,000 means tokenizing every
 * character before it. CodeMirror does that incrementally under a time budget and never
 * blocks — but on a file this size the parse never catches up with the viewport, so the
 * colour is absent anyway and the work is pure cost. Turning it off is the honest version
 * of what would otherwise happen.
 *
 * One megabyte is comfortably above every hand-written source file in this repository (the
 * largest is 60 KB) and comfortably below the 5 MB the milestone tests with.
 *
 * Bytes, and measured as bytes — see `byteSize.ts`. Compared against `source.length` this
 * would be UTF-16 code units, and a file of non-Latin prose would be measured at up to a
 * third of its real size.
 */
export const HIGHLIGHT_LIMIT_BYTES = 1024 * 1024

export interface EditorSurfaceProps {
  /** Absolute path, used for the status bar's trail and to pick the language. */
  path: string
  /**
   * Which **document** this buffer is, for everything keyed per buffer. Defaults to [`path`]. (M18)
   *
   * For an editor over a file on disk the two are the same thing, which is why the default exists
   * and why `panes/EditorPane.tsx` passes nothing. The exception is a buffer showing a file *as
   * some commit left it*: `panes/RevisionPane.tsx` draws `src/log.rs` at `a1b2c3d`, which is a
   * different document at the same path, and two documents sharing one registry key is two buffers
   * fighting over one slot.
   *
   * # The one that actually bites is `registerReveal`
   *
   * `revealRequest.ts::requestReveal` delivers to *live* receivers and only parks when there are
   * none. A revision pane registered under the real path therefore makes a search-result click
   * into that file silently stop moving the caret: the click finds a live receiver, parks nothing,
   * moves the caret in a read-only buffer of a forty-commit-old version of the file, and the file
   * tab that opens a moment later is never told. That is exactly the report `revealRequest.ts`
   * exists to answer, reintroduced by a viewer nobody would think to suspect.
   *
   * The other four are hygiene by comparison and are the same mistake: `viewTracker`'s per-file
   * view memory would remember a revision's scroll as the working file's, `ctrlLink`'s cache would
   * answer one buffer's Ctrl+hover from another's, `navRecorder` would put Back-stack entries under
   * the real path, and `claimCaret` would hand every surface that reads the caret a path plus a
   * line number from a document that no longer exists.
   *
   * # What it does not touch, deliberately
   *
   * [`path`] keeps every job that is about the *file*: `languageName` and `loadLanguage` (so
   * `a1b2c3d:src/log.rs` is still Rust), `pathTrail` and the status bar's claim, the find bar, the
   * context menu and ⌥⏎'s mention. That split is the whole point of the default — a caller that
   * passes nothing cannot change behaviour, and a caller that passes an identity changes only which
   * key the registries use. The two exceptions inside that split are argued where they are made:
   * `claimStatusReadout` below keeps the path, `claimCaret` beside it takes the identity.
   *
   * `useCodeMenu` keeps the path too, and one piece of that is a real residue rather than a
   * decision: `highlightLevel.ts` is a genuine per-path registry, so *Highlighting: syntax only*
   * chosen in a revision pane records an override against the working file. It is session-only,
   * never persisted, and the menu is the only way to reach it — but it is the one collision this
   * prop does not close, and `editor/codeMenu.tsx` takes a single path for five items that all
   * want the real one.
   *
   * # The contract: an identity is fixed for the life of a mount
   *
   * The build effect below is keyed on `[path, reloadKey]` and not on this, so an identity that
   * changed while `path` stayed put would leave the registrations under the *previous* one — live
   * receivers for a document nobody is showing. Every caller satisfies that structurally rather
   * than by care: an identity is a pure function of the path, and Rust keys `TabKind::Revision` on
   * `(repo, path, rev)`, so another revision is another tab and therefore another mount.
   *
   * It is a contract rather than a second dependency because that array is the one thing in this
   * file that must not grow. Everything ever added to it has cost somebody their scrollback, their
   * undo history or their unsaved edits, `check:blame` and `check:editor` both pin it letter for
   * letter for that reason, and a caller that really does need a new identity in a live buffer
   * already has [`reloadKey`], which exists to rebuild the view on purpose.
   */
  identity?: string | undefined
  /**
   * The project root the trail is drawn relative to.
   *
   * Absent shows the whole absolute path, which is right for a file outside any project and
   * wrong-looking for one inside it — `crates › cide-core › src › lib.rs` is the mock, not
   * `home › lantian › work › cide › crates › …`. See `pathTrail`.
   */
  root?: string | undefined
  /**
   * The project this buffer belongs to, for Go to definition.
   *
   * Distinct from `root`, which is only ever used to shorten a path for display: this one is an
   * identity the backend resolves against. Passed down rather than derived here — see
   * `CodeMenuOptions.project` for why deriving it from the window's role is wrong.
   */
  project?: string | undefined
  /** The file exactly as it came off disk, line endings included. */
  doc: string
  /**
   * Bumped by the caller to replace the buffer from disk.
   *
   * A changed `doc` alone is deliberately ignored. The prop is a string and a parent that
   * recomputes it would otherwise reset the buffer under a half-typed edit; making the
   * reload explicit means a reload only happens when something decided one should.
   */
  reloadKey?: number | undefined
  readOnly?: boolean | undefined
  onDirtyChange?: ((dirty: boolean) => void) | undefined
  /**
   * Receives the buffer with the file's original line endings restored.
   *
   * The returned promise decides the tab's dirty state: the buffer is only considered
   * saved once it resolves. A handler that returns nothing is treated as having succeeded,
   * which is right for a fixture and wrong for anything that can fail — so anything that
   * writes should return its promise.
   */
  onSave?: ((text: string, cause: SaveCause) => void | Promise<void>) | undefined
  /**
   * Write a dirty buffer without anybody asking. (M15)
   *
   * `undefined` turns the whole thing off, which is what a fixture and a diff pane get. The
   * **policy** is not here: `allow` is `autosave.shouldAutosave` bound to facts only the pane
   * knows (the conflict bar, a pending agent diff, the setting), and everything in this file is
   * the mechanism that asks it.
   *
   * `idleMs`/`ceilingMs` are passed rather than imported so the two timers can be driven from a
   * test without waiting a minute — and, more usefully, so this file states that the ceiling
   * exists rather than leaving it to be discovered in `autosave.ts`.
   *
   * **This must not enter the build effect's dependency list.** That effect is keyed on
   * `[path, reloadKey]` and its comment says why: a changed identity there tears the view down
   * and takes the user's unsaved edits with it. Held in a ref like every other callback here,
   * so toggling the setting takes effect on the next keystroke — which is right.
   */
  autosave?:
    | {
        idleMs: number
        ceilingMs: number
        allow: (
          reason: AutosaveReason,
          dom: { windowFocused: boolean; focusInsideEditor: boolean },
        ) => boolean
      }
    | undefined
  /** Called on focus, so the pane tree can follow the caret. */
  onFocus?: (() => void) | undefined
  /**
   * The selection, for Claude Code's `selection_changed` notification.
   *
   * Lines are **1-based here** and converted to the protocol's 0-based at the Rust boundary,
   * so nothing in between has to remember which convention it is holding. Fires on every
   * selection change including an empty one — a caret move is a selection of zero
   * characters, and the CLI's status wants to follow the caret, not only a highlight.
   *
   * Debouncing is the caller's: a drag fires this once per animation frame.
   */
  onSelection?:
    | ((selection: { text: string; startLine: number; endLine: number }) => void)
    | undefined
  /**
   * Receives an awaitable save for this buffer, and `null` when the editor goes away.
   *
   * The buffer lives in a CodeMirror state that nothing above this component can reach, so
   * *Save and close* would otherwise be a button the dialog could not honour. Handing the
   * function out is narrower than lifting the text: the caller can write the file and cannot
   * read it.
   */
  onSaveHandle?: ((save: (() => Promise<void>) | null) => void) | undefined
  /**
   * Hand out a way to scroll this buffer to a line, without moving the caret. (M20)
   *
   * The same shape as [`onSaveHandle`] and for the same reason — a live view outlives any value a
   * callback could close over — and the same lifetime: handed out when the view is built, handed
   * back as `null` in the cleanup, so nothing can scroll a destroyed editor.
   *
   * **Not a reveal, and the difference is the whole point.** `revealRequest.ts` and `jump.ts`
   * move the *selection* and then scroll it into view, which is right for Go to definition and
   * wrong for this: the caller is the markdown preview keeping the buffer in step as somebody
   * scrolls the rendering, and a selection that walked down the file while they read it would be
   * a selection they did not make, in a buffer they are about to type into.
   */
  onScrollHandle?: ((scrollTo: ((line: number) => void) | null) => void) | undefined
  /**
   * The buffer changed, and here is how to read it. (M12)
   *
   * **No text is passed.** This fires on every keystroke, and `doc.toString()` on a five-megabyte
   * rope per character is exactly the cost the rest of this file goes out of its way to avoid —
   * the cursor readout is written straight into a DOM node for the same reason. The caller
   * debounces and then calls `read()`, which returns the buffer *as it is then*, not as it was
   * when the change fired.
   *
   * The same shape as [`onSaveHandle`], and for the same reason: a live view outlives any value
   * a callback could have closed over.
   */
  onDocChanged?: ((read: () => string) => void) | undefined
  /**
   * This file's structure, for the status bar's `mod › impl › fn` trail. (M12)
   *
   * Passed **in** rather than read from a store, so this module stays what its header says it is:
   * text in, text out, no IPC. `outlineStore` reaches `client.ts`, and importing it here would
   * make every editor transitively depend on the wire.
   *
   * The arithmetic over it is `memberNav.ts`, which is pure and import-free — so the trail is
   * computed in the update listener below without a round trip, which is the whole reason the
   * outline is cached in the first place.
   */
  symbols?: readonly OutlineNode[] | undefined
  /**
   * Whether this editor's **tab is the one in front**. (M16)
   *
   * The one fact about a file pane that no part of the editor can observe: `TabContent` hides an
   * inactive tab with `visibility: hidden` and never unmounts it, so a background editor is
   * mounted, laid out at full size, still painting, and — before this — indistinguishable from
   * the one the user is reading. `TabContent` has computed and passed the flag since M4
   * (`renderTree: (tab, active) => ReactNode`, documented there for exactly this class of
   * consumer); `App.tsx` wrote `renderTree={(tab) =>` and threw it away.
   *
   * Two things read it, both about the status bar's single slot: a mount behind another tab must
   * not claim it, and a tab coming forward must take it. See `statusReadout.ts`'s header for the
   * two reports.
   *
   * Deliberately **not** the same thing as `tree.focused`. `PaneBody` dispatches on the pane
   * kind, so a File tab holds exactly one editor pane by construction — being the active tab is
   * therefore the whole of "is this editor on screen", and routing it through the focused-pane
   * machinery would tie the bar to a second, slower answer.
   */
  onScreen?: boolean | undefined
  /**
   * This file's problems, already filtered by the host. (M12)
   *
   * Filtered *before* it gets here, deliberately: the panel, the status bar and the rail badge
   * all read one snapshot that `App.tsx` filters once, and an editor applying the severity rules
   * itself would be a fourth place for them to disagree. What this component decides is only
   * *where* to draw them.
   */
  diagnostics?: readonly LintSource[] | undefined
  /**
   * How much to draw. IDEA's highlighting-level widget, per editor.
   *
   * `none` draws nothing; `syntax` is applied by the host's filter, not here — by the time a list
   * arrives it is already the right list. This prop only decides whether the lint extension is in
   * the compartment at all, so `none` also removes the gutter column rather than leaving an empty
   * one.
   */
  highlight?: 'none' | 'syntax' | 'all' | undefined
  /**
   * Where the user was last time this file was on screen. (M12)
   *
   * Applied once, in the dispatch that follows construction, and only when no explicit
   * navigation is parked for this path — `planRestore` owns that rule and says why.
   *
   * Read through a ref and **deliberately not a dependency of the build effect**: this value's
   * whole life cycle is that the pane fetches it, hands it down, and then the editor starts
   * *producing* newer ones through [`onView`]. A prop that both feeds the effect and is
   * refreshed by it would rebuild the `EditorView` — losing scrollback, undo history and any
   * unsaved edits — every time the user scrolled.
   */
  at?: FileView | null | undefined
  /**
   * The buffer's view moved: the caret, the first visible line, or both. (M12)
   *
   * Fires at most once an animation frame, and only when the answer actually changed — see
   * `viewTracker.ts`. **The caller must debounce before it does anything expensive**, the same
   * contract [`onSelection`] carries and for the same reason; `EditorPane` trailing-debounces
   * this at 500 ms and flushes it on unmount.
   */
  onView?: ((at: FileView) => void) | undefined
  /**
   * Who wrote each line, already collapsed into one marker per line. (M18)
   *
   * Computed by `panes/EditorPane.tsx` with `blameModel.collapseRuns` and passed **in**, so this
   * module stays what its header says it is: text in, text out, no IPC. `blameStore.ts` reaches
   * `client.ts`, and importing it here would make every editor — including the fixtures and the
   * diff panes — transitively depend on the wire.
   *
   * `null` and `[]` are different: `null` is *nothing has been fetched*, `[]` is a run set the
   * model refused (see `collapseRuns`) or a file with no lines. Neither draws a cell; only
   * [`blameOn`] decides whether the column is there at all.
   *
   * **Not a dependency of the build effect.** Neither is [`blameOn`]. That effect is keyed
   * `[path, reloadKey]` and rebuilding the `EditorView` costs the undo history, the scroll
   * position, the selection and any unsaved edits — turning a column on must not do that, and a
   * refresh of it certainly must not. `check:blame` asserts both names are absent from that array.
   */
  blame?: BlameMarker[] | null | undefined
  /** Whether the column is showing. The gutter's compartment holds the extension only while true. */
  blameOn?: boolean | undefined
  /**
   * A gutter cell, or the card's *Show in log*, was clicked. The argument is a full oid.
   *
   * Optional, and absent in a fixture and in a diff pane: an editor with no host to answer this
   * simply has no click target, which is better than a control that reports a refusal.
   */
  onShowCommit?: ((oid: string) => void) | undefined
  /** The card's *Annotate previous revision*, with the oid the hop starts from. */
  onAnnotateParent?: ((oid: string) => void) | undefined
  /**
   * *Show history for this file*, from the buffer's own context menu. (M18)
   *
   * Takes the **absolute** path, which this surface already has. Absent in a window with no tool
   * window, which disables the item with a reason rather than hiding it — a menu that silently
   * loses a line reads as a menu that never had it.
   */
  onShowHistory?: ((path: string) => void) | undefined
}

/** `Ln 128, Col 24`, one-based in both, which is what every editor and every stack trace uses. */
/**
 * Which of the three things asked for this write.
 *
 * Carried all the way to `EditorPane`'s rejection handler, because the *report* differs: a
 * failed Ctrl+S is a keystroke the user watched not work, and the tab staying dirty is arguably
 * report enough; a failed autosave happened on a timer while they were looking somewhere else,
 * and a silent one is the worst outcome this feature can have.
 */
export type SaveCause = 'manual' | 'autosave'

export function cursorLabel(state: EditorState): string {
  const head = state.selection.main.head
  const line = state.doc.lineAt(head)
  return `Ln ${line.number}, Col ${head - line.from + 1}`
}

export function EditorSurface({
  path,
  // Defaulted from `path` in the pattern itself rather than in a `??` below, so the fallback is
  // visible at the one place a reader looks for a prop's default and there is no second binding
  // that could be used by mistake. See the prop's doc comment for the split between the two.
  identity = path,
  root,
  project,
  doc,
  reloadKey = 0,
  readOnly = false,
  onDirtyChange,
  onSave,
  autosave,
  onFocus,
  onSelection,
  onSaveHandle,
  onScrollHandle,
  onDocChanged,
  symbols,
  diagnostics,
  highlight = 'all',
  at,
  onView,
  onScreen = true,
  blame = null,
  blameOn = false,
  onShowCommit,
  onAnnotateParent,
  onShowHistory,
}: EditorSurfaceProps): ReactNode {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const viewRef = useRef<EditorView | null>(null)
  /** The live view's lint compartment, so the push effect can reconfigure it. */
  const lintSlotRef = useRef<Compartment | null>(null)
  /** The same, for the blame column. (M18) */
  const blameSlotRef = useRef<Compartment | null>(null)
  /** This buffer's hold on the status bar, for the trail effect below. See `statusReadout.ts`. */
  const slotRef = useRef<ReadoutSlot | null>(null)

  // Callbacks in refs so a parent that rebuilds its handlers every render cannot reach the
  // effect below and tear the editor down — which would take the user's unsaved edits.
  const saveCb = useRef(onSave)
  saveCb.current = onSave
  const dirtyCb = useRef(onDirtyChange)
  dirtyCb.current = onDirtyChange
  const focusCb = useRef(onFocus)
  focusCb.current = onFocus
  const selectionCb = useRef(onSelection)
  selectionCb.current = onSelection
  const saveHandleCb = useRef(onSaveHandle)
  saveHandleCb.current = onSaveHandle
  const scrollHandleCb = useRef(onScrollHandle)
  scrollHandleCb.current = onScrollHandle
  const docChangedCb = useRef(onDocChanged)
  docChangedCb.current = onDocChanged
  const viewCb = useRef(onView)
  viewCb.current = onView
  /*
   * In a ref, and **not** in the build effect's dependency list.
   *
   * `EditorPane` rebuilds this object every render — `allow` closes over the conflict flag and
   * over a selector result — so listing it at `[path, reloadKey]` would tear the view down on
   * every render of the pane and take the user's unsaved edits with it. The file already warns
   * about exactly that for `onSave`; this is the same hazard with a shorter fuse, because the
   * facts it carries change while the user types.
   */
  const autosaveCb = useRef(autosave)
  autosaveCb.current = autosave
  const atRef = useRef(at)
  atRef.current = at
  /**
   * The most recent view this component itself observed, whatever the props say.
   *
   * This is what makes *reload from disk* keep the user's place, and it is the one part of this
   * feature where the obvious implementation is quietly wrong. `EditorPane` bumps `reloadKey`
   * when the agent edits the open file, the build effect re-runs, and the new `EditorView` has
   * to be restored from *where the user was a moment ago* — not from the `at` prop, which was
   * fetched when the tab opened, and not from the Rust store, which holds whatever the 500 ms
   * debounce last managed to send. Both of those are stale by exactly the amount the user has
   * scrolled since, which on a file they are actively reading is all of it.
   *
   * Guarded on the **identity** so a *different* document does not inherit this one's line
   * number — `viewTracker` below stamps its own key onto every observation it publishes, and the
   * comparison further down has to ask the same question with the same word. For every editor
   * over a file that is the path; for a revision buffer it is the thing that stops `src/log.rs`
   * at `a1b2c3d` from restoring the working file's scroll position.
   */
  const observedRef = useRef<FileView | null>(null)

  /*
   * The blame column's four moving parts, all in refs and none of them in the build effect's
   * dependency list — the hazard `autosave` above writes up, with the same fuse. (M18)
   *
   * `blameRef`/`blameOnRef` are read once at construction so a view rebuilt by a `reloadKey` bump
   * comes back with the column already on; the effect further down handles every later change.
   * The two callbacks are in refs because `EditorPane` rebuilds them per render and they are
   * captured *inside* the memoised extension below, which must never be rebuilt.
   */
  const blameRef = useRef(blame)
  blameRef.current = blame
  const blameOnRef = useRef(blameOn)
  blameOnRef.current = blameOn
  const showCommitCb = useRef(onShowCommit)
  showCommitCb.current = onShowCommit
  const annotateParentCb = useRef(onAnnotateParent)
  annotateParentCb.current = onAnnotateParent

  /**
   * The gutter extension, built **once** for the life of this component.
   *
   * `Compartment.reconfigure` is given this value, and CodeMirror keeps a `StateField`'s contents
   * across a reconfigure only while the field is the same object. A fresh `blameExtension(…)` per
   * render would therefore mint a fresh field, throw away the markers and the `touched` set, and
   * tear the gutter's DOM down — on every keystroke, since `blame` changes identity whenever the
   * store answers. Memoised with no dependencies, with the callbacks read through refs, so the
   * value never changes and reconfiguring with it twice is a no-op.
   */
  const blameExt = useMemo(
    () =>
      blameExtension({
        onShowCommit: (oid) => showCommitCb.current?.(oid),
        /*
         * Spread in only when the host supplied one. The card draws the second button from the
         * *presence* of this key, so handing over a closure that quietly calls nothing would put a
         * button on screen that does nothing — the failure this project has paid for repeatedly.
         *
         * The memo therefore keys on the presence and not on the function: the identity of the
         * callback changes on every render of the pane and must not reach this, but a host that
         * gains or loses the ability to answer is a real change and is allowed to rebuild it.
         */
        ...(onAnnotateParent === undefined
          ? {}
          : { onAnnotateParent: (oid: string) => annotateParentCb.current?.(oid) }),
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [onAnnotateParent === undefined],
  )

  /*
   * The right-click menu. Reads the view through a getter rather than being handed it, because
   * `items` runs at open time and the view alive then is the one to act on — a captured
   * `viewRef.current` from render time is a destroyed editor after a reload.
   */
  const { onContextMenu, menu } = useCodeMenu({
    view: () => viewRef.current,
    path,
    readOnly,
    project,
    /*
     * The buffer's *effective* level, which is the right fallback in both cases: with a per-file
     * override it already equals that override, and without one it equals the workspace default.
     * The option was previously supplied by nobody, so `checked: levelFor(path, defaultLevel)`
     * fell back to the literal `all` and a user whose default was `Syntax only` saw the tick on
     * `All problems` while the buffer showed neither.
     */
    defaultLevel: highlight,
    /*
     * The two git items. Both are *optional* on `CodeMenuOptions`, so before this line they were
     * drawn disabled with their own sentences — which is the correct fallback and is exactly why
     * nothing broke while the two halves of this feature were built separately. Passing them is
     * what makes them live.
     *
     * `blameOn` is a `checked` mark rather than a renamed label: a menu item that renames itself
     * between openings is one the user has to re-read every time.
     */
    blameOn,
    ...(onShowHistory === undefined ? {} : { onShowHistory }),
  })

  /*
   * *Send lines to Claude*, from the keyboard.
   *
   * In a ref for the same reason every other callback here is: this one's identity changes
   * whenever the workspace mirror moves the mention target, and a changed identity reaching the
   * effect below would tear the editor down and take the user's unsaved edits with it.
   *
   * The menu is not enough on its own. A mouse-only gesture is a gesture most people never
   * find, and Ctrl+P's own footer already advertises ⌥⏎ for "send to Claude" — so the editor
   * answering the same chord is matching a promise the app makes elsewhere, not inventing one.
   */
  const toClaude = useSendToClaude()
  const sendCb = useRef(toClaude)
  sendCb.current = toClaude

  const segments = useMemo(() => pathTrail(path, root), [path, root])
  /*
   * The outline, in a ref rather than a dependency.
   *
   * The effect below is keyed on `[path, reloadKey]` and rebuilds the whole `EditorView` when it
   * re-runs — scrollback, selection, undo history and any in-flight composition included. A
   * re-parse arriving three hundred milliseconds after a keystroke must not do that, so the
   * listener reads the latest value through a ref instead.
   */
  const symbolsRef = useRef<readonly OutlineNode[]>(symbols ?? [])
  symbolsRef.current = symbols ?? []
  /*
   * And the path segments, in a ref for the mirror-image reason — which was a live bug. (M16)
   *
   * `segments` is memoized on `[path, root]` and the build effect is keyed on `[path, reloadKey]`,
   * so a **root that arrives late** produces new segments without re-running the effect: the
   * update listener kept the array it closed over at build time. The `[segments]` effect below
   * repaired the bar once, and then the very next caret move republished the stale absolute trail
   * over it. Reading through a ref is what makes the listener and the effect agree about what
   * this file is called.
   */
  const segmentsRef = useRef<readonly string[]>(segments)
  segmentsRef.current = segments
  /** The trail as last published, so nothing below has to compare arrays it has not built. */
  const publishedRef = useRef<readonly string[]>(segments)
  const onScreenRef = useRef(onScreen)
  onScreenRef.current = onScreen

  /*
   * `src/main.rs › impl Parser › parse`, published if and only if it moved.
   *
   * Takes the caret rather than reading it, because its hot caller — the update listener — has
   * already paid for `doc.lineAt` and must not pay twice: this runs on every selection change,
   * which under a held arrow key is thirty times a second, and publishes on almost none of them.
   * That is what keeps the trail off the same budget as the `Ln 7, Col 48` readout beside it,
   * which goes straight into a DOM node for exactly this reason.
   *
   * A function rather than three copies is not tidiness. There are **three** callers now — the
   * listener, a root that arrived late, and an outline that arrived late — and the last two are
   * the ones that were missing: a freshly opened tab showed a path with no symbols until the user
   * moved the caret, and the late-root repair called `setTrail(segments)` with the path alone,
   * *truncating* a symbol tail that was already on screen.
   */
  const publishTrail = useCallback((line: number, column: number) => {
    const slot = slotRef.current
    if (slot === null) return
    const segs = segmentsRef.current
    const next = [...segs, ...trailNames(symbolsRef.current, line, column)]
    if (sameTrail(publishedRef.current, next)) return
    publishedRef.current = next
    // The split index travels with the trail: `StatusBar` has to know where the path ends and
    // the symbols begin, or it draws `parse` as a clickable directory. See `statusReadout.ts`.
    slot.setTrail(next, segs.length)
  }, [])

  /** The same, for a caller with no caret in hand — it reads the live view for one. */
  const rebuildTrail = useCallback(() => {
    const view = viewRef.current
    if (view === null) return
    const head = view.state.selection.main.head
    const at = view.state.doc.lineAt(head)
    publishTrail(at.number, head - at.from + 1)
  }, [publishTrail])

  const language = useMemo(() => languageName(path), [path])
  const endings = useMemo(() => captureLineEndings(doc), [doc])

  // Captured per load, because after `EditorState.create` the buffer's endings are all `\n`
  // and the question can no longer be asked. See `lineEndings.ts`.
  const endingRef = useRef<DocumentEndings>(endings)
  const dirtyRef = useRef(false)

  useEffect(() => {
    const host = hostRef.current
    if (host === null) return

    const source = doc
    const oversize = exceedsBytes(source, HIGHLIGHT_LIMIT_BYTES)
    // `endings` is memoized on the same `doc` this effect captured, so reusing it here is
    // the same answer for one scan instead of two — and it keeps the readout and the bytes
    // that get written from ever disagreeing about what the file was.
    endingRef.current = endings
    dirtyRef.current = false

    const languageSlot = new Compartment()
    /*
     * The lint gutter, reconfigurable without rebuilding the view.
     *
     * A `Compartment` for the same reason `languageSlot` is one: the effect that builds this view
     * is keyed on `[path, reloadKey]`, and rebuilding it would take scrollback, selection, undo
     * history and any in-flight composition with it. Turning highlighting off for one file must
     * not cost the user their undo stack.
     */
    const lintSlot = new Compartment()
    /*
     * The blame column, in a compartment of its own for the same reason. (M18)
     *
     * Seeded from a ref rather than from the prop, because this effect is keyed `[path, reloadKey]`
     * and listing `blameOn` there would rebuild the whole view — undo history, scroll, selection,
     * unsaved edits — every time the column was toggled. The ref is what makes a `reloadKey` bump
     * come back with the column still on.
     */
    const blameSlot = new Compartment()
    let baseline: EditorState['doc'] | null = null
    /*
     * The status bar's line, claimed once the view exists further down — the update
     * listener is built before the editor it listens to, so this is a `let` rather than a
     * parameter. Every use is optional-chained: nothing dispatches into a view during
     * construction, and a listener that fired before the claim would be reporting a position
     * in a buffer the user cannot see yet.
     */
    let readout: ReadoutSlot | null = null
    let caret: CaretSlot | null = null
    const readoutFor = (state: EditorState): string =>
      formatReadout({ language, ending: endings.ending, cursor: cursorLabel(state) })

    /**
     * Write the buffer and resolve once it has landed — or reject.
     *
     * Separate from the keymap's `save` below because the two callers want opposite things.
     * A `Mod-s` handler must return a boolean synchronously and swallow the failure (the tab
     * staying dirty is the report). *Save and close* must await it and must know if it
     * failed, because closing after a failed write is exactly the loss the confirmation
     * exists to prevent.
     */
    const saveNow = (view: EditorView, cause: SaveCause = 'manual'): Promise<void> => {
      if (readOnly) return Promise.resolve()
      const saving = view.state.doc
      const text = restoreLineEndings(saving.toString(), endingRef.current)
      return Promise.resolve(saveCb.current?.(text, cause)).then(() => {
        baseline = saving
        setDirty(!view.state.doc.eq(saving))
      })
    }

    const save = (view: EditorView): boolean => {
      if (readOnly) return false
      const saving = view.state.doc
      const text = restoreLineEndings(saving.toString(), endingRef.current)

      // The baseline moves only once the write has landed, and this ordering is the whole
      // of it. Clearing the dirty flag optimistically reads better and is wrong: a write
      // that fails — read-only mount, disk full, the file replaced by a directory — would
      // leave a tab that looks saved over a buffer that is not. Rust *refuses* to close a
      // tab carrying this flag without an explicit `force`, so staying dirty through a
      // failed save is not a belt-and-braces measure — it is what puts the close
      // confirmation in front of the user, and clearing it early is what would let the next
      // `×` discard the write that never landed.
      void Promise.resolve(saveCb.current?.(text, 'manual')).then(
        () => {
          baseline = saving
          // Compared against the buffer as it is *now*, not as it was when the write
          // started: the user may have typed during the round trip, and those keystrokes
          // are unsaved.
          setDirty(!view.state.doc.eq(saving))
        },
        () => {
          // The caller reports the failure; the tab simply stays dirty.
        },
      )
      return true
    }

    const setDirty = (next: boolean): void => {
      if (dirtyRef.current === next) return
      dirtyRef.current = next
      dirtyCb.current?.(next)
    }

    /*
     * ---------------------------------------------------------------------------------------
     * Autosave. (M15)
     *
     * Two `let` handles inside the build effect rather than a module-level registry, and that
     * placement *is* the defence against the worst bug this feature can have. The effect's
     * cleanup already runs on every `reloadKey` bump, and a destroyed `EditorView` still
     * answers `view.state.doc` — so a timer that outlived its view would write the buffer as it
     * stood **before** the reload straight over the file that replaced it. A registry keyed by
     * tab (the `paneHosts.ts` shape) is the obvious alternative and is exactly wrong here: it
     * would outlive the view a reload replaced, which is the one lifetime that must not be
     * outlived.
     * ---------------------------------------------------------------------------------------
     */
    let idleTimer: ReturnType<typeof setTimeout> | undefined
    let ceilingTimer: ReturnType<typeof setTimeout> | undefined

    const disarm = (): void => {
      if (idleTimer !== undefined) clearTimeout(idleTimer)
      if (ceilingTimer !== undefined) clearTimeout(ceilingTimer)
      idleTimer = undefined
      ceilingTimer = undefined
    }

    /**
     * The DOM half of the blur question, read at the moment of asking.
     *
     * Both facts are about *this instant* and neither is derivable from the `ViewUpdate`, so
     * they are gathered here and handed to the policy rather than the policy reaching for
     * globals — which is what keeps `shouldAutosave` drivable under node.
     */
    const domFacts = (view: EditorView) => ({
      // `document.hasFocus()` is false exactly when the OS window is deactivated, which is the
      // frame-deactivation save IDEA is known for. `view.hasFocus` is `document.hasFocus() &&
      // activeElement === contentDOM`, so the two together separate "the caret moved" from "the
      // window went away" — and the policy has to check the second first.
      windowFocused: typeof document === 'undefined' || document.hasFocus(),
      // The find bar is a CodeMirror *panel* inside `view.dom`, so opening it blurs the content
      // without leaving the file. One test covers it and anything else that ever lives there.
      focusInsideEditor:
        typeof document !== 'undefined'
        && document.activeElement !== null
        && view.dom.contains(document.activeElement),
    })

    /** Ask, and write if the answer is yes. Never throws into a CodeMirror update. */
    const autosaveIf = (view: EditorView, reason: AutosaveReason): void => {
      const config = autosaveCb.current
      if (config === undefined) return
      if (!config.allow(reason, domFacts(view))) return
      /*
       * Disarmed *before* the write is issued, not after it resolves.
       *
       * Both timers describe "this buffer is owed a save", and one is now in flight — so the
       * other would fire against a buffer that is either clean by then or has just been refused,
       * and in the refused case it would retry every sixty seconds for the life of the tab. A
       * keystroke re-arms, which is the moment the user's attention is back on this file.
       *
       * Note what this does *not* disarm: a save the policy refused. That path returns above, so
       * a blur while the palette is open leaves the idle timer running — which is exactly right,
       * because the buffer really is still owed a save.
       */
      disarm()
      void saveNow(view, 'autosave').catch(() => {
        /*
         * Reported by `EditorPane`'s rejection arm, which raises a notice — a *silent* failed
         * autosave is the worst outcome available here, and this `catch` exists only so the
         * rejection is not an unhandled one inside an update listener.
         *
         * And nothing is re-armed. A refused write — a full disk, a read-only mount, a file
         * that moved under the buffer — would otherwise retry every sixty seconds for the life
         * of the tab. The buffer stays dirty, so the next thing the user *types* arms it again,
         * which is the moment their attention is back on this file.
         */
      })
    }

    /**
     * Arm the two timers from a document change.
     *
     * A 60-second debounce and a five-minute ceiling from the moment the buffer went dirty. The
     * ceiling is not belt-and-braces: a restarting debounce is starved by continuous input, so
     * somebody typing steadily for twenty minutes would never autosave at all. `docSync.ts` and
     * `gitCountStore.ts` both carry the same note about the same trap.
     *
     * Reset by **document changes only** — not by scrolling, not by caret moves. "Inactive" in a
     * buffer means the text stopped changing; a person reading a dirty file and scrolling
     * through it would otherwise never get a save.
     */
    const arm = (view: EditorView): void => {
      const config = autosaveCb.current
      if (config === undefined || readOnly) return
      if (idleTimer !== undefined) clearTimeout(idleTimer)
      idleTimer = setTimeout(() => {
        idleTimer = undefined
        autosaveIf(view, 'idle')
      }, config.idleMs)
      // Armed once per dirty *episode*, not per keystroke — restarting it here is precisely the
      // starvation the ceiling exists to prevent.
      if (ceilingTimer === undefined) {
        ceilingTimer = setTimeout(() => {
          ceilingTimer = undefined
          autosaveIf(view, 'idle')
        }, config.ceilingMs)
      }
    }

    const shared: Extension[] = [
      /*
       * **First, before `lineNumbers()`, and that position is the feature.** (M18)
       *
       * CodeMirror lays gutters out in extension order — `activeGutters` is a facet and a facet's
       * inputs keep the order of the extensions that supplied them — so this line is what puts the
       * annotation column to the *left* of the numbers, where IDEA puts it. Moved below
       * `lineNumbers()` it still works and is simply in the wrong place, which is the kind of
       * regression a reader cannot see in a diff; `check:blame` pins it.
       */
      blameSlot.of(blameOnRef.current === true ? blameExt : []),
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightActiveLine(),
      history(),
      drawSelection(),
      dropCursor(),
      rectangularSelection(),
      crosshairCursor(),
      EditorState.allowMultipleSelections.of(true),
      /*
       * Ctrl+hover and Ctrl+click. (M14)
       *
       * Both halves of one gesture, and they live together in `ctrlLink.ts` rather than here —
       * the underline is a promise about what the click will do, and two handlers in two files
       * would each hold their own idea of which word is under the pointer. The Ctrl+click
       * `mousedown` that used to sit at this line moved there unchanged, and
       * `clickAddsSelectionRange` moved with it, because a facet override that frees Ctrl is part
       * of the gesture rather than part of the surface.
       *
       * `identity` and not `path`: `ctrlLink` caches its answers in a module-level map keyed by
       * that argument and shared across every mounted editor, so a second buffer over one path
       * would answer this one's Ctrl+hover from the other's cache. See the prop.
       */
      ctrlLink(project, identity),
      syntaxHighlighting(cideHighlightStyle),
      findExtensions(),
      minimap(),
      /*
       * Per-file view memory's producer. Beside `minimap()` because it watches the same thing
       * for the same reason — a scroll is not reliably a `ViewUpdate` — and the two carry the
       * same note.
       *
       * Both destinations in one place: the ref that survives a reload of *this* buffer, and
       * the callback the pane debounces into Rust. Two subscribers on the update listener would
       * be two things to keep in step, and the ref is the one that must never be skipped.
       *
       * Keyed on `identity`, which is what the observation is stamped with and therefore what the
       * restore below compares against. It also decides what a host would *store*: a revision
       * buffer's scroll must not be written into the working file's remembered position, which is
       * what keying this on the real path would do the first time a pane wired `onView` to one.
       */
      viewTracker(identity, (seen) => {
        observedRef.current = seen
        viewCb.current?.(seen)
      }),
      /*
       * The mouse, on the Back stack. (M16)
       *
       * Beside `viewTracker` because it is the same shape — a `ViewPlugin` watching the same
       * update stream, with every decision in an import-free module a check script runs — and
       * beside `ctrlLink(project, identity)` because it takes the same two arguments for the same
       * reason. The whole rule is `navHistory.ts::recordsClick`; what this line buys is that a
       * click more than half a viewport away, or into another file, is a place Back can return
       * from. Without it the pointer was the one input device that moved the caret and wrote
       * nothing, so Back was empty for anyone who navigates with a mouse.
       *
       * Deliberately **not** folded into the update listener below: it must see the caret slot
       * before that listener moves it, and it must not pay for a keystroke. `navRecorder.ts`
       * writes both reasons out.
       *
       * `identity`, and it has to be the *same* key `claimCaret` takes further down: `originOf`
       * decides whether a click came from another document by comparing `focusedCaret().path`
       * against this argument, so two spellings of one buffer would make every click in it look
       * like a cross-file jump and put an entry on the Back stack that returns to itself.
       */
      navRecorder(project, identity),
      indentUnit.of('    '),
      EditorState.tabSize.of(4),
      languageSlot.of([]),
      lintSlot.of([]),
      // `Prec` is not needed here: this keymap is added before `defaultKeymap`, and
      // CodeMirror runs same-precedence keymaps in order, so Mod-s is claimed before
      // anything else can look at it.
      keymap.of([
        { key: 'Mod-s', run: save, preventDefault: true },
        /*
         * ⌥⏎ — send the selection, or the file when there is none.
         *
         * Always `true`, even when there is nothing to send to. Returning `false` would let the
         * chord fall through to a keymap that does not want it and leave the user with a
         * keystroke that did nothing, which is the failure being fixed; `send` reports its own
         * refusal through `Failures`. Unbound in `defaultKeymap`, `historyKeymap` and
         * `closeBracketsKeymap`, and unbound in the app keymap (`cide-core::keymap`), so
         * nothing is being taken from anyone — checked, not assumed.
         */
        {
          key: 'Alt-Enter',
          run: (target) => {
            sendCb.current.send(target, path)
            return true
          },
          preventDefault: true,
        },
        /*
         * Move line up / down, re-homed — the compensation `cide-core::keymap` has claimed since
         * M12 and that nobody had built. (M16)
         *
         * `alt+up`/`alt+down` are `navigate.prevMember`/`navigate.nextMember` in the app keymap,
         * and the key gate is a window **capture** listener, so `defaultKeymap`'s
         * `Alt-ArrowUp`/`Alt-ArrowDown` never see the event: move-line was simply gone from every
         * buffer, with no replacement anywhere — not in the palette, not in the Code menu. The
         * comment beside those two bindings said this file re-homed them to `Mod-Shift-Arrow`,
         * `grep` found nothing, and that comment is now true.
         *
         * # Why the chord is spelled twice
         *
         * `Mod-Shift-Arrow` is free on Linux and Windows and is **not** free on macOS, which is
         * the half the old comment got wrong: `standardKeymap` carries `{ mac: 'Cmd-ArrowUp',
         * shift: selectDocStart }`, so ⌘⇧↑/⌘⇧↓ are select-to-top-of-file and select-to-bottom
         * there. This block is added before `defaultKeymap` and therefore claims first, so
         * spelling it `Mod-` would take those two away silently. `Mod-Alt-Shift-Arrow` is free in
         * both layers — checked in `check-editor.mjs` by expanding the composed keymap the way
         * CodeMirror expands it, `shift:` sub-bindings and per-platform `mac:` spellings included,
         * rather than by reading the documentation.
         *
         * IDEA spells this ⌥⇧↑ on both platforms; that is `Shift-Alt-ArrowUp`, which is
         * `copyLineUp` here — trading one capability for another is not a re-homing, so it loses.
         */
        { key: 'Mod-Shift-ArrowUp', mac: 'Mod-Alt-Shift-ArrowUp', run: moveLineUp },
        { key: 'Mod-Shift-ArrowDown', mac: 'Mod-Alt-Shift-ArrowDown', run: moveLineDown },
        /*
         * Ctrl+D — duplicate line or selection. Shared with `DiffPane` and `MergePane`; the
         * binding and the whole argument for it are in `editorKeys.ts`.
         *
         * **No `Prec.high` needed, and that is a fact about `find.ts` rather than about this
         * array.** `findExtensions()` is added *earlier* in `shared`, so its keymap is offered
         * the stroke first — and `searchKeymap` used to bind `Mod-d` to `selectNextOccurrence`,
         * which returns `true` whenever the caret is in a word. An entry added here without
         * touching that one would simply never have fired. `searchBindings` filters it out and
         * re-homes it to `Alt-j`, which is what leaves `Mod-d` free for this line.
         */
        ...lineEditKeymap,
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...historyKeymap,
        indentWithTab,
      ]),
      EditorView.updateListener.of((update) => {
        if (update.selectionSet || update.docChanged) {
          // Typing is a claim on the slot, not only clicking into the pane: an editor that
          // mounts in a fresh split takes the readout when it appears, and without this the
          // bar would keep reporting that new pane's `Ln 1, Col 1` while the user carries on
          // typing over here. `focus` costs one array read when the slot is already held.
          //
          // `hasFocus` is `root.activeElement == contentDOM`, and **that the read-only branch
          // below makes `.cm-content` focusable at all is load-bearing here**, not a detail of
          // that branch. Without the `tabindex` it adds, this test is permanently false in every
          // library and toolchain buffer, so such an editor claims both slots once on mount and
          // can never take them back — and Ctrl+G, Ctrl+F12, ⌥F7, Ctrl+B and Back then all act
          // on whichever *editable* file the user touched last.
          if (update.view.hasFocus) {
            readout?.focus()
            caret?.focus()
          }
          readout?.set(readoutFor(update.state))
          /*
           * A plain assignment into a module-level object — no React, no store, no listener.
           * This runs on every selection change, which under a held arrow key is thirty times a
           * second, and it sits directly beside the readout write that already goes to that
           * length to avoid a re-render. `caretTrack` is a *read* surface: nothing subscribes.
           */
          {
            const head = update.state.selection.main.head
            const at = update.state.doc.lineAt(head)
            const column = head - at.from + 1
            // `doc.lines` is a field on the rope, not a walk — the same integer the readout's
            // `Ln x, Col y` is already computed beside, so Go to line's "past the end of this
            // file" note costs nothing on the per-keystroke path.
            caret?.set(at.number, column, update.state.doc.lines)
            /*
             * `src/main.rs › impl Parser › parse`, handed the caret this branch already computed.
             *
             * The rule itself is `publishTrail` above, and it is up there rather than inline for
             * a reason this listener demonstrates: the other two things that move the trail — a
             * root and an outline that arrive late — cannot reach a closure defined inside the
             * build effect, and while it lived here they simply did not move it.
             */
            publishTrail(at.number, column)
          }
          // Read from `update.state`, not from a captured view: this listener outlives
          // several states and the one that changed is the one to report.
          const { from, to } = update.state.selection.main
          selectionCb.current?.({
            text: update.state.sliceDoc(from, to),
            startLine: update.state.doc.lineAt(from).number,
            endLine: update.state.doc.lineAt(to).number,
          })
        }
        if (update.docChanged) {
          // The read is deferred, not the notification: `viewRef` is what makes "as it is then"
          // rather than "as it was when this fired" possible.
          docChangedCb.current?.(() => viewRef.current?.state.doc.toString() ?? '')
        }
        if (update.docChanged && baseline !== null) {
          // `Text.eq` compares lengths and line counts first, so the common case — a typed
          // character, which changes the length — costs two integer comparisons rather than
          // a walk of a five-megabyte rope.
          setDirty(!update.state.doc.eq(baseline))
          /*
           * Arm or disarm the autosave timers from what the buffer *became*.
           *
           * Inside the `docChanged` branch, so scrolling and caret moves cost nothing — and
           * after `setDirty`, so `dirtyRef` describes this transaction. A change that made the
           * buffer clean again (an undo back to the last save) disarms: leaving a timer running
           * over a clean buffer would fire a save that the policy refuses, which is harmless and
           * is still a timer nobody needed.
           */
          if (dirtyRef.current) arm(update.view)
          else disarm()
        }
        if (update.focusChanged && update.view.hasFocus) {
          caret?.focus()
          // Before the callback, so the bar follows a click into a pane even when the click
          // lands on the caret's own position and no selection change follows it.
          readout?.focus()
          focusCb.current?.()
        } else if (update.focusChanged) {
          /*
           * The losing edge — the `else` this listener never had. (M15)
           *
           * One hook covers both halves of "the user left this file", which is why it is the
           * right signal and a `window.addEventListener('blur')` is not:
           *
           *   * `view.hasFocus` is `document.hasFocus() && root.activeElement === contentDOM`,
           *     so `observers.blur` fires for a click into another pane, into the file tree,
           *     into a terminal's textarea, into a tab-strip button — **and** for the OS window
           *     being deactivated, which is IDEA's frame-deactivation save arriving free.
           *   * A **tab switch is a blur**: hidden tabs are `visibility: hidden`, never
           *     unmounted (`TabContent.tsx` says so, and says the browser drops focus when a
           *     focused element goes hidden). So clicking another tab reaches here rather than
           *     unmounting anything.
           *
           * A window-level listener would double-fire against this path and would be a per-pane
           * subscription to a window-scoped fact.
           */
          autosaveIf(update.view, 'blur')
        }
      }),
    ]

    if (!oversize) {
      shared.push(
        EditorView.lineWrapping,
        bracketMatching(),
        closeBrackets(),
        indentOnInput(),
        /*
         * Code folding. (M19)
         *
         * # Why it is here and not beside `lineNumbers()`
         *
         * Two reasons, and the first is the ordering it *keeps*: this array is appended to
         * `shared`, so the fold gutter still lands after `lineNumbers()` — and gutters are laid
         * out in extension order, which is what puts the chevrons *between* the numbers and the
         * text where IDEA has them. `blame.ts` carries the mirror image of that note for why its
         * column goes first. `check:editor` pins the ordering the way `check:blame` pins the
         * other one.
         *
         * The second is the gate itself. Above `HIGHLIGHT_LIMIT_BYTES` this buffer gets no
         * grammar, no bracket matching and no wrapping, because a megabyte of generated output is
         * opened to look at rather than worked in. A fold scan is linear and cheap next to any of
         * those, but it is the same judgement about the same population, and putting it under the
         * same flag means there is one answer to "what does an oversize buffer do" rather than
         * two. `scanFolds` has its own `FOLD_LINE_LIMIT` besides, for the file that is enormous
         * without being large.
         *
         * `foldSpecFor` is synchronous and total — it answers for a `.txt` too, see
         * `DEFAULT_FOLD_SPEC` — and it has to be: the fold restore below runs inside the mount
         * dispatch, and a spec arriving with the grammar's dynamic `import()` would land a tick
         * after it.
         */
        foldExtensions(foldSpecFor(path)),
      )
    }
    if (readOnly) {
      shared.push(
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
        /*
         * **The line that makes a read-only buffer answer the keyboard at all.** (M16)
         *
         * Reported as "Ctrl+F does nothing in a std-library file"; the find bar was never the
         * problem. `EditorView.editable.of(false)` gives `.cm-content` `contenteditable="false"`
         * (`@codemirror/view`, `updateAttrs`), and a `contenteditable="false"` div with no
         * `tabindex` **is not focusable**. CodeMirror registers every DOM handler — `keydown`
         * included — on `view.contentDOM` (`InputState.ensureHandlers`), and events bubble *up*,
         * so a keystroke delivered anywhere else never reaches it. Clicking such a buffer focuses
         * `.cm-scroller` instead, which is contentDOM's parent and has `tabIndex = -1` of its
         * own, and `focusPreventScroll(view.contentDOM)` in CodeMirror's `mousedown` — and every
         * `view.focus()` in this codebase, including `registerReveal`'s below and `ctrlLink`'s —
         * is a no-op against an unfocusable element.
         *
         * So it was not Ctrl+F that was dead. **The entire editor keymap was dead** in every
         * library and toolchain buffer: `Mod-f`, `F3`/`Shift-F3`, `Mod-d`, `Mod-Shift-l`,
         * `Escape`, `Alt-Enter` (*send lines to Claude*), and the whole of `defaultKeymap` — the
         * arrow keys, Home/End, PageUp/PageDown, `Mod-a`. It read as alive because `.cm-scroller`
         * is the focused overflowing element, so those keys still *scrolled* the pane natively.
         * What was actually missing was the caret: the base theme hides `.cm-cursor` and unhides
         * it only under `&.cm-focused`, and `view.hasFocus` is `root.activeElement ==
         * contentDOM`, so it was permanently false.
         *
         * That same permanently-false `hasFocus` is the second, quieter half. The update listener
         * above re-claims the status readout and `caretTrack`'s slot **only** on
         * `update.view.hasFocus`, so such a buffer claimed both on mount and could never take
         * them back. Click an editable file and then click back into a std file, and Ctrl+G,
         * Ctrl+F12, ⌥F7, Ctrl+B and Back all acted on the *other* file, while the status bar's
         * trail, language and `Ln x, Col y` went on naming it.
         *
         * # Why this and not the two alternatives
         *
         * Dropping `editable.of(false)` and keeping only `EditorState.readOnly` also works —
         * CodeMirror ignores DOM changes under that facet — and it makes `.cm-content` a *root
         * editable element* again, which is exactly the thing `codeMenu.tsx`'s `restoreFocus`
         * note relies on read-only buffers not being, and it re-enables IME and a native caret in
         * a document that cannot change.
         *
         * Binding `ctrl+f` in `cide-core::keymap` fights a written rule:
         * `nothing_binds_the_find_bars_f_keys` keeps the find bar's chords out of the Rust keymap
         * because the window capture gate would swallow them in the buffer and in the find field
         * at once. It would also have fixed one chord and left the other twenty dead.
         *
         * `contentAttributes` merges over the computed attrs — `updateAttrs` builds the defaults
         * and then folds this facet in on top — so `contenteditable="false"` survives untouched
         * and only `tabindex` is added.
         *
         * `'0'` rather than `'-1'`, and the reason is *not* that `-1` would fail to work: an
         * element with `tabindex="-1"` takes focus from a click and from `.focus()` just as well,
         * so every chord above would come back either way. It is that `-1` would leave the two
         * halves disagreeing. `@codemirror/view`'s `updateSelection` computes
         * `!focused && !(editable || dom.tabIndex > -1)` to decide whether to write a *pointer*
         * selection back into a `.cm-content` it believes can never hold focus — and the DOM
         * reports `tabIndex === -1` both for `tabindex="-1"` and for no attribute at all. So `-1`
         * would make the element focusable while leaving the library on the branch it keeps for
         * unfocusable content: half-fixed, and in the half that is hard to see. `'0'` says the
         * same thing to both, and it is what the editable case already is — a `contenteditable`
         * element is a tab stop without being given one.
         *
         * `indentWithTab` refuses under `readOnly`, so Tab still falls through to the browser and
         * moves focus out of the buffer, which is correct.
         */
        EditorView.contentAttributes.of({ tabindex: '0' }),
      )
    }

    let view: EditorView
    try {
      view = new EditorView({ doc: source, extensions: shared, parent: host })
    } catch (error) {
      // A third-party constructor over arbitrary file contents in an app with no error
      // boundary: an exception escaping here unmounts the React root and blanks the window,
      // taking every terminal in it. A pane that says nothing is recoverable; that is not.
      console.error('[cide] the editor failed to build', error)
      return
    }
    baseline = view.state.doc
    viewRef.current = view
    lintSlotRef.current = lintSlot
    blameSlotRef.current = blameSlot
    /*
     * The markers the column was already showing, pushed into the buffer that replaced it.
     *
     * The compartment above only decides whether the *gutter* is there; its contents live in a
     * state field, which a freshly built view creates empty. Without this a reload of an annotated
     * file — the agent editing it, *Reload from disk* — would come back with an empty column and
     * no event to fill it, because the effect below is keyed on props that did not change.
     */
    if (blameOnRef.current === true) {
      view.dispatch({ effects: setBlame.of(blameRef.current ?? []) })
    }
    // Hand the awaitable save outward, so a close confirmation can offer *Save and close*.
    // Cleared in the cleanup below: a handle to a destroyed view would write from a buffer
    // that is no longer on screen.
    saveHandleCb.current?.(() => saveNow(view))

    /*
     * And the way to scroll it, for the markdown preview. (M20)
     *
     * `EditorView.scrollIntoView` on the line's *start*, with `y: 'start'`, rather than writing
     * `scrollDOM.scrollTop`: the height map is measured from the top of the document,
     * `.cm-content` carries its own padding and a wrapped line is taller than one row, so a
     * pixel offset computed out here would be off by all three. `viewTracker.ts` reads this
     * geometry the same way round, through `posAtCoords`, and for the same reason.
     *
     * Clamped, because the caller's line came from a *rendering* of the document and the
     * document may have changed since it was rendered. An out-of-range position throws out of
     * `dispatch`, and there is no error boundary between here and the React root.
     */
    scrollHandleCb.current?.((line) => {
      const doc = view.state.doc
      const clamped = Math.min(Math.max(1, Math.trunc(line)), doc.lines)
      try {
        view.dispatch({
          effects: EditorView.scrollIntoView(doc.line(clamped).from, { y: 'start' }),
        })
      } catch (error) {
        console.error('[cide] could not scroll the buffer to line', clamped, error)
      }
    })

    // Claimed after the view is built and released in the cleanup below, so the bar's line
    // and the buffer on screen have exactly the same lifetime. A view that failed to
    // construct returned above and never claims one.
    // `path` and not the trail: the trail may be root-relative, and `chrome/StatusBar.tsx` has to
    // turn a crumb back into an absolute path to reveal it.
    /*
     * And `path` rather than `identity`, which is the opposite call from `claimCaret` five
     * statements down. The two are opposite on purpose, and the line between them is that the bar
     * **describes** the buffer while the caret slot **addresses** it. Three reasons, in the order
     * that decides it:
     *
     * * **This is not a registry.** `statusReadout.ts` keeps claims in a stack and identifies one
     *   by the claim object — two panes can show one file and both hold a claim — so there is no
     *   slot here for a second document to collide with and nothing to make unique.
     * * **The bar answers *what am I looking at*.** For a revision buffer the honest answer is
     *   `src › log.rs`; the pane's own crumb strip is what says which commit, right above it. The
     *   workaround this prop replaced had to pass `a1b2c3d:src/log.rs` as the path, and a trail
     *   reading `a1b2c3d:src › log.rs` was written up as its cost. Passing the identity here would
     *   keep that cost while pretending to have removed it.
     * * **`file` is consumed as a path.** `rowPaths::crumbTargets` slices this string into
     *   ancestor directories and tests each against the reveal roots, so anything that is not a
     *   path produces nonsense crumbs. A revision pane's path is repo-relative, so no reveal root
     *   contains it and every crumb still answers `null` — the trail draws inert, which is the
     *   truth about where those crumbs lead, and it now reads like the file it is showing.
     */
    // `onScreenRef` and not `onScreen`,
    // because this effect is keyed on `[path, reloadKey]` and adding the flag to that list would
    // rebuild the whole `EditorView` — scrollback, undo history and unsaved edits — on every tab
    // switch. The flag is only read at this instant; the effect further down handles it moving.
    readout = claimStatusReadout(
      path,
      segments,
      readoutFor(view.state),
      onScreenRef.current,
    )
    slotRef.current = readout
    // The claim carries the path alone, so a file whose outline has *already* been parsed — a tab
    // reopened, a second pane over the same buffer — gets its `impl Parser › parse` tail in the
    // first frame rather than on the user's first caret move.
    publishedRef.current = segments
    rebuildTrail()
    // Claimed and released with the readout, and for the same reason: the two answer the same
    // question — *which editor is the user in* — and a caret slot outliving its buffer would
    // send Ctrl+F12 to a file that is no longer on screen.
    /*
     * The word reader is what lets ⌥F7 name the identifier it is searching for.
     *
     * `keys/dispatch.ts` has a caret and no `EditorView` — the predicament `caretTrack` exists for
     * — so without this the popup would be headed *"Usages of the symbol"* and the empty-result
     * notice would say *"No usages found"* about nothing in particular.
     *
     * A **getter**, so nothing is computed until the chord fires: putting the word on the claim
     * beside the line and column would extract one per selection change, which under a held arrow
     * key is thirty a second on the one path this file goes out of its way to keep cheap. And it is
     * `wordTargetAt` — the same normaliser Ctrl+click uses — so the keyboard and the mouse cannot
     * disagree about where a word starts and ends.
     *
     * # `identity`, against the readout's `path` — the pair that had to be decided separately
     *
     * Everything that reads this slot turns the path back into a **target**, which is the whole
     * difference from the bar above:
     *
     * * `overlays/GoToLine.tsx` reads `focusedCaret().path` and hands it straight to
     *   `requestReveal`. Under the real path, Ctrl+G inside a revision buffer would move the caret
     *   in the *working* file whenever a tab for it happens to be open, and park a request that
     *   ambushes the next one to open when it is not — the same delivery bug this prop exists to
     *   close, arriving from the other end.
     * * `navRecorder.originOf` compares this against its own key; see `navRecorder(project,
     *   identity)` above, which is only right if this line agrees with it.
     * * ⌥F7 and Ctrl+⌥B send it to a language server reading the file **as it is now**. An
     *   identity is not a path there, so the request fails and says so — which is the same refusal
     *   `RevisionPane` already chooses by withholding `project`, and the alternative is a real
     *   path carrying a line number from a forty-commit-old buffer, which is an answer that is
     *   confidently wrong rather than absent.
     *
     * The cost is that a revision buffer contributes no *useful* path to those three, and that is
     * correct: there is no place in the working tree that means "line 40 as it was at `a1b2c3d`".
     */
    /**
     * Run one folding command against whatever view is live, or answer that none is.
     *
     * `viewRef.current` and not the `view` this effect built: the two are the same object for
     * the whole of its life, and differ for exactly the window in which the effect's cleanup has
     * run and a chord is still in flight — a Ctrl+Minus pressed as the tab closes. Dispatching
     * into a destroyed `EditorView` throws, and it would throw *out of the key gate*, which is a
     * window-level capture listener with no boundary above it.
     */
    const onLiveView = (run: (target: EditorView) => boolean): boolean => {
      const live = viewRef.current
      return live !== null && run(live)
    }

    caret = claimCaret(
      identity,
      () => {
        const live = viewRef.current
        if (live === null) return null
        return wordTargetAt(live, live.state.selection.main.head)?.text ?? null
      },
      /*
       * The folding commands, on the same claim. (M19)
       *
       * `keys/dispatch.ts` fires `editor.fold` with no `EditorView` and no way to get one — the
       * predicament this whole slot exists for — and the question "which editor should collapse"
       * is the question the slot already answers. `caretTrack.ts` writes out why this is not a
       * second registry.
       *
       * Every one reads `viewRef.current` rather than closing over `view`: a claim outlives no
       * view, but a chord that arrives during the teardown of one would otherwise dispatch into
       * a destroyed instance, which throws out of the key gate.
       */
      {
        fold: () => onLiveView(foldHere),
        unfold: () => onLiveView(unfoldHere),
        toggle: () => onLiveView(toggleFoldHere),
        foldAll: () => onLiveView(foldAllRanges),
        unfoldAll: () => onLiveView(unfoldAllRanges),
        foldRecursively: () => onLiveView(foldRecursive),
        unfoldRecursively: () => onLiveView(unfoldRecursive),
      },
    )
    /*
     * Seed the real numbers immediately, because nothing else will until the user types.
     *
     * `claimCaret` opens at `{line: 1, column: 1, lines: 1}` and the only writer is the update
     * listener's `selectionSet || docChanged` branch — and constructing a view produces no
     * update at all. So a file opened and never touched reported *one line*, and Go to line,
     * whose only feedback is that count, told the user "Past the end — this file has 1 line"
     * about a nine-hundred-line file. The jump itself was right, which is what made it read as
     * the popup lying rather than as a bug.
     *
     * After the restore below would be wrong: this runs before it, so the line count is correct
     * from the first frame and the restore's own selection dispatch updates the position.
     */
    caret.set(
      view.state.doc.lineAt(view.state.selection.main.head).number,
      view.state.selection.main.head - view.state.doc.lineAt(view.state.selection.main.head).from + 1,
      view.state.doc.lines,
    )

    /*
     * Put the user back where they were.
     *
     * # Where the position comes from, in order
     *
     * `observedRef` first — this component's own last observation, which is the only source
     * that is current on a reload from disk (see the ref's comment). The `at` prop second,
     * which is what Rust remembered from a previous session or a previously closed tab.
     *
     * # Why this is a dispatch and not `EditorState.create({ selection })`
     *
     * The selection could go into the initial state, and the scroll could not: `scrollIntoView`
     * is an *effect*, and effects need a view to be dispatched into. Doing half of it one way
     * and half the other would be two mechanisms for one restore. One dispatch also means one
     * undo-history entry boundary and one `update`, which is what the tracker above sees.
     *
     * `scrollIntoView: true` is deliberately **not** set on this transaction. That flag scrolls
     * the *selection* into view, minimally, and would fight the explicit `y: 'start'` effect —
     * the same collision `planReveal`'s `center` branch documents below.
     *
     * # Why it is before `registerReveal`, and why that is not left to line order
     *
     * An explicit navigation outranks a remembered position: Go to definition into a file the
     * user had scrolled must land on the definition, not where they were last week. Running the
     * restore first and the reveal second gets that for free — but "for free" here means "until
     * someone swaps two statements", so the rule is also *stated* and enforced, in
     * `planRestore`, which refuses outright while a request is parked for this path.
     */
    {
      // Both questions ask about `identity` rather than `path`, because both are about *this
      // document*: `viewTracker` stamped the observation with the identity, and the veto has to
      // name the key `registerReveal` is about to register under — checking one name and
      // registering another would let a parked reveal be missed here and then delivered a line
      // later, which is precisely the collision `planRestore` refuses to guess at.
      const remembered =
        observedRef.current?.path === identity ? observedRef.current : (atRef.current ?? null)
      const plan = planRestore(
        remembered,
        view.state.doc.lines,
        pendingReveals().includes(identity),
      )
      if (plan !== null) {
        try {
          const target = view.state.doc.line(plan.line)
          // `Math.min` against `line.to`: `clampView` bounds the *line*, and a column past the
          // end of a line that has since been shortened would still be past the end of the
          // document's idea of that line. Same rule as `revealRange`.
          const anchor = Math.min(target.from + plan.column - 1, target.to)
          view.dispatch({
            selection: EditorSelection.cursor(anchor),
            /*
             * `yMargin: 0`, and it is not cosmetic — it is the fix for a **cumulative** drift.
             *
             * `scrollIntoView`'s default margin is 5px, so `y: 'start'` puts the recorded line
             * five pixels *below* the top of the viewport and the tracker's hit test at the top
             * pixel lands on the line above. Measured, not reasoned about: seeding
             * `positions.json` with `topLine: 200`, launching and reading the file back gave
             * 199 — and it would have given 198 the launch after that, creeping a line per
             * relaunch. `firstFullyVisible` covers the same class of error from the other side.
             */
            /*
             * The folds go in **this** transaction, ahead of the scroll. (M19)
             *
             * Not a second dispatch, and the order inside the array is not the reason — a
             * transaction's effects all apply to one new state, and `scrollIntoView` is measured
             * against it. Two dispatches would be the bug: the first lays the document out at
             * its *unfolded* heights and scrolls line `topLine` to the top, the second collapses
             * several thousand lines above it, and the user lands somewhere they have never
             * been. Once per restore, on every file they had folded.
             *
             * `foldEffectsFor` drops a remembered line that no longer names a foldable range,
             * which is the ordinary case after the file changed underneath — see its note on why
             * this stores lines rather than offsets.
             */
            effects: [
              ...foldEffectsFor(view.state, plan.folds),
              EditorView.scrollIntoView(view.state.doc.line(plan.topLine).from, {
                y: 'start',
                yMargin: 0,
              }),
            ],
          })
        } catch (error) {
          // Wrapped for the reason `registerReveal`'s handler is: this runs inside an effect
          // with no error boundary above it, and an exception escaping here unmounts the React
          // root and takes every terminal in the window with it. `planRestore` clamps, so this
          // is for what the clamp cannot foresee.
          console.error('[cide] could not restore the last view of this file', error)
        }
      }
    }

    /*
     * "Open this file at this line", from a click in the search results.
     *
     * Registered here rather than in `EditorPane` because the request is answered by a
     * `dispatch` into *this* view, and the view only exists inside this effect. A request
     * made while nothing was mounted is parked and spent by this call — see
     * `revealRequest.ts`, which is where the whole of that reasoning lives.
     *
     * The editor is focused **only when the request asks for it**, and the default is not to.
     * A single click on a result opens the file (`clickSemantics.ts`), and pulling focus out of
     * the results list on every click would end the ArrowDown/Enter walk the panel supports
     * after exactly one hit. The selection and the active-line tint are both painted while
     * unfocused — see the `.cm-activeLine` note in `EditorSurface.module.css` — so the place is
     * shown without taking the keyboard.
     *
     * That reasoning is right for a click and was silently wrong for everything else. Go to
     * line, the File Structure popup and Go to symbol all accept with `closeOverlay()`, which
     * unmounts the card whose `<input>` held focus — so `activeElement` falls to `<body>`, this
     * handler moved the caret without claiming it, and the `updateListener` above only re-takes
     * the readout on `update.view.hasFocus`, which is false. The caret moved and the keyboard
     * did not follow it. `RevealTarget.focus` is where that decision now lives, on the request
     * rather than here, because only the caller knows which of the two gestures it is.
     *
     * The clamp inside `revealRange` is what keeps this from throwing; the guard is for what
     * it cannot foresee. This runs inside the sidebar's click handler, which has no error
     * boundary over it, so an exception escaping here unmounts the React root and takes every
     * terminal in the window with it. Missing the line is recoverable; that is not.
     *
     * # `identity`, and this is the registration the prop was added for
     *
     * `requestReveal` delivers to live receivers and only parks when there are none, so a buffer
     * that is *not* the working file must not be registered under the working file's path. It
     * would answer a search-result click meant for the real file — moving the caret in a read-only
     * view of a commit, parking nothing, and leaving the file tab that opens a moment later at
     * line 1 with no request left to spend. Nothing else in this repository can see that failure,
     * which is why `check:editor` asserts this argument by name.
     */
    const stopReveal = registerReveal(identity, (target) => {
      try {
        // Every decision is `planReveal`'s, so this handler holds only the two things a headless
        // check could not run anyway: the dispatch and the focus call.
        const plan = planReveal(view.state.doc, target)
        view.dispatch({
          selection: EditorSelection.create([plan.range]),
          /*
           * `scrollIntoView: true` is CodeMirror's minimal scroll; `'center'` is an explicit
           * effect. They are alternatives rather than additions — passing both would queue two
           * scrolls for one dispatch, and the minimal one runs second and undoes the centring.
           */
          ...(plan.center
            ? { effects: EditorView.scrollIntoView(plan.range.from, { y: 'center' }) }
            : { scrollIntoView: true }),
        })
        // After the dispatch, not before: `focus()` scrolls the caret into view on its own in
        // some browsers, and doing it first would fight the alignment chosen above.
        if (plan.focus && !view.hasFocus) view.focus()
      } catch (error) {
        console.error('[cide] the editor could not reveal that position', error)
      }
    })

    // Fire-and-forget, and guarded on the view still being the live one: a tab closed while
    // its grammar chunk is in flight would otherwise dispatch into a destroyed editor.
    if (!oversize) {
      void loadLanguage(path).then((extension) => {
        if (extension === null || viewRef.current !== view) return
        view.dispatch({ effects: languageSlot.reconfigure(extension) })
      })
    }

    return () => {
      viewRef.current = null
      lintSlotRef.current = null
      blameSlotRef.current = null
      saveHandleCb.current?.(null)
      scrollHandleCb.current?.(null)
      /*
       * **The single most important line in this feature.**
       *
       * A `reloadKey` bump rebuilds the view, and a destroyed CodeMirror view still answers
       * `view.state.doc`. An orphaned autosave timer would therefore write the buffer as it
       * stood before the reload straight over the file that replaced it — silently, a minute
       * later, with the correct contents already on screen. That is the whole reason the timers
       * are `let`s inside this effect rather than entries in a module-level map.
       */
      disarm()
      // Hands the bar back to whichever editor is under this one, and blanks it when there
      // is none. A slot left behind would keep a closed file's position on screen.
      readout?.release()
      caret?.release()
      if (slotRef.current === readout) slotRef.current = null
      // Before `destroy`, so a request racing the unmount cannot dispatch into a dead view.
      stopReveal()
      view.destroy()
    }
    // Rebuilt only on a different file or an explicit reload. `doc` is intentionally absent:
    // see `reloadKey`. `readOnly` is absent because it only ever arrives with a new file. And
    // `identity` is absent although the registrations above are keyed on it: it is a pure function
    // of the path for every caller, so the two move together, and the prop's doc comment states
    // that as a contract rather than paying for it with an entry in the one array in this file
    // whose every addition has cost somebody their unsaved edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, reloadKey])

  /*
   * The trail against a root, or an outline, that arrived late.
   *
   * Two effects and not one, because the two facts change independently and React would
   * otherwise re-run the work for whichever of them did not move.
   *
   * **The root.** The claim above is taken once per buffer and carries the trail as it stood
   * then, which is wrong exactly once: `PaneBody` passes `roots[0]?.path ?? PROJECT_ROOT`, so an
   * editor restored before its project record lands computes an absolute path and, without this,
   * would keep showing one until the tab is reopened.
   *
   * `rebuildTrail()` rather than the `setTrail(segments)` this used to be, which was a second
   * bug hiding inside the fix for the first: it published the **path alone**, so a root arriving
   * while `impl Parser › parse` was on the bar truncated the tail the user was reading.
   */
  useEffect(() => {
    rebuildTrail()
  }, [segments, rebuildTrail])

  /*
   * **The outline**, which is not a race but the ordinary case.
   *
   * `OutlineFeed` parses on a worker and publishes into `outlineStore` some time after the tab
   * opens, so a freshly opened file has a path and no symbols for a moment. Nothing recomputed
   * on that: the symbol half was appended only by the update listener, so the tail stayed missing
   * until the user happened to move the caret — which reads exactly like the feature being
   * unfinished, and is why it is listed here rather than left to the next selection change.
   *
   * `symbols` and not `symbolsRef`: this is the one place the identity of that prop is wanted.
   * `EditorPane` reads it through `useSyncExternalStore` over a store that returns a shared
   * frozen constant for the empty case, so an unchanged outline is an unchanged reference and
   * this does not run per render.
   */
  useEffect(() => {
    rebuildTrail()
  }, [symbols, rebuildTrail])

  /*
   * This tab came forward, so the bar follows it. (M16)
   *
   * The missing half of the claim stack. `TabContent` never unmounts an inactive tab and
   * `file_open` is open-*or-activate*, so switching to an already-open file produces no mount,
   * no `EditorView`, and no DOM focus — nothing the stack was watching. The bar kept naming the
   * previous file until the user clicked into the buffer, which is precisely the report.
   *
   * `focus()` and not a fresh claim: this editor already holds one, and `focus` is a splice.
   * There is deliberately **no** `else` demoting it — clicking into a terminal tab leaves the
   * last buffer's trail standing, which `chrome/StatusBar.tsx` states as intended behaviour and
   * which a demotion here would quietly change into a blank slot.
   */
  useEffect(() => {
    if (onScreen) slotRef.current?.focus()
  }, [onScreen])

  /*
   * Push the diagnostics, and configure the gutter.
   *
   * Two dispatches rather than one, because they answer different questions and change at wildly
   * different rates: the gutter's presence follows a setting the user changes by hand, the
   * squiggles follow an analyser that republishes on every save.
   *
   * `setDiagnostics` — not `linter()`. `linter()` is a *pull* source CodeMirror polls on a timer;
   * ours are pushed from Rust the moment a server publishes, and a poll on top of a push is a
   * second clock to keep in step for no gain.
   *
   * The clear-on-`none` is load-bearing: reconfiguring the compartment removes the *gutter* but
   * leaves whatever `setDiagnostics` last installed underlining the text, so turning highlighting
   * off would drop the marks in the margin and keep the squiggles.
   */
  useEffect(() => {
    const view = viewRef.current
    const slot = lintSlotRef.current
    if (view === null || slot === null) return
    const on = highlight !== 'none'
    view.dispatch({ effects: slot.reconfigure(on ? [lintGutter()] : []) })
    const ranges = on ? lintRanges(diagnostics ?? [], view.state.doc) : []
    /*
     * Wrapped, for the reason `registerReveal`'s handler is: this runs inside an effect with no
     * error boundary above it, and a `dispatch` that throws unmounts the React root and takes
     * every terminal in the window with it. `lintMap` clamps every position it produces, so this
     * is for what it cannot foresee — a CodeMirror invariant we have not read.
     */
    try {
      view.dispatch(setDiagnostics(view.state, ranges))
    } catch (error) {
      console.error('[cide] could not paint diagnostics', error)
    }
  }, [diagnostics, highlight])

  /*
   * Put the blame column up, and keep it filled. (M18)
   *
   * Beside the diagnostics effect and shaped exactly like it, because the two answer the same pair
   * of questions: *is this gutter here at all* (a compartment) and *what is in it* (a state
   * field). Two dispatches, not one, and in this order.
   *
   * **The clear when `blameOn` is false is load-bearing**, and it is the same trap the lint effect
   * above records: reconfiguring the compartment away removes the *column* and leaves the field's
   * contents standing behind it, so the hover card would outlive the gutter it hung off — a card
   * about a commit, floating over a buffer with no annotation on it and no way to dismiss it. The
   * lint version of this bug drops the margin marks and keeps the squiggles; this one is worse,
   * because a tooltip takes the pointer.
   *
   * `blame`/`blameOn` appear here and **nowhere near** the build effect's `[path, reloadKey]`.
   */
  useEffect(() => {
    const view = viewRef.current
    const slot = blameSlotRef.current
    if (view === null || slot === null) return
    view.dispatch({ effects: slot.reconfigure(blameOn ? blameExt : []) })
    /*
     * Wrapped for the reason the diagnostics dispatch is: this runs inside an effect with no error
     * boundary above it, and an exception out of `dispatch` unmounts the React root and takes every
     * terminal in the window with it. `build` in `blame.ts` clips a marker past the end of the
     * document, so this is for what it cannot foresee.
     */
    try {
      view.dispatch({ effects: setBlame.of(blameOn ? (blame ?? []) : null) })
    } catch (error) {
      console.error('[cide] could not paint the blame column', error)
    }
  }, [blame, blameOn, blameExt])

  /*
   * No breadcrumb bar. `crates › cide-core › src › lib.rs · Rust · UTF-8 · LF · Ln 7, Col 48`
   * is one line in the status bar now (`statusReadout.ts` → `chrome/StatusBar.tsx`), and the
   * 28px row it used to need goes to the buffer — in every pane of a split, which is where
   * the row was costing the most for saying the least: the tab above it already names the
   * file, badge and all, with the full path on hover.
   */
  return (
    <div className={styles.pane}>
      {/*
        * `data-native-menu="false"` even though the surface would already be suppressed: the
        * host is a `contenteditable`, which is the one shape `wantsNativeMenu` treats as a text
        * entry by default, and this pane now has its own Cut/Copy/Paste. Stating it means the
        * answer does not change if someone flips `nativeInTextInputs` from `App.tsx`.
        *
        * `{menu}` has to be rendered or nothing appears; it portals out of this
        * `overflow: hidden` pane on its own.
        */}
      <div
        className={styles.body}
        ref={hostRef}
        onContextMenu={onContextMenu}
        data-native-menu="false"
      />
      {menu}
    </div>
  )
}
