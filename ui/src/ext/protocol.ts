/**
 * The wire between cide and an extension's worker. (M22)
 *
 * # Why there is a wire at all
 *
 * An extension runs in a dedicated `Worker`. It has no DOM, no `@tauri-apps/api` and no `invoke` —
 * not because those are hidden from it, but because a worker is a different realm and they are
 * genuinely not there. Everything it can do arrives through this protocol, which is a fixed table
 * of requests, and every one of them is checked against the extension's declared capabilities
 * **in the host**, on the main thread, where the extension cannot reach.
 *
 * That last clause is the whole design. A capability check inside the worker would be a check the
 * extension could delete.
 *
 * # Two directions, three shapes
 *
 * The worker sends `HostRequest`s and gets `HostReply`s; it also sends unsolicited `WorkerNote`s —
 * a new view, a log line. cide sends `HostNote`s — a file was opened, a command was invoked, the
 * panel was shown — and never asks the worker a question it has to answer. That asymmetry is
 * deliberate: a request cide had to await would be a request a hung extension could make cide wait
 * for, and there is no timeout short enough to be both safe and not flaky.
 *
 * # Everything is structured-cloneable
 *
 * Plain objects, strings, numbers, booleans, arrays. No functions, no `Map`, no class instances,
 * no `undefined` inside arrays. A value that cannot be cloned throws inside `postMessage` at the
 * sender, which is the one failure in this file that is not reported as a message.
 *
 * Import-free apart from view-model types, so `ui/scripts/check-ext-protocol.mjs` can compile it
 * standalone and assert that every request has a reply and every capability named here is one
 * `cide_ipc::ext::Capability` actually has.
 */
import type { PanelView } from './viewModel'

/** The capability strings, spelled as the manifest spells them. Mirrors `cide_ipc::ext::Capability`. */
export type CapabilityName =
  | 'editor:read'
  | 'editor:write'
  | 'fs:read'
  | 'git:read'
  | 'process:spawn'

/** Every capability, for the check that pins this against the Rust enum. */
export const CAPABILITIES: readonly CapabilityName[] = [
  'editor:read',
  'editor:write',
  'fs:read',
  'git:read',
  'process:spawn',
]

/** What cide tells a worker, unprompted. */
export type HostNote =
  /**
   * The first message, always. Nothing else is sent before it and the worker must not send before
   * receiving it — its own module scope has run by then, but it has no id, no capability list and
   * no way to name its own panels until this arrives.
   */
  | {
      readonly kind: 'ready'
      readonly extension: string
      readonly version: string
      readonly capabilities: readonly CapabilityName[]
      readonly panels: readonly { readonly id: string; readonly label: string }[]
    }
  /** The active editor changed, or its text did. Only with `editor:read`. */
  | {
      readonly kind: 'editor'
      readonly path: string | null
      readonly languageId: string | null
      readonly text: string | null
      /** 1-based, where the caret is. `null` when no editor is focused. */
      readonly line: number | null
    }
  /** A panel became visible. Workers do their work here rather than on every editor change. */
  | { readonly kind: 'panelShown'; readonly panel: string }
  /** A panel went away — the sidebar was closed, the tab switched. */
  | { readonly kind: 'panelHidden'; readonly panel: string }
  /** A toolbar button, a row click, or a palette command. */
  | {
      readonly kind: 'invoke'
      readonly panel: string | null
      readonly command: string
      /** The row's own id, when the invocation came from a row. */
      readonly row: string | null
    }
  /** An answer to a `HostRequest`. */
  | { readonly kind: 'reply'; readonly id: number; readonly reply: HostReply }

/** What a worker asks cide for. Every one is capability-gated in the host. */
export type HostRequest =
  /** Read a file under a project root. `fs:read`. */
  | { readonly kind: 'readFile'; readonly path: string }
  /** The text of the active editor. `editor:read`. */
  | { readonly kind: 'activeText' }
  /** Move the caret in the active editor. `editor:write`. */
  | { readonly kind: 'reveal'; readonly path: string; readonly line: number; readonly column?: number }
  /** Publish an outline for a path. `editor:write`. */
  | {
      readonly kind: 'outline'
      readonly path: string
      readonly symbols: readonly OutlineSymbol[]
    }
  /** Publish diagnostics for a path. `editor:write`. */
  | {
      readonly kind: 'diagnostics'
      readonly path: string
      readonly items: readonly WorkerDiagnostic[]
    }

