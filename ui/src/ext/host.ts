/**
 * One `Worker` per enabled extension, and the capability gate in front of it. (M22)
 *
 * # Outside React, on purpose
 *
 * The workers live in a module-level map, for `layout/paneHosts.ts`'s reason rather than by
 * imitation of it: a worker holds an extension's whole accumulated state — a parsed document, a
 * connection, whatever it built since it started — and React unmounting a panel because the user
 * clicked a different rail button must not throw that away. A panel closing is a `panelHidden`
 * note; a worker only ever stops when its extension is disabled, uninstalled, or the window closes.
 *
 * # The gate
 *
 * Every `HostRequest` is checked against the extension's granted capabilities here, on the main
 * thread. The worker is told what it was granted so it can *avoid* asking, but nothing in the
 * worker is trusted to enforce it — `protocol.ts` says why at length, and it is one line: a check
 * inside the worker is a check the extension could delete.
 *
 * # What a broken extension costs
 *
 * A worker that throws on load, posts nonsense, or hangs costs its own panels and nothing else.
 * There is no `await` from the main thread into a worker anywhere in this file, so a hung
 * extension cannot make cide wait; and every message is validated by `isWorkerNote` before it is
 * destructured, so a malformed post is a logged line rather than an exception inside React.
 */
import { ext as extApi, extAssetUrl, file as fileApi } from '@/ipc/client'
import type { Capability, ExtensionRef, InstalledExtension } from '@/ipc/client'
import {
  type CapabilityName,
  type HostNote,
  type HostReply,
  type HostRequest,
  type OutlineSymbol,
  type WorkerDiagnostic,
  REQUIRES,
  refusal,
  isWorkerNote,
} from './protocol'
import { type PanelView, PENDING, failedView, isPanelView } from './viewModel'

/** A live extension. */
interface Live {
  readonly id: ExtensionRef
  readonly key: string
  readonly worker: Worker
  readonly capabilities: ReadonlySet<CapabilityName>
  /** The latest view each of this extension's panels posted. */
  readonly views: Map<string, PanelView>
  /** Panels currently on screen, so a worker is only told about the ones it can affect. */
  readonly shown: Set<string>
}

/** Keyed by `<marketplace>.<extension>` — the same pair `cide-ext://` uses as a host. */
const LIVE = new Map<string, Live>()

/** Views for extensions that could not start, so a panel draws the reason rather than a spinner. */
const DEAD = new Map<string, PanelView>()

type Listener = () => void
const LISTENERS = new Set<Listener>()

/** The installed set, as the store last saw it. Read when a panel asks for a worker. */
let INSTALLED: readonly InstalledExtension[] = []

export function keyOf(id: ExtensionRef): string {
  return `${id.marketplace}.${id.extension}`
}

/** Subscribe to view changes. Every panel host does; `useSyncExternalStore` drives the render. */
export function subscribe(listener: Listener): () => void {
  LISTENERS.add(listener)
  return () => {
    LISTENERS.delete(listener)
  }
}

function announce(): void {
  for (const listener of LISTENERS) listener()
}

/**
 * The view a panel should draw right now.
 *
 * Never null: a panel with no worker yet gets [`PENDING`], a panel whose extension failed gets the
 * failure, and a panel whose worker is running but silent gets `PENDING` too. Returning null and
 * letting the caller decide would put the "what does nothing mean" question in every call site.
 *
 * **Returns a stable reference** for an unchanged view. `check:selectors` exists because a
 * selector that returns a fresh object re-renders for ever and ends at *Maximum update depth
 * exceeded*, which unmounts the whole root — and `useSyncExternalStore` compares with `Object.is`
 * exactly as zustand does.
 */
export function viewOf(id: ExtensionRef, panel: string): PanelView {
  const key = keyOf(id)
  const live = LIVE.get(key)
  // `DEAD` is consulted even for a live worker, because a worker that failed to load is still in
  // `LIVE` — it exists, it simply never ran — and falling through to `PENDING` there would leave a
  // panel spinning for ever over an extension that has already reported why it is not working.
  return live?.views.get(panel) ?? DEAD.get(key) ?? PENDING
}

/**
 * Reconcile the running workers with what is installed and enabled.
 *
 * Called on every snapshot. Stops what should not be running, starts what should, and leaves alone
 * what is unchanged — the last of those is the important one: a snapshot arriving because some
 * *other* extension was installed must not restart this one and lose its state.
 */
