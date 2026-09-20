/**
 * A file tab's pane when the file is an Excalidraw drawing. (M63)
 *
 * > *"need implement support for .excalidraw, .excalidraw.json, .excalidraw.svg, and
 * > .excalidraw.png formats - when opening such file need to show an editor"*
 *
 * # What this replaced
 *
 * Two wrong answers, both correct for the question they were asked. `x.excalidraw` and
 * `x.excalidraw.json` opened in `EditorPane` as unhighlighted JSON — no language claims the
 * suffix — and `x.excalidraw.svg` / `x.excalidraw.png` opened in `ImagePane`, because
 * `imageKindFor` reads the last extension and the picture *is* a picture. Neither could be
 * drawn on.
 *
 * # The decisions worth knowing before editing this file
 *
 * **1. It is an ordinary `TabKind::File`, forked in `PaneBody` by name — before the image
 * fork.** `ImagePane`'s header carries the argument for keeping Rust out of it (a derived fact
 * frozen into `workspace.json`, a second extension table); what is new is the order, because
 * `x.excalidraw.png` would otherwise be a picture. Everything else about the tab is inherited:
 * the dirty dot and the close guard (`file.setDirty`), Ctrl+S (`registerBuffer`), Ctrl+Shift+T,
 * a rename moving the tab (`retarget_paths`), detach into a window.
 *
 * **2. The pane is static; the drawing engine is not.** `PaneBody` says why a *pane* must not
 * be lazy — a pane that renders nothing for a frame is measured at zero. This component is a
 * static import and paints its full-size root and *Opening …* on the first frame; the 2.7 MB
 * engine (`ExcalidrawSurface.tsx`, the one module that imports the package) is `import()`ed in
 * the load effect, in parallel with the file read, and no window pays for it until somebody
 * opens a drawing. The font-asset path below has to be set before that import can run, which
 * is why it is at module scope of *this* file.
 *
 * **3. Bytes cross the IPC as a framed raw body, never as JSON or base64.** A drawing is a PNG
 * as often as it is JSON, and `file.read` — the text road — refuses a NUL in the first 8 KiB,
 * correctly. So every format is read through `file.readBytes` and written through
 * `file.writeBytes`, which are `cide_ipc::frame` over Tauri's raw bodies: `image_read` explains
 * why the JSON and base64 roads are refused, and `file_read_bytes` why this one is the
 * exception that keeps the reason. The bytes decide how to parse (`sniffDrawing`); the name
 * decides what a save writes (`drawingKindFor`, read **at save time**, so a file renamed from
 * `.excalidraw` to `.excalidraw.png` is written as a PNG by its next save).
 *
 * **4. Dirty means the scene fingerprint moved.** Excalidraw's `onChange` fires for a
 * selection, a pan and a zoom, and a tab that goes dirty when you look around a drawing is a
 * close guard nobody trusts. `excalidrawKinds.ts` states what a save persists; the baseline is
 * the first `onChange` whose live elements are the ones the file held — not the first
 * `onChange` at all, which is the engine measuring its container with an empty scene, and not
 * the loaded scene itself, which the engine restores again on mount and may re-version. After a
 * save, the tab is clean only if the scene still matches the snapshot the save serialised: an
 * edit made during a slow PNG export must not be silently clean.
 *
 * **5. A reload is a remount, never `updateScene`.** `updateScene` keeps the undo stack, so
 * after *Reload from disk* a Ctrl+Z would resurrect the pre-reload scene — which autosave then
 * writes, the clobber the conflict bar exists to prevent. The surface is remounted under a new
 * `key` with the viewport carried across, and undo resets, exactly as `read(true)` rebuilds the
 * `EditorView`.
 *
 * **6. The absences, written down.** `handleKeyboardGlobally` and `autoFocus` stay false, for
 * the reasons `ExcalidrawSurface` gives. There is no in-pane `focusout` autosave: Excalidraw's
 * pickers are popovers portalled to `document.body`, so a `focusout` whose target is outside
 * this pane fires mid-gesture, and each save is a canvas export; idle and window deactivation
 * cover the intent. And there is no *Download* anywhere — the canvas actions that end in one
 * are hidden, because a wry webview drops `<a download>` and an inert button is the state this
 * codebase forbids.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { diag, events, file as fileApi, fileChanged, type ProjectId } from '@/ipc/client'
import type { FileStamp } from '@/ipc/generated'
import { useWorkspace } from '@/store/workspace'
import { autosaveDelay, shouldAutosave } from '@/editor/autosave'
import { registerBuffer, unregisterBuffer } from '@/editor/openBuffers'
import { claimStatusReadout, pathTrail, type ReadoutSlot } from '@/editor/statusReadout'
import { describe, notify } from '@/chrome/notices'
import { contextMenuOpen } from '@/menus/menuState'
import { overlayOpen } from '@/overlays/store'
import {
  drawingDetail,
  drawingKindFor,
  notADrawingMessage,
  sameFingerprint,
  sameStamp,
  sniffDrawing,
  type SceneFingerprint,
} from './excalidrawKinds'
import type {
  ExcalidrawImperativeAPI,
  Scene,
  SceneChange,
  Viewport,
} from './ExcalidrawSurface'
import styles from './ExcalidrawPane.module.css'

/*
 * Where Excalidraw fetches its fonts from. (M63)
 *
 * The engine loads its hand-drawn faces at runtime, lazily per font, from this global, and
 * with it unset falls back to a CDN that the CSP (`font-src 'self' data:`) blocks **silently**:
 * every label in a fallback face, nothing logged. `vite.config.ts`'s `excalidrawFonts` plugin
 * answers `/excalidraw/fonts/…` from the package in dev and from a copy beside the bundle in a
 * build, so the one spelling here is relative to the document — `document.baseURI` is
 * `…/index.html?window=…` in every window, so this resolves to the app's own origin under
 * both `http://localhost:1420/` and `tauri://localhost/`. Set at module scope of the static
 * pane so it is in place before the surface's first `import()`; guarded because the render
 * checks SSR-bundle whatever imports `PaneBody` under node, where there is no document.
 */
