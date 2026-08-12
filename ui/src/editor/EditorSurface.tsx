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
import { registerReveal, revealRange } from './revealRequest'
import { claimStatusReadout, formatReadout, pathTrail, type ReadoutSlot } from './statusReadout'
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
  doc,
  reloadKey = 0,
  readOnly = false,
  onDirtyChange,
  onSave,
  onFocus,
  onSelection,
  onSaveHandle,
}: EditorSurfaceProps): ReactNode {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const viewRef = useRef<EditorView | null>(null)
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

  /*
   * The right-click menu. Reads the view through a getter rather than being handed it, because
   * `items` runs at open time and the view alive then is the one to act on — a captured
   * `viewRef.current` from render time is a destroyed editor after a reload.
   */
  const { onContextMenu, menu } = useCodeMenu({
    view: () => viewRef.current,
    path,
    readOnly,
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
    let baseline: EditorState['doc'] | null = null
    /*
     * The status bar's line, claimed once the view exists further down — the update
     * listener is built before the editor it listens to, so this is a `let` rather than a
     * parameter. Every use is optional-chained: nothing dispatches into a view during
     * construction, and a listener that fired before the claim would be reporting a position
     * in a buffer the user cannot see yet.
     */
    let readout: ReadoutSlot | null = null
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
      syntaxHighlighting(cideHighlightStyle),
      findExtensions(),
      minimap(),
      indentUnit.of('    '),
      EditorState.tabSize.of(4),
      languageSlot.of([]),
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
          if (update.view.hasFocus) readout?.focus()
          readout?.set(readoutFor(update.state))
          // Read from `update.state`, not from a captured view: this listener outlives
          // several states and the one that changed is the one to report.
          const { from, to } = update.state.selection.main
          selectionCb.current?.({
            text: update.state.sliceDoc(from, to),
            startLine: update.state.doc.lineAt(from).number,
            endLine: update.state.doc.lineAt(to).number,
          })
        }
        if (update.docChanged && baseline !== null) {
          // `Text.eq` compares lengths and line counts first, so the common case — a typed
          // character, which changes the length — costs two integer comparisons rather than
          // a walk of a five-megabyte rope.
          setDirty(!update.state.doc.eq(baseline))
        }
        if (update.focusChanged && update.view.hasFocus) {
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
    // Hand the awaitable save outward, so a close confirmation can offer *Save and close*.
    // Cleared in the cleanup below: a handle to a destroyed view would write from a buffer
    // that is no longer on screen.
    saveHandleCb.current?.(() => saveNow(view))

    // Claimed after the view is built and released in the cleanup below, so the bar's line
    // and the buffer on screen have exactly the same lifetime. A view that failed to
    // construct returned above and never claims one.
    readout = claimStatusReadout(segments, readoutFor(view.state))
    slotRef.current = readout

    /*
     * "Open this file at this line", from a click in the search results.
     *
     * Registered here rather than in `EditorPane` because the request is answered by a
     * `dispatch` into *this* view, and the view only exists inside this effect. A request
     * made while nothing was mounted is parked and spent by this call — see
     * `revealRequest.ts`, which is where the whole of that reasoning lives.
     *
     * The editor is deliberately **not** focused. A single click on a result opens the file
     * (`clickSemantics.ts`), and pulling focus out of the results list on every click would
     * end the ArrowDown/Enter walk the panel supports after exactly one hit. The selection
     * and the active-line tint are both painted while unfocused — see the `.cm-activeLine`
     * note in `EditorSurface.module.css` — so the place is shown without taking the keyboard.
     *
     * The clamp inside `revealRange` is what keeps this from throwing; the guard is for what
     * it cannot foresee. This runs inside the sidebar's click handler, which has no error
     * boundary over it, so an exception escaping here unmounts the React root and takes every
     * terminal in the window with it. Missing the line is recoverable; that is not.
     */
    const stopReveal = registerReveal(path, (target) => {
      try {
        view.dispatch({
          selection: EditorSelection.create([revealRange(view.state.doc, target)]),
          scrollIntoView: true,
        })
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
      saveHandleCb.current?.(null)
      // Hands the bar back to whichever editor is under this one, and blanks it when there
      // is none. A slot left behind would keep a closed file's position on screen.
      readout?.release()
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