export function reconcile(installed: readonly InstalledExtension[]): void {
  INSTALLED = installed
  const wanted = new Map<string, InstalledExtension>()
  for (const row of installed) {
    // `main` is `undefined` for a purely declarative extension — a language and a server and
    // nothing else — which is a first-class case: there is nothing to start, and starting nothing
    // is what it should cost.
    if (!row.enabled || row.unavailable !== undefined || row.main === undefined) continue
    wanted.set(keyOf(row), row)
  }

  for (const [key, live] of [...LIVE]) {
    if (!wanted.has(key)) {
      stop(live)
      LIVE.delete(key)
      DEAD.delete(key)
    }
  }
  for (const [key, row] of wanted) {
    if (!LIVE.has(key)) start(key, row)
  }
  announce()
}

/** Stop everything. The window is closing, or the whole registry was replaced. */
export function stopAll(): void {
  for (const live of LIVE.values()) stop(live)
  LIVE.clear()
  DEAD.clear()
  announce()
}

function stop(live: Live): void {
  // `terminate` and not a polite shutdown message. A worker that is wedged in a loop would never
  // read one, and there is nothing to flush: everything an extension produces is a view, and a
  // view from an extension that is being disabled is a view nobody wants.
  try {
    live.worker.terminate()
  } catch (error) {
    console.warn(`[cide] could not terminate ${live.key}`, error)
  }
}

/**
 * The `blob:` shim a worker is actually constructed from.
 *
 * # Why the extension's own URL cannot be handed to `new Worker`
 *
 * **A worker's script URL must be same-origin.** That is not CSP and no header relaxes it: the
 * page is `tauri://localhost`, an extension's files are served from `cide-ext://localhost`, and
 * `new Worker('cide-ext://…')` is refused at construction with a `SecurityError` — the same rule
 * that makes `new Worker` different from `importScripts`, which CORS *does* govern.
 *
 * This was the first thing that went wrong in a real window, and it went wrong twice over: the
 * construction failed, and the failure arrived as a bare `Event` on `onerror` with no `message`,
 * so the panel drew `YAML: undefined`. Both halves are fixed here — this, and [`describeError`].
 *
 * # Why a one-line shim rather than fetching the source
 *
 * Fetching `main.js` and making a blob out of *its text* would work for a file with no imports and
 * silently break the moment an extension split itself into two — a relative `import './parse.js'`
 * in a blob has no base URL to resolve against. A blob that only *imports* the real module leaves
 * every specifier inside it resolving against `cide-ext://…`, which is what the manifest, the jail
 * and this file's own documentation all assume.
 *
 * The import is a cross-origin module fetch, which **is** CORS-governed, which is why
 * `ext_assets.rs` sets `Access-Control-Allow-Origin`.
 */
function shimFor(url: string): string {
  // `JSON.stringify` and not a template literal: the URL contains an extension id from a manifest,
  // and while `is_safe_segment` forbids a quote, the escaping belongs at the point where a string
  // becomes source rather than three modules away in Rust.
  return `import ${JSON.stringify(url)}\n`
}

/**
 * What went wrong, in words, from an event that may carry none.
 *
 * A module worker that fails to *load* fires a plain `Event` at the worker — not an `ErrorEvent` —
 * so `message`, `filename` and `lineno` are all `undefined`. Reading `message` straight into a
 * sentence is how a panel comes to say `YAML: undefined`, which tells the user nothing and tells
 * the extension's author less.
 */
function describeError(event: Event, url: string): string {
  const detail = event as Partial<ErrorEvent>
  if (typeof detail.message === 'string' && detail.message !== '') {
    const where =
      typeof detail.lineno === 'number' && detail.lineno > 0 ? ` (line ${detail.lineno})` : ''
    return `${detail.message}${where}`
  }
  // No message at all: the script did not load. Name the URL, because the two causes — the file is
  // not there, or the extension is disabled and `cide-ext://` refused it — are both about that URL
  // and neither is guessable from "an error occurred".
  return `its worker could not be loaded from ${url}`
}

