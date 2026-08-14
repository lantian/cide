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
import { useEffect, useMemo, useRef, type ReactNode } from 'react'
import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete'
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
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
import { findExtensions } from './find'
import { minimap } from './minimap'
import { languageName, loadLanguage } from './languages'
import { captureLineEndings, restoreLineEndings, type DocumentEndings } from './lineEndings'
import { exceedsBytes } from './byteSize'
import { pendingReveals, planReveal, registerReveal } from './revealRequest'
import { planRestore, type FileView } from './position'
import { viewTracker } from './viewTracker'
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
  onSave?: ((text: string) => void | Promise<void>) | undefined
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
}

/** `Ln 128, Col 24`, one-based in both, which is what every editor and every stack trace uses. */
export function cursorLabel(state: EditorState): string {
  const head = state.selection.main.head
  const line = state.doc.lineAt(head)
  return `Ln ${line.number}, Col ${head - line.from + 1}`
}

export function EditorSurface({
  path,
  root,
  project,
  doc,
  reloadKey = 0,
  readOnly = false,
  onDirtyChange,
  onSave,
  onFocus,
  onSelection,
  onSaveHandle,
  onDocChanged,
  symbols,
  diagnostics,
  highlight = 'all',
  at,
  onView,
}: EditorSurfaceProps): ReactNode {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const viewRef = useRef<EditorView | null>(null)
  /** The live view's lint compartment, so the push effect can reconfigure it. */
  const lintSlotRef = useRef<Compartment | null>(null)
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
  const docChangedCb = useRef(onDocChanged)
  docChangedCb.current = onDocChanged
  const viewCb = useRef(onView)
  viewCb.current = onView
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
   * Guarded on the path so a *different* file does not inherit this one's line number.
   */
  const observedRef = useRef<FileView | null>(null)

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
    // The whole trail as last published, so the listener can compare before touching the DOM.
    // `sameTrail` does the same job one layer down; this avoids even building the array.
    let publishedTrail: readonly string[] = segments
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
    const saveNow = (view: EditorView): Promise<void> => {
      if (readOnly) return Promise.resolve()
      const saving = view.state.doc
      const text = restoreLineEndings(saving.toString(), endingRef.current)
      return Promise.resolve(saveCb.current?.(text)).then(() => {
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
      void Promise.resolve(saveCb.current?.(text)).then(
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

    const shared: Extension[] = [
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
       */
      ctrlLink(project, path),
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
       */
      viewTracker(path, (seen) => {
        observedRef.current = seen
        viewCb.current?.(seen)
      }),
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
             * `src/main.rs › impl Parser › parse`.
             *
             * The path half changes when the user switches file — rare; the symbol half changes
             * when the caret crosses a member boundary, which is far rarer than a caret move.
             * So this runs on every selection change and publishes on almost none of them, which
             * is what keeps it off the same budget as the `Ln 7, Col 48` readout beside it (that
             * one is written straight into a DOM node for exactly this reason).
             */
            const next = [...segments, ...trailNames(symbolsRef.current, at.number, column)]
            if (!sameTrail(publishedTrail, next)) {
              publishedTrail = next
              readout?.setTrail(next)
            }
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
        }
        if (update.focusChanged && update.view.hasFocus) {
          caret?.focus()
          // Before the callback, so the bar follows a click into a pane even when the click
          // lands on the caret's own position and no selection change follows it.
          readout?.focus()
          focusCb.current?.()
        }
      }),
    ]

    if (!oversize) {
      shared.push(
        EditorView.lineWrapping,
        bracketMatching(),
        closeBrackets(),
        indentOnInput(),
      )
    }
    if (readOnly) {
      shared.push(EditorState.readOnly.of(true), EditorView.editable.of(false))
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
    // Hand the awaitable save outward, so a close confirmation can offer *Save and close*.
    // Cleared in the cleanup below: a handle to a destroyed view would write from a buffer
    // that is no longer on screen.
    saveHandleCb.current?.(() => saveNow(view))

    // Claimed after the view is built and released in the cleanup below, so the bar's line
    // and the buffer on screen have exactly the same lifetime. A view that failed to
    // construct returned above and never claims one.
    readout = claimStatusReadout(segments, readoutFor(view.state))
    slotRef.current = readout
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
     */
    caret = claimCaret(path, () => {
      const live = viewRef.current
      if (live === null) return null
      return wordTargetAt(live, live.state.selection.main.head)?.text ?? null
    })
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
      const remembered =
        observedRef.current?.path === path ? observedRef.current : (atRef.current ?? null)
      const plan = planRestore(remembered, view.state.doc.lines, pendingReveals().includes(path))
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
            effects: EditorView.scrollIntoView(view.state.doc.line(plan.topLine).from, {
              y: 'start',
              yMargin: 0,
            }),
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
     */
    const stopReveal = registerReveal(path, (target) => {
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
      saveHandleCb.current?.(null)
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
    // see `reloadKey`. `readOnly` is absent because it only ever arrives with a new file.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, reloadKey])

  /*
   * The trail against a root that arrived late.
   *
   * The claim above is taken once per buffer and carries the trail as it stood then, which
   * is wrong exactly once: `PaneBody` passes `roots[0]?.path ?? PROJECT_ROOT`, so an editor
   * restored before its project record lands computes an absolute path and, without this,
   * would keep showing one until the tab is reopened. Runs after the mount effect on the
   * first pass and is a no-op there — `setTrail` compares before it publishes.
   */
  useEffect(() => {
    slotRef.current?.setTrail(segments)
  }, [segments])

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
