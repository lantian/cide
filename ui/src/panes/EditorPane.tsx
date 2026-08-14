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
import { EditorSurface } from '@/editor/EditorSurface'
import { claude as claudeApi, diag, events, file as fileApi } from '@/ipc/client'
import { registerBuffer, unregisterBuffer } from '@/editor/openBuffers'
import {
  fetchOutline,
  forgetOutline,
  scheduleOutline,
  subscribeOutlines,
  symbolsOf,
} from '@/editor/outlineStore'
import { closeDoc, openDoc, resetDoc, savedDoc, scheduleDoc } from '@/editor/docSync'
import { levelFor, subscribeHighlightLevels } from '@/editor/highlightLevel'
import { useDiagnostics } from '@/sidebar/diagnosticsStore'
import { useWorkspace } from '@/store/workspace'
import { visible, type DiagnosticFilters } from '@/sidebar/ProblemsPanel/model'
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
  | { kind: 'ready'; text: string; writable: boolean }
  | { kind: 'failed'; why: string }

export function EditorPane({ path, root, project, tab }: EditorPaneProps): ReactNode {
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
          // After the write, not before: `didSave` makes rust-analyzer re-run `cargo check`, and
          // checking a file that is still mid-write is how you get a diagnostic for a truncated
          // buffer. This is also the notification the whole panel depends on — see `docSync.ts`.
          savedDoc(path)
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
        <OutlineFeed project={project} path={path} text={load.text} />
        <SyncFeed project={project} path={path} text={load.text} />
        <EditorSurface
          path={path}
          root={root}
          project={project}
          doc={load.text}
          reloadKey={reloadKey}
          readOnly={!load.writable}
          onDirtyChange={reportDirty}
          onSave={onSave}
          onSelection={reportSelection}
          onSaveHandle={registerSaveHandle}
          symbols={outline}
          onDocChanged={(read) => {
            // Debounced in the store, and the text is read when the timer fires — so the popup,
            // the breadcrumb and the member walk follow the buffer rather than the last save.
            if (project !== undefined) scheduleOutline(project as ProjectId, path, read)
            // The same reader on its own timer: this is what gives the language server the
            // *unsaved* text, which is the whole of what it has over `cargo check` in a terminal.
            scheduleDoc(path, read)
          }}
          diagnostics={diagnostics}
          highlight={level}
        />
      </div>
    </div>
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