function start(key: string, row: InstalledExtension): void {
  // The manifest's own entry, not a hard-coded `main.js`. `cide-ext` has already resolved it
  // through the jail, so a `main` that left the installed directory arrived here as `undefined`
  // and `reconcile` never asked for a worker.
  const url = extAssetUrl(row, row.main ?? 'main.js')
  let worker: Worker
  let blob: string | null = null
  try {
    // Same-origin by construction — see `shimFor`. `type: 'module'`, so the `import` inside it is
    // a module fetch and an extension may go on importing its own files relative to `main.js`.
    blob = URL.createObjectURL(new Blob([shimFor(url)], { type: 'text/javascript' }))
    worker = new Worker(blob, { type: 'module', name: `cide-ext:${key}` })
  } catch (error) {
    if (blob !== null) URL.revokeObjectURL(blob)
    DEAD.set(key, failedView(row.name, `its worker could not be started (${String(error)})`))
    return
  }
  // Revoked as soon as the worker exists: the blob has been fetched by then, and one that is never
  // released is a leak for the life of the window — small, and exactly the kind that is invisible
  // until somebody toggles an extension a few hundred times.
  URL.revokeObjectURL(blob)

  const capabilities = new Set<CapabilityName>(
    row.capabilities.map((cap) => cap as unknown as CapabilityName),
  )
  const live: Live = {
    id: row,
    key,
    worker,
    capabilities,
    views: new Map(),
    shown: new Set(),
  }
  LIVE.set(key, live)

  worker.onmessage = (event: MessageEvent<unknown>) => {
    if (!isWorkerNote(event.data)) {
      console.warn(`[cide] ${key} posted something that is not an extension message`, event.data)
      return
    }
    const note = event.data
    if (note.kind === 'log') {
      // Never surfaced to the user. An extension's own logging is for its author, and a notice
      // toast per log line would make one chatty extension unusable for everybody.
      console[note.level === 'debug' ? 'log' : note.level](`[${key}] ${note.text}`)
      return
    }
    if (note.kind === 'view') {
      if (!isPanelView(note.view)) {
        live.views.set(note.panel, failedView(row.name, 'posted a panel cide cannot draw'))
      } else {
        live.views.set(note.panel, note.view)
      }
      announce()
      return
    }
    void answer(live, note.id, note.request)
  }
  worker.onerror = (event: Event) => {
    // The extension threw where cide cannot catch it — module scope, or an async callback — or it
    // never loaded at all. Its panels say which; the worker is left alive because a later message
    // may still be fine, and killing it would turn one bad turn into a dead extension until the
    // user re-enables it.
    //
    // `Event` and not `ErrorEvent`, deliberately: a *load* failure fires a plain event with no
    // message on it, and typing this as an `ErrorEvent` is what let `undefined` reach the screen.
    const message = describeError(event, url)
    console.error(`[cide] ${key}: ${message}`)
    for (const panel of row.contributes.panels) {
      live.views.set(panel.id, failedView(row.name, message))
    }
    // Also recorded as the extension's dead view, so a panel whose worker died before it ever
    // posted anything shows the reason rather than the `PENDING` spinner for ever.
    DEAD.set(key, failedView(row.name, message))
    announce()
  }

  post(live, {
    kind: 'ready',
    extension: `${row.marketplace}.${row.extension}`,
    version: row.version,
    capabilities: [...capabilities],
    panels: row.contributes.panels.map((panel) => ({ id: panel.id, label: panel.label })),
  })

  /*
   * And immediately, what is already on screen.
   *
   * `noteEditor` fires when an editor mounts or its text changes, so a worker started *after* the
   * user opened a file would never be told about it — and its panel would sit on "open a .yaml
   * file" over a `.yaml` file until somebody typed a character. That is what enabling an extension
   * while a file is open looks like, which is the ordinary way an extension is enabled.
   *
   * Only for workers that asked for `editor:read`, like every other editor note.
   */
  if (ACTIVE !== null && capabilities.has('editor:read')) {
    post(live, {
      kind: 'editor',
      path: ACTIVE.path,
      languageId: ACTIVE.languageId,
      text: ACTIVE.text,
      line: ACTIVE.line,
    })
  }
}

function post(live: Live, note: HostNote): void {
  try {
    live.worker.postMessage(note)
  } catch (error) {
    // A value that will not structured-clone. The only failure in this file that is not a
    // message, because it happens at the sender — see `protocol.ts`.
    console.warn(`[cide] could not send ${note.kind} to ${live.key}`, error)
  }
}

/** Answer one request, refusing anything the extension was not granted. */
async function answer(live: Live, id: number, request: HostRequest): Promise<void> {
  const reply = await run(live, request)
  post(live, { kind: 'reply', id, reply })
}

async function run(live: Live, request: HostRequest): Promise<HostReply> {
  const needed = REQUIRES[request.kind]
  if (!live.capabilities.has(needed)) {
    // The gate. On the main thread, where the extension cannot reach it.
    return refusal(request.kind)
  }
  switch (request.kind) {
    case 'readFile': {
      try {
        const text = await fileApi.read(request.path)
        return { ok: true, value: text }
      } catch (error) {
        return { ok: false, why: String(error) }
      }
    }
    case 'activeText': {
      const active = ACTIVE
      return { ok: true, value: active === null ? null : active.text }
    }
    case 'reveal': {
      const handler = REVEAL
      if (handler === null) return { ok: false, why: 'this window has no editor to reveal in' }
      handler(request.path, request.line, request.column ?? 1)
      return { ok: true }
    }
    case 'outline': {
      PUBLISH_OUTLINE?.(live.id, request.path, request.symbols)
      return { ok: true }
    }
    case 'diagnostics': {
      PUBLISH_DIAGNOSTICS?.(live.id, request.path, request.items)
      return { ok: true }
    }
  }
}

