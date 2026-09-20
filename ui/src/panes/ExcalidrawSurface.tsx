/**
 * The one module that imports `@excalidraw/excalidraw`. (M63)
 *
 * Reached from `ExcalidrawPane.tsx` through `import('./ExcalidrawSurface')` and from nowhere
 * else, which is what keeps 2.7 MB of drawing engine — plus its 144 KB stylesheet and sixty
 * locale chunks — out of every window's entry bundle. `check:excalidraw` pins both halves: this
 * is the only file under `src` that names the package at runtime, and the pane reaches this
 * file only through a dynamic import (a `typeof import(…)` in a type position is erased and
 * does not count).
 *
 * Everything the pane needs from the package is wrapped here as a function over cide's own
 * types (`excalidrawKinds.ts`), so the pane never spells an Excalidraw API and the version
 * bump that renames one has exactly one file to visit.
 *
 * # The three package facts this file relies on, and where each was read
 *
 * * `loadFromBlob` picks its parser from `blob.type` — PNG metadata, SVG metadata, or JSON —
 *   so `mimeFor(sniffDrawing(bytes))` on the `Blob` is what routes the bytes. Read from
 *   `dist/types/excalidraw/data/blob.d.ts` and the package source.
 * * `exportToSvg` / `exportToBlob` embed the scene when `appState.exportEmbedScene` is set,
 *   which is what makes an `.excalidraw.svg` or `.png` written here reopen as a drawing rather
 *   than as a picture.
 * * `onChange` hands over the elements **including deleted ones**, so the fingerprint the pane
 *   compares is taken over that same list on both sides (`fingerprintOfChange` here,
 *   `fingerprintOf` over `getSceneElementsIncludingDeleted()` at save time). A save writes
 *   `getSceneElements()` — the live ones — as `serializeAsJSON` would anyway.
 */
import {
  Excalidraw,
  exportToBlob,
  exportToSvg,
  getSceneVersion,
  loadFromBlob,
  serializeAsJSON,
} from '@excalidraw/excalidraw'
import '@excalidraw/excalidraw/index.css'
import type {
  AppState,
  BinaryFiles,
  ExcalidrawImperativeAPI,
  ExcalidrawInitialDataState,
} from '@excalidraw/excalidraw/types'
import type { ExcalidrawElement } from '@excalidraw/excalidraw/element/types'
import type { ReactNode } from 'react'
import {
  mimeFor,
  sceneFingerprint,
  type DrawingKind,
  type SceneFingerprint,
  type SniffedDrawing,
} from './excalidrawKinds'

export type { ExcalidrawImperativeAPI }

/** What `loadScene` produced and `<Surface>` mounts with. */
export type Scene = ExcalidrawInitialDataState

/** Where the user was looking, carried across a reload so a remount does not jump to the origin. */
export type Viewport = Pick<AppState, 'scrollX' | 'scrollY' | 'zoom'>

/** The shape `onChange` delivers, as the pane sees it. */
export type SceneChange = (
  elements: readonly ExcalidrawElement[],
  appState: AppState,
  files: BinaryFiles,
) => void

/**
 * Parse a file's bytes into a scene.
 *
 * `'empty'` is a blank canvas and never a parse: a `.excalidraw` the tree just created is zero
 * bytes, and `loadFromBlob` on an empty blob throws a JSON error about a file that has nothing
 * wrong with it. A `viewport` is what a reload carries over; on a first open there is none, and
 * `scrollToContent` puts the drawing in view instead — what a file that was drawn on another
 * screen needs, and what a reload of the file under the user's eyes must not do.
 */
export async function loadScene(
  bytes: Uint8Array<ArrayBuffer>,
  sniffed: SniffedDrawing,
  viewport: Viewport | null,
): Promise<Scene> {
  if (sniffed === 'empty') {
    return { elements: [], appState: viewport ?? {}, files: {}, scrollToContent: false }
  }
  const restored = await loadFromBlob(new Blob([bytes], { type: mimeFor(sniffed) }), null, null)
  return {
    elements: restored.elements,
    appState: viewport === null ? restored.appState : { ...restored.appState, ...viewport },
    files: restored.files,
    scrollToContent: viewport === null,
  }
}

/** The viewport as it stands, for `loadScene` on the way back in. */
export function captureViewport(api: ExcalidrawImperativeAPI): Viewport {
  const { scrollX, scrollY, zoom } = api.getAppState()
  return { scrollX, scrollY, zoom }
}

/**
 * The bytes a save writes, in the format the file's **name** asks for.
 *
 * Every road sets `exportEmbedScene`, or the SVG and the PNG would be pictures that do not
 * reopen. The rest of the export `appState` — background on, dark mode off, scale 1 — is
 * whatever Excalidraw holds for the session; none of it is persisted in the file
 * (`APP_STATE_STORAGE_CONF` marks them `export: false`), so a file's rendering does not depend
 * on which theme cide happened to be in when it was saved.
 *
 * A scene with nothing live in it is written as **zero bytes** for SVG and PNG. Excalidraw's
 * own *Export image* refuses an empty canvas, and a picture of nothing has no honest size; zero
 * bytes is exactly what the tree's *New file* produced, and it reopens as the blank canvas it
 * is. JSON has a perfectly good spelling for an empty scene and gets it.
 */