type AssetPathHost = Window & { EXCALIDRAW_ASSET_PATH?: string }
if (typeof document !== 'undefined') {
  const host = window as AssetPathHost
  host.EXCALIDRAW_ASSET_PATH = new URL('excalidraw/', document.baseURI).href
}

/** Throttle for the `cide://git-status` recheck — `EditorPane`'s number, for the same reason. */
const GIT_RECHECK_MS = 120

/** The lazily imported engine. A type-only reference: erased, so the chunk boundary holds. */
type SurfaceModule = typeof import('./ExcalidrawSurface')

export interface ExcalidrawPaneProps {
  /** Absolute path of the file this tab shows — the path as the user asked for it. */
  path: string
  /** The project root, so the status bar's trail is repo-relative. */
  root?: string | undefined
  /** The owning project, for the dirty dot and the autosave notice. */
  project?: ProjectId | undefined
  /** The owning tab, which is what the dirty flag and the save registry are keyed on. */
  tab?: string | undefined
  /** Whether this pane's tab is the one in front — `ImagePane` and `EditorPane`'s input. */
  onScreen?: boolean | undefined
}

/** What the pane is showing. `ready` carries the scene the surface mounts with. */
type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; scene: Scene; key: number; bytes: number }
  | { kind: 'failed'; why: string }

/** The last path segment, for sentences that should name the file rather than its whole path. */
function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut >= 0 ? path.slice(cut + 1) : path
}

