/**
 * The native diff view: what the user sees instead of a wall of ASCII in the terminal when
 * Claude proposes an edit, and what they answer with.
 *
 * Three outcomes leave this pane, and they are not three names for the same thing. Accept
 * sends the *current buffer*, which is why the right-hand side is editable at all — the
 * protocol carries the user's final text back to the agent, so an edit made here is an edit
 * the agent is told about. Accept as proposed sends nothing but consent, which is the
 * cheaper answer when the buffer is untouched. Reject sends refusal.
 *
 * Nothing here talks to IPC. The pane takes callbacks and plain strings so a fixture can
 * render it, and so the broker id it echoes back is the caller's concern rather than this
 * component's.
 */
import { useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react'
import { MergeView, unifiedMergeView } from '@codemirror/merge'
import { history, historyKeymap } from '@codemirror/commands'
import { EditorState } from '@codemirror/state'
import { EditorView, keymap, lineNumbers } from '@codemirror/view'
import {
  getDiffView,
  getServerDiffView,
  setDiffView,
  subscribeDiffView,
} from '@/editor/diffViewMode'
import { lineEditKeymap } from '@/editor/editorKeys'
import type { DiffView } from '@/ipc/client'
import styles from './DiffPane.module.css'

export interface DiffPaneProps {
  requestId: string
  oldPath: string
  newPath: string
  original: string
  proposed: string
  onAccept: (finalContents: string) => void
  onAcceptAsProposed: () => void
  onReject: () => void
}

/**
 * Where the inline diff gives up, measured across both sides together.
 *
 * `@codemirror/merge` forces `height: auto !important` on each side's `.cm-scroller` so the
 * two documents can be aligned row for row. That takes the scroller out of the scrolling
 * role, and with it CodeMirror's viewport windowing: the editor's viewport becomes the whole
 * document, so every line of both files is realised as a DOM element before the first frame.
 * The diff itself is bounded — `presentableDiff` falls back to a coarse algorithm past its
 * `scanLimit` — but the layout is not.
 *
 * Twelve thousand lines is roughly six thousand a side, which lays out without a visible
 * stall; a minified bundle or a lockfile is an order of magnitude past it and would freeze
 * the window. The character budget catches the other shape of the same problem — few lines,
 * each enormous — where line count alone says the file is small.
 */
const MAX_DIFF_LINES = 12_000
const MAX_DIFF_CHARS = 1024 * 1024

function countLines(text: string): number {
  // `split` allocates the whole array for a file that may be a megabyte; counting is enough.
  let n = 1
  for (let i = text.indexOf('\n'); i !== -1; i = text.indexOf('\n', i + 1)) n++
  return n
}

function tooLargeToDiff(original: string, proposed: string): boolean {
  if (original.length + proposed.length > MAX_DIFF_CHARS) return true
  return countLines(original) + countLines(proposed) > MAX_DIFF_LINES
}

/**
 * Whether every line break in the text is a CRLF.
 *
 * `EditorState.create` splits an incoming document on `/\r\n?|\n/` and `Text.toString()`
 * rejoins it with `\n`, so a CRLF file handed to the editable side comes back LF-only. The
 * accept path writes that text to disk, so without restoring the endings a one-line edit to
 * a Windows file rewrites every line of it. "Every break, or none" is the deliberate test:
 * a file with mixed endings is left exactly as CodeMirror returned it rather than being
 * silently unified in a direction nobody asked for.
 */
function crlfThroughout(text: string): boolean {
  const breaks = countLines(text) - 1
  if (breaks === 0) return false
  let crlf = 0
  for (let i = text.indexOf('\r\n'); i !== -1; i = text.indexOf('\r\n', i + 2)) crlf++
  // A bare CR is a line break to CodeMirror as well, so one anywhere means the endings are
  // mixed however the CRLFs count up, and the text goes back exactly as the editor gave it.
  return crlf === breaks && !/\r(?!\n)/.test(text)
}

/**
 * How many lines differ, without diffing.
 *
 * A real diff is the expensive thing the size guard exists to avoid, so this counts by
 * multiset instead: a line that appears three times in the original and once in the proposal
 * contributes two removals. That ignores order, so a pure reordering reads as no change —
 * acceptable for a figure whose only job is to say how much is at stake, and honest in the
 * direction that matters, since it never overstates.
 */
function roughChangedLines(original: string, proposed: string): number {
  const counts = new Map<string, number>()
  for (const line of original.split('\n')) counts.set(line, (counts.get(line) ?? 0) + 1)

  let added = 0
  for (const line of proposed.split('\n')) {
    const seen = counts.get(line) ?? 0
    if (seen > 0) counts.set(line, seen - 1)
    else added++
  }

  let removed = 0
  for (const n of counts.values()) removed += n
  return added + removed
}

/**
 * Everything about a request that is derived from its documents, computed once.
 *
 * `changed` is only populated on the oversize path, because the multiset walk below is the
 * one thing this pane does that scales with a file it has already decided is too big to
 * touch — doing it per render would reintroduce the stall the size guard exists to prevent.
 */
interface Frozen {
  requestId: string
  original: string
  proposed: string
  oversize: boolean
  crlf: boolean
  changed: number
}

function freeze(requestId: string, original: string, proposed: string): Frozen {
  const oversize = tooLargeToDiff(original, proposed)
  return {
    requestId,
    original,
    proposed,
    oversize,
    crlf: crlfThroughout(proposed),
    changed: oversize ? roughChangedLines(original, proposed) : 0,
  }
}

/** `a/b/c.rs → a/b/d.rs` when the edit moves the file, and one path when it does not. */
function pathLabel(oldPath: string, newPath: string): ReactNode {
  if (oldPath === newPath || newPath.length === 0) {
    return <span className={styles.path}>{oldPath}</span>
  }
  return (
    <>
      <span className={styles.path}>{oldPath}</span>
      <span className={styles.arrow}>→</span>
      <span className={styles.path}>{newPath}</span>
    </>
  )
}

export function DiffPane({
  requestId,
  oldPath,
  newPath,
  original,
  proposed,
  onAccept,
  onAcceptAsProposed,
  onReject,
}: DiffPaneProps): ReactNode {
  const hostRef = useRef<HTMLDivElement | null>(null)
  /**
   * The editor whose buffer Accept sends.
   *
   * Two shapes now — `MergeView.b` in the split layout, a plain `EditorView` in the unified
   * one — and this holds the *editable* one either way. That is deliberately the narrowest
   * thing the rest of the component needs: `currentBuffer()` wants a document, not a merge
   * view, and keeping the union out of it is what stops the two layouts diverging in what
   * Accept means.
   */
  const bufferRef = useRef<EditorView | null>(null)
  const destroyRef = useRef<(() => void) | null>(null)

  /**
   * The proposal as it stood when the last editor was torn down, and the request it belongs to.
   *
   * Rebuilding the editor for a layout change would otherwise re-seed it from `docs.proposed`
   * and silently drop whatever the user had typed — on the one pane in the app whose Accept
   * writes a file, and while `openDiff` is blocking the agent's turn. It is not only the
   * deliberate toggle that lands here: `diffView` is a *shared, stored* preference, so a second
   * window or the Settings screen flipping it rebuilds every mounted diff pane, including ones
   * the user is not looking at.
   *
   * Keyed by `requestId` because it must never leak across requests: a stale carry would answer
   * a new diff with the previous one's text, which is the exact mismatch `freeze` exists to
   * prevent. A different id falls back to `docs.proposed`.
   */
  const carryRef = useRef<{ requestId: string; text: string } | null>(null)

  /*
   * Split or unified, from `Settings.editor.diffView` — the same stored preference the git
   * diff pane reads, because a user who chose a layout chose it for diffs, not for one pane.
   *
   * **Note what that changed for this pane.** It was unconditionally a `MergeView`, i.e. split;
   * `EditorSettings::default()` is `DiffView::Unified`, so a user who has never touched the
   * toggle now opens Claude's diffs unified. That is the deliberate cost of one setting for
   * both panes rather than two — the git pane's long-standing default is unified and the git
   * pane is the one that has to survive a 400px detached tab — and the toggle in the header
   * puts it back permanently in one click. It is written down because the shape of the change
   * is easy to miss: nothing in this file names a default.
   */
  const diffView = useSyncExternalStore(subscribeDiffView, getDiffView, getServerDiffView)

  // Callbacks held in refs so a parent that re-creates its handlers on every render cannot
  // reach the effect below and tear down the editor — which would discard whatever the user
  // has typed into the proposal.
  const acceptCb = useRef(onAccept)
  acceptCb.current = onAccept
  const acceptAsIsCb = useRef(onAcceptAsProposed)
  acceptAsIsCb.current = onAcceptAsProposed
  const rejectCb = useRef(onReject)
  rejectCb.current = onReject

  // The two documents are frozen for the life of a request. A diff request is immutable —
  // the file on disk and the agent's proposal are both fixed by the time the tool call
  // arrives — so a changed `original` or `proposed` under an unchanged `requestId` is prop
  // churn, and honouring it would rebuild the editor underneath a half-typed edit.
  //
  // A changed `requestId` is a different question and re-derives everything. The caller is
  // still expected to key the pane on `requestId`, but not keying it now costs a remount
  // rather than a mismatch: without this the editor would be rebuilt on the new id holding
  // the *previous* request's text, and Accept would send one file's contents as the answer
  // to a diff opened on another.
  const [docs, setDocs] = useState(() => freeze(requestId, original, proposed))
  const [failed, setFailed] = useState(false)
  if (docs.requestId !== requestId) {
    setDocs(freeze(requestId, original, proposed))
    setFailed(false)
  }

  useEffect(() => {
    if (docs.oversize) return
    const host = hostRef.current
    if (host === null) return

    /*
     * Line numbers, wrapping — and an undo history, which this pane shipped without.
     *
     * # Why the omission mattered here more than anywhere else
     *
     * This is the one editable surface in the app whose Accept **writes a file**, and it does it
     * while `openDiff` is blocking the agent's turn. A user tidying the proposal before accepting
     * it could delete a block, reach for Ctrl+Z, and get nothing back: `history()` is a
     * `StateField`, and without it in the state `undo` has no transactions to walk and returns
     * `false`. Nothing in the app was swallowing the chord — `cide-core::keymap` binds no `z`, so
     * the window capture gate passes it straight through (see
     * `the_editor_undo_chords_are_not_bound_here` over there) — the editor simply had nowhere to
     * put it. `EditorSurface` has had `history()` since M9; this pane never got it.
     *
     * `historyKeymap` alone, not `defaultKeymap`. The rest of `defaultKeymap` would be a much
     * larger behaviour change to a pane that has been getting its Enter, Backspace and arrow
     * handling from CodeMirror's own `beforeinput`/DOM-observation path since it shipped, and
     * changing that while fixing undo would put two unrelated risks on one line. It carries
     * Mod-z, Mod-y, Ctrl-Shift-z (Linux), Mod-u and Alt-u, which is the whole of the gesture.
     *
     * Shared with the read-only `a` side deliberately: that side adds `EditorState.readOnly`, and
     * `undo`/`redo` check exactly that facet and refuse, so one array is safe for both.
     *
     * What this still cannot do is survive `carryRef` (see below). A layout toggle rebuilds the
     * editor — `MergeView` and `unifiedMergeView` are different editors, not a reconfigure — and
     * the carry hands the replacement the *text*, not the history. Undo after switching Split ↔
     * Unified therefore starts from the carried document. That is a real limit and is written
     * down rather than left for a reader to assume otherwise; carrying the history would mean
     * serialising a `StateField` across two different extension sets, which CodeMirror offers no
     * way to do.
     */
    /*
     * Ctrl+D rides in the same array, and for the same reason `historyKeymap` does: `copyLine`
     * guards on `state.readOnly` and returns `false`, so the read-only `a` side refuses it
     * without a second array. See `editor/editorKeys.ts` for the binding and the chord trade.
     */
    const shared = [
      lineNumbers(),
      EditorView.lineWrapping,
      history(),
      keymap.of([...historyKeymap, ...lineEditKeymap]),
    ]
    // Long unchanged stretches collapse to a clickable band. Without this a one-line change in
    // a thousand-line file opens on a screen of identical context.
    const collapseUnchanged = { margin: 3, minSize: 4 }

    /*
     * The proposal to open with: what the user last had on screen for *this* request, or the
     * agent's original proposal the first time. See `carryRef`.
     *
     * Not cleared here but after the build succeeds: if the constructor throws, the pane
     * degrades to the plain summary and `currentBuffer()` becomes the only route the text has
     * back to the agent, so the carry has to survive to be read there.
     */
    const carried = carryRef.current
    const startDoc =
      carried !== null && carried.requestId === docs.requestId ? carried.text : docs.proposed

    try {
      if (diffView.view === 'unified') {
        /*
         * One editor holding the proposal, with the original shown as deleted blocks above the
         * lines that replaced them.
         *
         * The per-chunk gesture survives the change of layout, which is the whole condition on
         * offering this at all. `unifiedMergeView`'s "reject" reverts the chunk to the original
         * — exactly what `revertControls: 'a-to-b'` does in the split layout — and its "accept"
         * drops the chunk's markers without touching the text.
         *
         * They are **relabelled**, and that is not cosmetic. This pane's footer already has
         * buttons called Accept and Reject, and those answer the agent: they end the tool call
         * and unblock the conversation. A per-chunk control with the same two words would be
         * two very different gestures spelled identically, on the one screen where getting it
         * wrong writes a file. "Keep original" and "Keep change" say what the chunk buttons do
         * and cannot be read as an answer.
         */
        const view = new EditorView({
          doc: startDoc,
          parent: host,
          extensions: [
            ...shared,
            unifiedMergeView({
              original: docs.original,
              highlightChanges: true,
              gutter: true,
              collapseUnchanged,
              mergeControls: (type, action) => {
                const el = document.createElement('button')
                el.type = 'button'
                el.className = styles.chunkButton ?? ''
                el.textContent = type === 'reject' ? 'Keep original' : 'Keep change'
                el.title =
                  type === 'reject'
                    ? 'Put the original text back in this chunk'
                    : 'Leave this chunk as proposed and stop marking it'
                el.addEventListener('click', action)
                return el
              },
            }),
          ],
        })
        bufferRef.current = view
        destroyRef.current = () => view.destroy()
      } else {
        const view = new MergeView({
          // A: the file as it stands. Read-only in both senses — `readOnly` stops commands and
          // `editable` stops the DOM being contenteditable at all, so a click does not place a
          // caret in a document that cannot be changed.
          a: {
            doc: docs.original,
            extensions: [...shared, EditorState.readOnly.of(true), EditorView.editable.of(false)],
          },
          b: { doc: startDoc, extensions: shared },
          parent: host,
          // The per-chunk control copies a chunk from A into B: "put the original back here".
          // There is no per-chunk accept in a side-by-side view, because B already holds the
          // proposal — accepting a chunk would be a no-op, and rejecting one is the gesture the
          // user actually needs.
          revertControls: 'a-to-b',
          highlightChanges: true,
          gutter: true,
          collapseUnchanged,
        })
        bufferRef.current = view.b
        destroyRef.current = () => view.destroy()
      }
    } catch (e) {
      // A third-party constructor run over arbitrary file contents, in an app with no error
      // boundary: an exception escaping here unmounts the whole React root, which blanks the
      // window and takes the three controls with it — and `openDiff` blocks the agent's
      // turn, so the session then waits on an answer no one can give. Degrading to the same
      // plain summary the oversize path uses keeps all three answers reachable.
      console.error('[cide] diff view failed to build', e)
      setFailed(true)
      return
    }

    // The new editor now holds it, so the carry has done its job.
    carryRef.current = null

    return () => {
      // Save the proposal before the editor goes, so a layout change is a re-layout and not a
      // discard. `docs.requestId` is this effect's own — on a request change the new effect
      // sees a mismatched carry and starts from the new proposal instead.
      const live = bufferRef.current
      if (live !== null) carryRef.current = { requestId: docs.requestId, text: live.state.doc.toString() }
      bufferRef.current = null
      destroyRef.current?.()
      destroyRef.current = null
    }
    /*
     * Keyed on the request *and the layout*. `docs` is re-derived from `requestId` above and
     * the callbacks live in refs, so nothing else can legitimately rebuild the editor.
     *
     * A layout change does rebuild the editor — the two layouts are different extensions over
     * different editors and there is no `reconfigure` from one to the other — but it does not
     * cost the user's edits: `carryRef` hands the live document to the replacement. That is not
     * a nicety. The preference is shared and stored, so the rebuild is also triggered by a
     * second window or the Settings screen, on panes nobody is looking at, and "the user chose
     * this so they can live with it" would not be true of those.
     */
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestId, diffView.view])

  /**
   * What Accept sends.
   *
   * The proposal is only the starting point: the buffer is what the user has in front of
   * them, and it is what the protocol writes to disk.
   *
   * With no live editor there are two different cases and they do not share an answer.
   * `oversize` never built one, so there are no edits to have made *and the text never went
   * through CodeMirror* — it is returned untouched. A *failed rebuild* after a layout change
   * does have edits, held in `carryRef` because the build leaves it alone when it throws;
   * answering `docs.proposed` there would quietly send the agent text the user had changed.
   */
  const fromEditor = (text: string): string =>
    // Undo CodeMirror's line-ending normalisation, never impose one; see `crlfThroughout`.
    // Only ever applied to text that came *out* of an editor: `docs.proposed` may already hold
    // CRLFs, and running it through here would double every `\r`.
    docs.crlf ? text.replace(/\n/g, '\r\n') : text

  const currentBuffer = (): string => {
    const view = bufferRef.current
    if (view !== null) return fromEditor(view.state.doc.toString())
    const carried = carryRef.current
    if (carried !== null && carried.requestId === docs.requestId) return fromEditor(carried.text)
    return docs.proposed
  }

  // Two different reasons to show a sentence instead of a diff, one presentation.
  const plain = docs.oversize || failed

  return (
    <div className={styles.pane}>
      <header className={styles.header}>
        {pathLabel(oldPath, newPath)}
        {plain ? (
          <span className={styles.headerNote}>not shown inline</span>
        ) : (
          /*
           * The layout toggle is hidden on the plain path rather than disabled: there is no
           * diff on screen for it to rearrange, and an inert control beside a sentence
           * explaining that the file is too large to diff is noise, not information.
           */
          <div className={styles.layout} role="group" aria-label="Diff layout">
            {(['unified', 'split'] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                aria-pressed={diffView.view === mode}
                data-audit="claudeDiffLayout"
                disabled={!diffView.writable}
                title={
                  mode === 'split'
                    ? 'The file and the proposal beside each other.'
                    : 'One column, with the replaced lines shown above their replacements.'
                }
                className={
                  diffView.view === mode ? `${styles.mode} ${styles.modeOn}` : styles.mode
                }
                onClick={() => setDiffView(mode satisfies DiffView)}
              >
                {mode === 'split' ? 'Split' : 'Unified'}
              </button>
            ))}
          </div>
        )}
      </header>

      <div className={styles.body} ref={hostRef}>
        {plain ? (
          <div className={styles.oversize}>
            <div className={styles.oversizeCount}>
              {docs.oversize
                ? `${docs.changed.toLocaleString()} lines changed, too large to diff inline`
                : 'The diff could not be shown'}
            </div>
            <div className={styles.oversizeWhy}>
              {docs.oversize
                ? `The two sides come to more than ${MAX_DIFF_LINES.toLocaleString()} lines` +
                  ` or ${MAX_DIFF_CHARS / (1024 * 1024)} MiB together.`
                : 'The editor failed to build; the console has the reason.'}{' '}
              Accepting sends the proposal unchanged.
            </div>
          </div>
        ) : null}
      </div>

      <footer className={styles.actions}>
        <button
          type="button"
          className={`${styles.button} ${styles.reject}`}
          onClick={() => rejectCb.current()}
        >
          Reject
        </button>
        <button type="button" className={styles.button} onClick={() => acceptAsIsCb.current()}>
          Accept as proposed
        </button>
        <button
          type="button"
          className={`${styles.button} ${styles.accept}`}
          onClick={() => acceptCb.current(currentBuffer())}
        >
          Accept
        </button>
      </footer>
    </div>
  )
}