export async function serializeScene(
  api: ExcalidrawImperativeAPI,
  kind: DrawingKind,
): Promise<Uint8Array<ArrayBuffer>> {
  const elements = api.getSceneElements()
  const appState = api.getAppState()
  const files = api.getFiles()
  const text = (s: string): Uint8Array<ArrayBuffer> => {
    const encoded = new TextEncoder().encode(s)
    return new Uint8Array(encoded.buffer.slice(0, encoded.byteLength) as ArrayBuffer)
  }
  switch (kind) {
    case 'json':
      return text(serializeAsJSON(elements, appState, files, 'local'))
    case 'svg': {
      if (elements.length === 0) return new Uint8Array(new ArrayBuffer(0))
      const svg = await exportToSvg({
        elements,
        appState: { ...appState, exportEmbedScene: true },
        files,
      })
      return text(new XMLSerializer().serializeToString(svg))
    }
    case 'png': {
      if (elements.length === 0) return new Uint8Array(new ArrayBuffer(0))
      const blob = await exportToBlob({
        elements,
        appState: { ...appState, exportEmbedScene: true },
        files,
        mimeType: 'image/png',
      })
      return new Uint8Array(await blob.arrayBuffer())
    }
  }
}

/** The fingerprint of what `onChange` just delivered. */
export function fingerprintOfChange(
  elements: readonly ExcalidrawElement[],
  appState: AppState,
  files: BinaryFiles,
): SceneFingerprint {
  return sceneFingerprint(getSceneVersion(elements), Object.keys(files), appState)
}

/** The fingerprint of the scene as the API holds it — the snapshot a save is taken against. */
export function fingerprintOf(api: ExcalidrawImperativeAPI): SceneFingerprint {
  return fingerprintOfChange(
    api.getSceneElementsIncludingDeleted(),
    api.getAppState(),
    api.getFiles(),
  )
}

/** The ids of the elements that are on the canvas, sorted, for the baseline handshake. */
export function liveElementIds(elements: readonly ExcalidrawElement[]): string {
  return elements
    .filter((e) => !e.isDeleted)
    .map((e) => e.id)
    .sort()
    .join('\n')
}

/**
 * The canvas actions the pane hides, and why each one.
 *
 * * `loadScene` — opens a file picker and loads the choice *into this tab*, over the file the
 *   tab is about. Opening a file is the tree's job.
 * * `saveToActiveFile` — Excalidraw's own Ctrl+S, over the File System Access API, which
 *   WebKit does not implement. cide owns Ctrl+S (`file.save` → `registerBuffer`).
 * * `export` and `saveAsImage` — both end in a *Download* that is an `<a download>` or a
 *   `showSaveFilePicker`, and a wry webview drops the first and lacks the second: a button that
 *   does nothing, which is the state this codebase forbids. Copy-as-PNG and copy-as-SVG survive
 *   in the canvas context menu.
 * * `toggleTheme` — the canvas follows cide's theme through the `theme` prop; a toggle here
 *   would drift from it on the next settings change.
 *
 * `changeViewBackgroundColor` and `clearCanvas` stay: both change what the file will hold.
 */
const UI_OPTIONS = {
  canvasActions: {
    loadScene: false,
    saveToActiveFile: false,
    export: false,
    saveAsImage: false,
    toggleTheme: false,
  },
} as const

export interface SurfaceProps {
  readonly initial: Scene
  readonly theme: 'light' | 'dark'
  /** A dependency-cache file: shown, never editable, so it can never go dirty. */
  readonly viewMode: boolean
  /** The file's name, which Excalidraw uses as the drawing's name. */
  readonly name: string
  readonly onChange: SceneChange
  readonly onApi: (api: ExcalidrawImperativeAPI) => void
}

/**
 * The drawing engine, mounted. Fills its containing block, so the pane gives it one with a
 * definite size.
 *
 * `handleKeyboardGlobally` stays **false**: every tab is mounted behind `visibility: hidden`
 * and two drawings with global key handlers would both act on one keystroke. `autoFocus` stays
 * **false**: a restored workspace mounts every tab's pane at once, and the last drawing to
 * mount would take focus from the Claude pane the user was in.
 */
export function Surface({ initial, theme, viewMode, name, onChange, onApi }: SurfaceProps): ReactNode {
  return (
    <Excalidraw
      initialData={initial}
      theme={theme}
      viewModeEnabled={viewMode}
      name={name}
      UIOptions={UI_OPTIONS}
      autoFocus={false}
      handleKeyboardGlobally={false}
      onChange={onChange}
      excalidrawAPI={onApi}
    />
  )
}
