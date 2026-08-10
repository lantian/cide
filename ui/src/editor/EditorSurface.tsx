/**
 * The buffer, its breadcrumb bar and everything CodeMirror needs to be one of this app's
 * panes.
 *
 * Pure by design, in the same sense `panes/DiffPane.tsx` is: no IPC, no store, no knowledge
 * of tabs. Text comes in as a string and leaves through `onSave`, so the whole surface can
 * be driven from a fixture. `panes/EditorPane.tsx` is the piece that knows about files.
 *
 * Two things here are not the obvious implementation, and both are about not paying React
 * for something CodeMirror already does:
 *
 * * The cursor readout is written straight into the DOM from the update listener. Holding
 *   `Ln 128, Col 24` in React state re-renders this component on every caret move, which on
 *   a held arrow key is 30 renders a second, each one re-running the effect guards below.
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
import { Compartment, EditorState, type Extension } from '@codemirror/state'
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
import { findExtensions } from './find'
import { minimap } from './minimap'
import { languageName, loadLanguage } from './languages'
import { detectLineEnding, restoreLineEndings, type LineEnding } from './lineEndings'
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
 */
export const HIGHLIGHT_LIMIT_BYTES = 1024 * 1024

export interface EditorSurfaceProps {
  /** Absolute path, used for the breadcrumb trail and to pick the language. */
  path: string
  /**
   * The project root the trail is drawn relative to.
   *
   * Absent shows the whole absolute path, which is right for a file outside any project and
   * wrong-looking for one inside it — `crates › cide-core › src › lib.rs` is the mock, not
   * `home › lantian › work › cide › crates › …`.
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
}

/** The `crates › cide-core › src › lib.rs` trail. */
export function breadcrumbSegments(path: string, root?: string): string[] {
  let rest = path
  if (root !== undefined && root.length > 0) {
    const base = root.endsWith('/') ? root : `${root}/`
    if (path.startsWith(base)) rest = path.slice(base.length)
  }
  return rest.split(/[/\\]/).filter((s) => s.length > 0)
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
}: EditorSurfaceProps): ReactNode {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const readoutRef = useRef<HTMLDivElement | null>(null)
  const viewRef = useRef<EditorView | null>(null)

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

  const segments = useMemo(() => breadcrumbSegments(path, root), [path, root])
  const language = useMemo(() => languageName(path), [path])
  const lineEnding = useMemo(() => detectLineEnding(doc), [doc])

  // Captured per load, because after `EditorState.create` the buffer's endings are all `\n`
  // and the question can no longer be asked. See `lineEndings.ts`.
  const endingRef = useRef<LineEnding>('LF')
  const dirtyRef = useRef(false)

  useEffect(() => {
    const host = hostRef.current
    if (host === null) return

    const source = doc
    const oversize = source.length > HIGHLIGHT_LIMIT_BYTES
    // `lineEnding` is memoized on the same `doc` this effect captured, so reusing it here is
    // the same answer for one scan instead of two — and it keeps the readout and the bytes
    // that get written from ever disagreeing about what the file was.
    endingRef.current = lineEnding
    dirtyRef.current = false

    const languageSlot = new Compartment()
    let baseline: EditorState['doc'] | null = null

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
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...historyKeymap,
        indentWithTab,
      ]),
      EditorView.updateListener.of((update) => {
        if (update.selectionSet || update.docChanged) {
          const el = readoutRef.current
          if (el !== null) {
            el.textContent = `${language} · UTF-8 · ${lineEnding} · ${cursorLabel(update.state)}`
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
        if (update.docChanged && baseline !== null) {
          // `Text.eq` compares lengths and line counts first, so the common case — a typed
          // character, which changes the length — costs two integer comparisons rather than
          // a walk of a five-megabyte rope.
          setDirty(!update.state.doc.eq(baseline))
        }
        if (update.focusChanged && update.view.hasFocus) focusCb.current?.()
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

    const el = readoutRef.current
    if (el !== null) {
      el.textContent = `${language} · UTF-8 · ${lineEnding} · ${cursorLabel(view.state)}`
    }

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
      view.destroy()
    }
    // Rebuilt only on a different file or an explicit reload. `doc` is intentionally absent:
    // see `reloadKey`. `readOnly` is absent because it only ever arrives with a new file.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, reloadKey])

  return (
    <div className={styles.pane}>
      <div className={styles.breadcrumbs} data-audit="editorBreadcrumbs">
        <div className={styles.trail}>
          {segments.map((segment, i) => (
            // Keyed by position as well as text: a path can repeat a segment
            // (`src/cide/src`), and the text alone would collide.
            <span key={`${i}:${segment}`} className={styles.crumb}>
              {i > 0 && <span className={styles.separator}>›</span>}
              {segment}
            </span>
          ))}
        </div>
        <div className={styles.readout} ref={readoutRef} data-audit="editorReadout">
          {`${language} · UTF-8 · ${lineEnding} · Ln 1, Col 1`}
        </div>
      </div>

      <div className={styles.body} ref={hostRef} />
    </div>
  )
}