/** A symbol an extension contributes. A flattened subset of `cide_ipc::Symbol`. */
export interface OutlineSymbol {
  readonly name: string
  readonly detail?: string
  /** 1-based. */
  readonly line: number
  readonly endLine?: number
  readonly children?: readonly OutlineSymbol[]
}

/** A problem an extension found. */
export interface WorkerDiagnostic {
  /** 1-based. */
  readonly line: number
  readonly column?: number
  readonly endLine?: number
  readonly endColumn?: number
  readonly severity: 'error' | 'warning' | 'info' | 'hint'
  readonly message: string
  readonly code?: string
}

/**
 * What cide answers with.
 *
 * `ok: false` and a sentence, never a rejected promise: a refusal is a fact the extension should
 * be able to *read* and report in its panel, and a rejection is a fact it can only crash on.
 */
export type HostReply =
  | { readonly ok: true; readonly value?: unknown }
  | { readonly ok: false; readonly why: string }

/** What a worker sends without being asked. */
export type WorkerNote =
  /** A panel's content. The only way anything an extension made reaches the screen. */
  | { readonly kind: 'view'; readonly panel: string; readonly view: PanelView }
  /** A line for the console, prefixed with the extension's id. Never shown to the user. */
  | { readonly kind: 'log'; readonly level: 'debug' | 'info' | 'warn' | 'error'; readonly text: string }
  /** A request, awaiting a `reply` note with the same id. */
  | { readonly kind: 'request'; readonly id: number; readonly request: HostRequest }

/**
 * Which capability a request needs.
 *
 * A table and not a `switch`, so `check-ext-protocol.mjs` can assert it covers every request kind
 * — a request with no entry would be a request that needs nothing, which is the one failure mode
 * this whole file exists to make impossible.
 */
export const REQUIRES: Readonly<Record<HostRequest['kind'], CapabilityName>> = {
  readFile: 'fs:read',
  activeText: 'editor:read',
  reveal: 'editor:write',
  outline: 'editor:write',
  diagnostics: 'editor:write',
}

/** Every request kind, for the same check. */
export const REQUEST_KINDS: readonly HostRequest['kind'][] = [
  'readFile',
  'activeText',
  'reveal',
  'outline',
  'diagnostics',
]

/**
 * Whether a message off the worker port is one cide understands.
 *
 * A worker can post anything. Everything below runs on the main thread with the message still
 * untrusted, so this is the boundary: a note that does not pass is logged and dropped, never
 * destructured.
 */
export function isWorkerNote(value: unknown): value is WorkerNote {
  if (typeof value !== 'object' || value === null) return false
  const note = value as { kind?: unknown }
  if (note.kind === 'view') {
    const view = value as { panel?: unknown; view?: unknown }
    return typeof view.panel === 'string' && typeof view.view === 'object' && view.view !== null
  }
  if (note.kind === 'log') {
    const log = value as { text?: unknown }
    return typeof log.text === 'string'
  }
  if (note.kind === 'request') {
    const request = value as { id?: unknown; request?: unknown }
    if (typeof request.id !== 'number' || typeof request.request !== 'object') return false
    const inner = request.request as { kind?: unknown } | null
    return (
      inner !== null &&
      typeof inner.kind === 'string' &&
      (REQUEST_KINDS as readonly string[]).includes(inner.kind)
    )
  }
  return false
}

/**
 * The refusal a request without its capability gets.
 *
 * One sentence, naming the capability in the spelling the manifest uses, so an author can copy it
 * straight into `capabilities`. A refusal that said only "not permitted" would send them to the
 * documentation for the one fact the message could have carried.
 */
export function refusal(kind: HostRequest['kind']): HostReply {
  return {
    ok: false,
    why: `\`${kind}\` needs the \`${REQUIRES[kind]}\` capability. Add it to this extension's manifest and reinstall it.`,
  }
}
