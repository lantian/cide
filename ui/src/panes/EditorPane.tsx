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
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { EditorSurface } from '@/editor/EditorSurface'
import { diag, events, file as fileApi } from '@/ipc/client'
import styles from './EditorPane.module.css'

export interface EditorPaneProps {
  /** Absolute path of the file this tab shows. */
  path: string
  /** The project root, so the breadcrumb trail is repo-relative. */
  root?: string | undefined
  /** Set when this pane is inside a project; without it the dirty dot cannot be reported. */
  project?: string | undefined
  /** The file tab that owns this pane, for the same reason. */
  tab?: string | undefined
}

/** What the pane is currently showing instead of, or as well as, a buffer. */
type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; text: string; writable: boolean }
  | { kind: 'failed'; why: string }

export function EditorPane({ path, root, project, tab }: EditorPaneProps): ReactNode {
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [reloadKey, setReloadKey] = useState(0)
  /** Set when the file changed on disk while the buffer had unsaved edits. */
  const [conflict, setConflict] = useState(false)
  const dirtyRef = useRef(false)

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
      if (project === undefined || tab === undefined) return
      // Fire-and-forget: the dot is a hint, and a failed report must not interrupt typing.
      // The authoritative answer to "are there unsaved changes" is the buffer itself, which
      // is why nothing here waits for the round trip.
      void fileApi.setDirty(project, tab, dirty).catch(() => {})
    },
    [project, tab],
  )

  const read = useCallback(
    (bump: boolean) => {
      let cancelled = false
      void fileApi
        .read(path)
        .then((doc) => {
          if (cancelled) return
          setLoad({ kind: 'ready', text: doc.text, writable: doc.writable })
          setConflict(false)
          reportDirty(false)
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
    (text: string) =>
      fileApi.write(path, text).then(
        () => {
          setConflict(false)
        },
        (error: unknown) => {
          void diag.log(`could not save ${path}: ${String(error)}`)
          throw error
        },
      ),
    [path],
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
        <div className={styles.conflict} role="alert">
          <span className={styles.conflictText}>
            This file changed on disk while you had unsaved changes.
          </span>
          <button type="button" className={styles.conflictButton} onClick={() => read(true)}>
            Reload from disk
          </button>
          <button
            type="button"
            className={styles.conflictButton}
            onClick={() => setConflict(false)}
          >
            Keep mine
          </button>
        </div>
      )}
      <div className={styles.surface}>
        <EditorSurface
          path={path}
          root={root}
          doc={load.text}
          reloadKey={reloadKey}
          readOnly={!load.writable}
          onDirtyChange={reportDirty}
          onSave={onSave}
        />
      </div>
    </div>
  )
}