export function ExcalidrawPane({ path, root, project, tab, onScreen }: ExcalidrawPaneProps): ReactNode {
  const theme = useWorkspace((s) => s.theme)
  /** `settings.editor.autosave`, from the mirror — the editor's own default before bootstrap. */
  const autosaveOn = useWorkspace((s) => s.boot?.workspace.settings.editor.autosave ?? true)
  /**
   * Is a Claude Code diff of **this file** on screen, with the agent blocked on it? A boolean,
   * for the reason `EditorPane` gives: the selector runs on every snapshot and a fresh array
   * would re-render this pane on every keystroke in every window. `autosave.ts` says why an
   * agent diff vetoes a background save.
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
  /** Bumped by *Reload*, *Try again*, *Reload from disk* and a clean reload; re-runs the read. */
  const [reloads, setReloads] = useState(0)
  /** Set when the file changed on disk while the drawing had unsaved edits. */
  const [conflict, setConflict] = useState(false)
  /** Something moved while this tab was hidden; check once when it is next revealed. */
  const [gitStale, setGitStale] = useState(false)

  /*
   * The path as it stands, for everything that runs outside a render — the saver, the disk
   * follower, the reload. Assigned during render: a changed `path` on a mounted pane is a
   * rename (`retarget_paths` moved the tab), the scene on screen is the user's, and nothing
   * here re-reads for it. `EditorPane`'s carry-over does the same job for a buffer.
   */
  const pathRef = useRef(path)
  pathRef.current = path
  const surfaceRef = useRef<SurfaceModule | null>(null)
  const apiRef = useRef<ExcalidrawImperativeAPI | null>(null)
  const dirtyRef = useRef(false)
  /** What the file was when the drawing last agreed with the disk — `EditorPane.stampRef`. */
  const stampRef = useRef<FileStamp | null>(null)
  const writableRef = useRef(true)
  /** The fingerprint of what is on disk, or `null` while the baseline handshake is pending. */
  const savedRef = useRef<SceneFingerprint | null>(null)
  /** The live element ids the file held, for the handshake — see decision 4 in the header. */
  const loadedIdsRef = useRef('')
  /** The fingerprint `onChange` last delivered, compared against `savedRef` after a save. */
  const currentFpRef = useRef<SceneFingerprint | null>(null)
  /** The viewport a reload carries across the remount; `null` on a first open. */
  const viewportRef = useRef<Viewport | null>(null)
  /** False once this pane is gone, so a stat in flight cannot raise a bar on a dead pane. */
  const aliveRef = useRef(true)
  const onScreenRef = useRef(onScreen ?? true)
  onScreenRef.current = onScreen ?? true
  const conflictRef = useRef(conflict)
  conflictRef.current = conflict
  const agentDiffRef = useRef(agentDiff)
  agentDiffRef.current = agentDiff
  const autosaveOnRef = useRef(autosaveOn)
  autosaveOnRef.current = autosaveOn
  const elementCountRef = useRef(0)
  const bytesRef = useRef(0)
  const lastEditAtRef = useRef(0)
  const dirtySinceRef = useRef<number | null>(null)
  const autosaveTimerRef = useRef<number | null>(null)

  useEffect(() => {
    aliveRef.current = true
    return () => {
      aliveRef.current = false
      if (autosaveTimerRef.current !== null) window.clearTimeout(autosaveTimerRef.current)
    }
  }, [])

  /** The dirty dot. Deduplicated, fire-and-forget — `EditorPane.reportDirty`'s rule. */
  const reportDirty = useCallback(
    (dirty: boolean) => {
      if (dirtyRef.current === dirty) return
      dirtyRef.current = dirty
      if (project === undefined || tab === undefined) return
      void fileApi.setDirty(project, tab, dirty).catch(() => {})
    },
    [project, tab],
  )

  /* ------------------------------------------------------------------ the status bar */

  const kind = drawingKindFor(path) ?? 'json'
  const trail = useMemo(() => pathTrail(path, root), [path, root])
  const slot = useRef<ReadoutSlot | null>(null)
  const detailNow = useCallback(
    () => drawingDetail(kind, elementCountRef.current, bytesRef.current),
    [kind],
  )
  useEffect(() => {
    if (load.kind !== 'ready') return
    const claim = claimStatusReadout(path, trail, detailNow(), onScreen !== false)
    slot.current = claim
    return () => {
      claim.release()
      slot.current = null
    }
    // `trail` and `onScreen` are read at claim time and maintained by the effects below;
    // listing them would re-claim on every root change and on every tab switch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load.kind, path])
  useEffect(() => {
    slot.current?.setTrail(trail, trail.length)
  }, [trail])
  useEffect(() => {
    if (onScreen === true) slot.current?.focus()
  }, [onScreen])

  /* ------------------------------------------------------------------------- loading */

  useEffect(() => {
    let live = true
    const at = pathRef.current
    const viewport = viewportRef.current
    viewportRef.current = null
    setLoad({ kind: 'loading' })
    apiRef.current = null
    savedRef.current = null
    currentFpRef.current = null
    void Promise.all([import('./ExcalidrawSurface'), fileApi.readBytes(at)])
      .then(async ([surface, { head, bytes }]) => {
        if (!live) return
        surfaceRef.current = surface
        const sniffed = sniffDrawing(bytes)
        if (sniffed === null) {
          throw new NotADrawing(notADrawingMessage(basename(at), 'not JSON, SVG or PNG'))
        }
        let scene: Scene
        try {
          scene = await surface.loadScene(bytes, sniffed, viewport)
        } catch (error) {
          throw new NotADrawing(
            notADrawingMessage(basename(at), error instanceof Error ? error.message : String(error)),
          )
        }
        if (!live) return
        stampRef.current = head.stamp
        writableRef.current = head.writable
        loadedIdsRef.current = surface.liveElementIds(scene.elements ?? [])
        elementCountRef.current = (scene.elements ?? []).filter((e) => !e.isDeleted).length
        bytesRef.current = bytes.length
        dirtySinceRef.current = null
        if (autosaveTimerRef.current !== null) {
          window.clearTimeout(autosaveTimerRef.current)
          autosaveTimerRef.current = null
        }
        setConflict(false)
        reportDirty(false)
        setLoad({ kind: 'ready', scene, key: reloads, bytes: bytes.length })
      })
      .catch((error: unknown) => {
        if (!live) return
        // A parse failure carries its own sentence; an IPC rejection is a `CoreError` object,
        // which `describe` unwraps — `String()` of it is `[object Object]`.
        setLoad({
          kind: 'failed',
          why: error instanceof NotADrawing ? error.message : describe(error),
        })
      })
    return () => {
      live = false
    }
  }, [reloads, reportDirty])

  /** Re-read the file, carrying the viewport across the remount when the engine is up. */
  const reload = useCallback(() => {
    const api = apiRef.current
    const surface = surfaceRef.current
    if (api !== null && surface !== null) viewportRef.current = surface.captureViewport(api)
    setReloads((n) => n + 1)
  }, [])

  /* -------------------------------------------------------------------------- saving */

  const save = useCallback(
    async (cause: 'explicit' | 'autosave'): Promise<void> => {
      const surface = surfaceRef.current
      const api = apiRef.current
      // Nothing loaded — a Ctrl+S on the failure notice — is nothing to write, not a failure.
      if (surface === null || api === null) return
      const at = pathRef.current
      const format = drawingKindFor(at) ?? 'json'
      const snapshot = surface.fingerprintOf(api)
      const bytes = await surface.serializeScene(api, format)
      try {
        const stamp = await fileApi.writeBytes(at, bytes, cause === 'autosave' ? stampRef.current : null)
        // The token moves with the write, or the next autosave compares against the file as it
        // stood when the tab opened and refuses for ever after the first save.
        stampRef.current = stamp
        savedRef.current = snapshot
        bytesRef.current = bytes.length
        slot.current?.set(detailNow())
        setConflict(false)
        // Clean only if nothing moved while the export ran. A still-different fingerprint stays
        // dirty and its timers stay armed.
        if (sameFingerprint(currentFpRef.current, snapshot)) {
          reportDirty(false)
          dirtySinceRef.current = null
        }
      } catch (error) {
        void diag.log(`could not save ${at}: ${String(error)}`)
        if (fileChanged(error)) {
          // The precondition refused: something else wrote this file. Not a failure — the
          // question the conflict bar exists to ask.
          setConflict(true)
        } else if (cause === 'autosave') {
          // A silent failed autosave is the worst outcome this feature can have —
          // `EditorPane.onSave` carries the argument. Filed under this pane's project, because
          // a timer fires while the user may be in another one.
          notify(`cide could not save ${basename(at)}: ${describe(error)}`, {
            kind: 'error',
            hint: 'Your changes are still in the drawing.',
            project,
          })
        }
        // Rethrown either way: the tab stays dirty, which is what puts the close confirmation
        // in front of the user.
        throw error
      }
    },
    [project, reportDirty, detailNow],
  )

  /*
   * Ctrl+S, *Save all* and the close confirmation's *Save and close* all reach this pane
   * through the registry, keyed on the tab — a File tab has exactly one drawing pane, so the
   * dirty flag the confirmation reads and the saver it calls agree by construction.
   */
  useEffect(() => {
    if (tab === undefined) return
    registerBuffer(tab, () => save('explicit'))
    return () => unregisterBuffer(tab)
  }, [tab, save])

  /* ------------------------------------------------------------------------ autosave */

  /**
   * Everything autosave refuses, handed to the one function that decides. The facts are read
   * through refs because this runs from a timer and a window listener, where a captured
   * `conflict` would be the value at the time the closure was made.
   */
  const attemptAutosave = useCallback(
    (reason: 'blur' | 'idle') => {
      const allowed = shouldAutosave(reason, {
        enabled: autosaveOnRef.current,
        dirty: dirtyRef.current,
        readOnly: !writableRef.current,
        conflict: conflictRef.current,
        agentDiff: agentDiffRef.current,
        windowFocused: document.hasFocus(),
        // No panel of this pane holds focus in a way that means "still editing": the engine's
        // popovers live in `document.body`, which is exactly why there is no focusout trigger.
        focusInsideEditor: false,
        overlayOpen: overlayOpen(),
        contextMenuOpen: contextMenuOpen(),
      })
      if (!allowed) return
      void save('autosave').catch(() => {})
    },
    [save],
  )

  /** Arm the idle timer from the last edit — a debounce with the ceiling `autosave.ts` gives. */
  const armAutosave = useCallback(() => {
    if (autosaveTimerRef.current !== null) window.clearTimeout(autosaveTimerRef.current)
    autosaveTimerRef.current = null
    if (dirtySinceRef.current === null) return
    const delay = autosaveDelay(Date.now(), lastEditAtRef.current, dirtySinceRef.current)
    if (delay === null) return
    autosaveTimerRef.current = window.setTimeout(() => {
      autosaveTimerRef.current = null
      if (aliveRef.current) attemptAutosave('idle')
    }, delay)
  }, [attemptAutosave])

  useEffect(() => {
    // Window deactivation is a save, as it is for a buffer — alt-tabbing away from a drawing
    // writes it. `shouldAutosave` tests `windowFocused` first, so this passes even though
    // `document.activeElement` has not moved.
    const onBlur = () => attemptAutosave('blur')
    window.addEventListener('blur', onBlur)
    return () => window.removeEventListener('blur', onBlur)
  }, [attemptAutosave])

  /* ------------------------------------------------------------------- the engine */

  const onApi = useCallback((api: ExcalidrawImperativeAPI) => {
    apiRef.current = api
  }, [])

  /**
   * Every change the engine reports, most of which are not edits.
   *
   * Nothing is decided before the baseline handshake: the first `onChange` whose live element
   * ids are the file's is the scene as loaded, restored by the engine, and that fingerprint is
   * "clean". Until then — the engine measuring its container with an empty scene — nothing is
   * compared. After it, dirty is the fingerprint against `savedRef`, the timers follow the
   * dirty transitions, and the status bar learns a new element count.
   */
  const onChange = useCallback<SceneChange>(
    (elements, appState, files) => {
      const surface = surfaceRef.current
      if (surface === null || apiRef.current === null) return
      const fingerprint = surface.fingerprintOfChange(elements, appState, files)
      if (savedRef.current === null) {
        if (surface.liveElementIds(elements) !== loadedIdsRef.current) return
        savedRef.current = fingerprint
        currentFpRef.current = fingerprint
        return
      }
      currentFpRef.current = fingerprint
      const dirty = !sameFingerprint(fingerprint, savedRef.current)
      if (dirty) {
        const now = Date.now()
        lastEditAtRef.current = now
        if (dirtySinceRef.current === null) dirtySinceRef.current = now
        reportDirty(true)
        armAutosave()
      } else if (dirtyRef.current) {
        // Undone back to the saved scene: clean, and nothing left to write on a timer.
        reportDirty(false)
        dirtySinceRef.current = null
        armAutosave()
      }
      let count = 0
      for (const e of elements) if (!e.isDeleted) count += 1
      if (count !== elementCountRef.current) {
        elementCountRef.current = count
        slot.current?.set(detailNow())
      }
    },
    [reportDirty, armAutosave, detailNow],
  )

  /* -------------------------------------------------------- following the disk */

  /*
   * `cide://session-tool` — an agent's tool call completed and named this file. Clean drawing
   * reloads silently; a dirty one asks. `EditorPane` carries the argument and the caveat: a
   * `sed -i` in a shell pane never sends this, which is what the git-status recheck is for.
   */
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let dropped = false
    void events
      .onSessionTool((_session, paths) => {
        if (dropped || !paths.includes(pathRef.current)) return
        if (dirtyRef.current) setConflict(true)
        else reload()
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
  }, [reload])

  /**
   * Compare the stamp on disk with the one the drawing was read under, by **value** — two
   * answers are two objects. `file.stat` and not a re-read: a two-megabyte PNG must not be
   * read again on every status event to answer a question its `stat` answers.
   */
  const recheckOnDisk = useCallback(() => {
    void fileApi
      .stat(pathRef.current)
      .then((stamp) => {
        if (!aliveRef.current || sameStamp(stamp, stampRef.current)) return
        if (dirtyRef.current) setConflict(true)
        else reload()
      })
      .catch(() => {})
  }, [reload])

  useEffect(() => {
    let unlisten: (() => void) | null = null
    let dropped = false
    let timer: number | null = null
    void events
      .onGitStatus(() => {
        if (dropped) return
        if (!onScreenRef.current) {
          setGitStale(true)
          return
        }
        if (timer !== null) return
        timer = window.setTimeout(() => {
          timer = null
          if (!dropped) recheckOnDisk()
        }, GIT_RECHECK_MS)
      })
      .then((fn) => {
        if (dropped) fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      dropped = true
      if (timer !== null) window.clearTimeout(timer)
      unlisten?.()
    }
  }, [recheckOnDisk])

  useEffect(() => {
    if (!onScreen || !gitStale) return
    setGitStale(false)
    recheckOnDisk()
  }, [onScreen, gitStale, recheckOnDisk])

  /* -------------------------------------------------------------------- render */

  const name = basename(path)
  const Surface = surfaceRef.current?.Surface ?? null
  return (
    <div className={styles.pane} data-audit="excalidrawPane">
      {conflict && (
        <div className={styles.conflict} role="alert" data-pane-strip="fluid">
          <span className={styles.conflictText}>
            This file changed on disk while you had unsaved changes.
          </span>
          <button type="button" className={styles.conflictButton} onClick={reload}>
            Reload from disk
          </button>
          <button
            type="button"
            className={styles.conflictButton}
            /*
             * Re-stamp, not just dismiss — `EditorPane` explains: the old stamp would refuse
             * every later autosave, and "keep mine" means the next write may land on top of
             * what is there. The following successful write records the new stamp.
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
        {load.kind === 'loading' && <div className={styles.notice}>Opening {name}…</div>}
        {load.kind === 'failed' && (
          <div className={styles.notice}>
            <div className={styles.noticeTitle}>This drawing could not be opened</div>
            <div className={styles.noticeWhy}>{load.why}</div>
            <button type="button" className={styles.button} onClick={reload}>
              Try again
            </button>
          </div>
        )}
        {load.kind === 'ready' && Surface !== null && (
          <Surface
            key={load.key}
            initial={load.scene}
            theme={theme}
            viewMode={!writableRef.current}
            name={name}
            onChange={onChange}
            onApi={onApi}
          />
        )}
      </div>
      <div className={styles.bar}>
        <span className={styles.name} title={path}>
          {name}
        </span>
        <span className={styles.facts}>
          {load.kind === 'ready' ? detailNow() : kind === 'json' ? 'Excalidraw' : `Excalidraw · ${kind.toUpperCase()}`}
        </span>
        <span className={styles.spacer} />
        <button
          type="button"
          className={styles.button}
          onClick={reload}
          title="Read the file again — use this after something else has rewritten it"
        >
          Reload
        </button>
      </div>
    </div>
  )
}

/** A parse failure, carrying the sentence the pane shows; told apart from an IPC rejection. */
class NotADrawing extends Error {}
