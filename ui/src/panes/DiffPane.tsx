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
import { EditorState } from '@codemirror/state'
import { EditorView, lineNumbers } from '@codemirror/view'
import {
  getDiffView,
  getServerDiffView,
  setDiffView,
  subscribeDiffView,
} from '@/editor/diffViewMode'
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

  /*
   * Split or unified, from `Settings.editor.diffView` — the same stored preference the git
   * diff pane reads, because a user who chose a layout chose it for diffs, not for one pane.
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

    const shared = [lineNumbers(), EditorView.lineWrapping]
    // Long unchanged stretches collapse to a clickable band. Without this a one-line change in
    // a thousand-line file opens on a screen of identical context.
    const collapseUnchanged = { margin: 3, minSize: 4 }

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
          doc: docs.proposed,
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
          b: { doc: docs.proposed, extensions: shared },
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

    return () => {
      bufferRef.current = null
      destroyRef.current?.()
      destroyRef.current = null
    }
    /*
     * Keyed on the request *and the layout*. `docs` is re-derived from `requestId` above and
     * the callbacks live in refs, so nothing else can legitimately rebuild the editor.
     *
     * Rebuilding on a layout change costs whatever the user has typed into the proposal, which
     * is a real cost and is why it is not done for anything else. It is accepted here because
     * the two layouts are different extensions over different editors — there is no
     * `reconfigure` from one to the other — and because switching layout is a deliberate act on
     * a pane the user is looking at, unlike the prop churn the freeze above exists to ignore.
     */
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestId, diffView.view])

  /**
   * What Accept sends.
   *
   * The proposal is only the starting point: the buffer is what the user has in front of
   * them, and it is what the protocol writes to disk. Falling back to the proposal covers
   * both paths with no editor — oversize and failed — where there are no edits to have made.
   */
  const currentBuffer = (): string => {
    const view = bufferRef.current
    if (view === null) return docs.proposed
    const text = view.state.doc.toString()
    // Undo CodeMirror's line-ending normalisation, never impose one; see `crlfThroughout`.
    return docs.crlf ? text.replace(/\n/g, '\r\n') : text
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