// ---------------------------------------------------------------------------------------
// What the shell tells the workers, and the single-slot seams it registers to answer them.
//
// One slot each, not a listener list, on `chrome/panelRequests.ts`'s pattern: there is exactly one
// editor surface and one outline store per window, and a list would let two of them answer.
// ---------------------------------------------------------------------------------------

type Reveal = (path: string, line: number, column: number) => void
type PublishOutline = (
  id: ExtensionRef,
  path: string,
  symbols: readonly OutlineSymbol[],
) => void
type PublishDiagnostics = (
  id: ExtensionRef,
  path: string,
  items: readonly WorkerDiagnostic[],
) => void

let REVEAL: Reveal | null = null
let PUBLISH_OUTLINE: PublishOutline | null = null
let PUBLISH_DIAGNOSTICS: PublishDiagnostics | null = null
let ACTIVE: { path: string; languageId: string | null; text: string; line: number } | null = null

export function registerReveal(handler: Reveal | null): void {
  REVEAL = handler
}

export function registerOutlineSink(handler: PublishOutline | null): void {
  PUBLISH_OUTLINE = handler
}

export function registerDiagnosticsSink(handler: PublishDiagnostics | null): void {
  PUBLISH_DIAGNOSTICS = handler
}

/**
 * The active editor changed, or its text did.
 *
 * Only extensions with `editor:read` are told, and the text is only sent to those — a worker
 * without the capability is not sent a redacted note, it is not sent one at all, because a note
 * with every field null is a note that costs a wake-up to learn nothing.
 *
 * The caller debounces. This is called on every keystroke otherwise, and a `postMessage` of a
 * megabyte of source per keystroke is the one way an extension host can make typing slow.
 */
export function noteEditor(
  active: { path: string; languageId: string | null; text: string; line: number } | null,
): void {
  ACTIVE = active
  for (const live of LIVE.values()) {
    if (!live.capabilities.has('editor:read')) continue
    post(live, {
      kind: 'editor',
      path: active?.path ?? null,
      languageId: active?.languageId ?? null,
      text: active?.text ?? null,
      line: active?.line ?? null,
    })
  }
}

/** A panel came on screen. */
export function notePanelShown(id: ExtensionRef, panel: string): void {
  const live = LIVE.get(keyOf(id))
  if (live === undefined || live.shown.has(panel)) return
  live.shown.add(panel)
  post(live, { kind: 'panelShown', panel })
}

/** A panel went away. */
export function notePanelHidden(id: ExtensionRef, panel: string): void {
  const live = LIVE.get(keyOf(id))
  if (live === undefined || !live.shown.has(panel)) return
  live.shown.delete(panel)
  post(live, { kind: 'panelHidden', panel })
}

/**
 * A command or a row was invoked.
 *
 * Answers whether the worker was there to hear it, so `keys/dispatch.ts` can report *"that
 * extension is not running"* rather than appearing to succeed. A command that silently did nothing
 * is the state `check:commands` exists to make unrepresentable for builtin commands, and a
 * contributed one deserves the same.
 */
export function invoke(
  id: ExtensionRef,
  command: string,
  panel: string | null = null,
  row: string | null = null,
): boolean {
  const live = LIVE.get(keyOf(id))
  if (live === undefined) return false
  post(live, { kind: 'invoke', panel, command, row })
  return true
}

/** Whether an extension's worker is running, for the palette's availability answer. */
export function isRunning(id: ExtensionRef): boolean {
  return LIVE.has(keyOf(id))
}

/** The installed row for a reference, if the store has seen one. */
export function installedRow(id: ExtensionRef): InstalledExtension | null {
  const key = keyOf(id)
  return INSTALLED.find((row) => keyOf(row) === key) ?? null
}

/** The granted capability list, as the manifest spells them. Exported for the Extensions panel. */
export function capabilityNames(caps: readonly Capability[]): CapabilityName[] {
  return caps.map((cap) => cap as unknown as CapabilityName)
}

/** Ask Rust for a fresh snapshot and reconcile. Used by the store's generation-guarded refresh. */
export async function refreshFromRust(): Promise<readonly InstalledExtension[]> {
  const snapshot = await extApi.snapshot()
  reconcile(snapshot.extensions)
  return snapshot.extensions
}
