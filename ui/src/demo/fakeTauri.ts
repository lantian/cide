/**
 * A Tauri backend that is not there: the IPC the real `App` talks to, answered from a table.
 *
 * This exists so the feature site's screenshots are pictures of the **real** React UI rather than
 * of a mock-up — `ui/demo.html` boots the same `main.tsx` a cide window does, over this, in
 * headless Chromium (`ui/scripts/demo-shots.mjs`). Nothing here reaches a window of the app:
 * `vite build` builds `index.html` only, and `check:demo` fails if that ever names the demo.
 *
 * # Why not `@tauri-apps/api/mocks`
 *
 * `ui/src/ipc/client.ts` is the only file allowed to import `@tauri-apps/api`, and a second
 * importer — even a dev-only one — is the kind of exception that is copied. `mocks.js` is also
 * only a few dozen lines of assignments onto `window.__TAURI_INTERNALS__`, which is what the
 * library reads at call time; `sidebar/treeRefreshSmoke.ts` already fakes the backend that way.
 * So this does the same, and implements exactly the surface `core.js`, `event.js`, `window.js`
 * and `webview.js` touch: `invoke`, the callback registry, `metadata`, `convertFileSrc`, and the
 * event plugin's `listen`/`unlisten`.
 *
 * # An unmocked command is not an error
 *
 * The app issues a few hundred commands and a scene touches a few dozen. A command with no
 * handler resolves the empty value of its declared answer type (`demo/defaults.ts`), is logged once as
 * `[demo] unmocked <cmd>`, and is recorded in `window.__demoUnmocked` — which `demo-shots.mjs
 * --strict` reads, so a shot that silently rendered an empty panel because its data never came
 * can be told apart from one that rendered the panel as designed. A rejection would instead land
 * in whatever `.catch` the caller wrote, which for many is an error banner — a worse picture.
 */

export type Args = Record<string, unknown>
export type Handler = (args: Args) => unknown

interface Internals {
  invoke: (cmd: string, args?: Args, options?: unknown) => Promise<unknown>
  transformCallback: (cb?: (data: unknown) => void, once?: boolean) => number
  unregisterCallback: (id: number) => void
  runCallback: (id: number, data: unknown) => void
  callbacks: Map<number, (data: unknown) => void>
  convertFileSrc: (path: string, protocol?: string) => string
  metadata: {
    currentWindow: { label: string }
    currentWebview: { windowLabel: string; label: string }
  }
  plugins: Record<string, unknown>
}

declare global {
  interface Window {
    __demoUnmocked?: string[]
    __demoCalls?: string[]
  }
}

const callbacks = new Map<number, (data: unknown) => void>()
let nextCallback = 1

function transformCallback(cb?: (data: unknown) => void, once = false): number {
  const id = nextCallback++
  callbacks.set(id, (data) => {
    if (once) callbacks.delete(id)
    cb?.(data)
  })
  return id
}

/** Event name → the callback ids listening to it. */
const listeners = new Map<string, number[]>()
let nextEventId = 1

/** Deliver a `cide://` event to every subscriber, exactly as Rust's `emit` would. */
export function emitEvent(event: string, payload: unknown): void {
  for (const handler of listeners.get(event) ?? []) {
    callbacks.get(handler)?.({ event, id: nextEventId++, payload })
  }
}

/**
 * Install the fake. Must run before `@tauri-apps/api` is first *called* — importing it is fine,
 * since every function reads `window.__TAURI_INTERNALS__` at call time — which is why
 * `demo/main.tsx` imports the module that calls this ahead of `../main`.
 */
export function installFakeTauri(
  label: string,
  table: ReadonlyMap<string, Handler>,
  empty: (cmd: string) => unknown,
): void {
  const unmocked = new Set<string>()
  window.__demoUnmocked = []
  window.__demoCalls = []

  async function invoke(cmd: string, args: Args = {}): Promise<unknown> {
    window.__demoCalls?.push(cmd)
    if (cmd === 'plugin:event|listen') {
      const event = String(args['event'])
      const handler = Number(args['handler'])
      listeners.set(event, [...(listeners.get(event) ?? []), handler])
      return handler
    }
    if (cmd === 'plugin:event|unlisten') {
      const event = String(args['event'])
      const id = Number(args['eventId'])
      listeners.set(event, (listeners.get(event) ?? []).filter((h) => h !== id))
      return null
    }
    const handler = table.get(cmd)
    if (handler) return handler(args)
    if (!unmocked.has(cmd)) {
      unmocked.add(cmd)
      window.__demoUnmocked?.push(cmd)
      console.warn(`[demo] unmocked ${cmd}`)
    }
    return empty(cmd)
  }

  const internals: Internals = {
    invoke,
    transformCallback,
    unregisterCallback: (id) => void callbacks.delete(id),
    runCallback: (id, data) => callbacks.get(id)?.(data),
    callbacks,
    // No asset protocol in a browser. The demo serves nothing under a real path, so an image the
    // app asks for by path resolves against the demo's own `demo-assets/` directory by name.
    convertFileSrc: (path) => `demo-assets/${path.split('/').pop() ?? ''}`,
    metadata: {
      currentWindow: { label },
      currentWebview: { windowLabel: label, label },
    },
    plugins: {},
  }
  const w = window as unknown as {
    __TAURI_INTERNALS__: Internals
    __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (event: string, id: number) => void }
  }
  w.__TAURI_INTERNALS__ = internals
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (event, id) => {
      callbacks.delete(id)
      listeners.set(event, (listeners.get(event) ?? []).filter((h) => h !== id))
    },
  }
}
