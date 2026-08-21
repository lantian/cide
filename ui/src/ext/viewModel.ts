/**
 * What an extension's panel *is*. (M22)
 *
 * # Data, not DOM
 *
 * An extension does not render. It produces one of these — a tree, a list, a table, some markdown
 * — and cide draws it with cide's own components and cide's own theme tokens.
 *
 * That is not a restriction reluctantly imposed; it is what buys three properties that would each
 * otherwise have to be maintained by hand in every extension ever written:
 *
 * 1. **The theme is right, in both palettes and at every UI scale.** `check:theme` and
 *    `check:ui-scale` are true of a contributed panel for free, because the pixels are drawn by
 *    the same components they already cover. An extension that shipped CSS would be an extension
 *    that looks wrong the first time somebody switches to the light palette.
 * 2. **A panel cannot take the window down.** There is no extension code on the main thread — the
 *    worker produces this and posts it — so the worst a broken extension can do is post a bad view
 *    model, which is a render of an error row.
 * 3. **The main thread stays free.** ADR 0001: one webview per OS window, one JavaScript thread
 *    serving every pane, and PTY bytes coalesced in Rust to ≥8 KiB or 8 ms to keep it that way. An
 *    extension rendering on that thread would compete with xterm for it.
 *
 * # The cost, stated
 *
 * An extension cannot draw something cide has no component for. A chart, a canvas, a custom
 * gesture: not possible, and not by omission. If a panel kind is genuinely missing, the answer is
 * to add it here — where it gets a theme, a check and a keyboard story — and not to open a hole
 * for arbitrary markup.
 *
 * # Import-free
 *
 * `ui/scripts/check-ext-view.mjs` compiles this standalone and executes its rules. Every kind must
 * have a renderer and every renderer a kind, and a check that had to bundle React to establish
 * that would be a check nobody runs.
 */

/** A row's icon, from a small closed set. Extensions do not ship images. */
export type NodeIcon =
  | 'none'
  | 'file'
  | 'folder'
  | 'symbol'
  | 'error'
  | 'warning'
  | 'info'
  | 'run'
  | 'check'

/** How urgent a row is, which is the only colour an extension gets to choose. */
export type NodeTone = 'normal' | 'dim' | 'accent' | 'error' | 'warning'

/**
 * One row of a tree or a list.
 *
 * `id` is the extension's own and must be unique within its panel: it is what a click reports
 * back, what selection is keyed on, and what expansion state is remembered by. cide never parses
 * it.
 */
export interface ViewRow {
  readonly id: string
  readonly label: string
  /** Dimmed, after the label. A line number, a count, a type. */
  readonly detail?: string
  readonly icon?: NodeIcon
  readonly tone?: NodeTone
  /** Children, for a tree. An empty array is a leaf that *could* have children; omit for a leaf. */
  readonly children?: readonly ViewRow[]
  /** Whether a tree row starts open. Only read the first time a row appears. */
  readonly expanded?: boolean
}

/** A button in a panel's toolbar. `command` is the extension's own id, unprefixed. */
export interface ViewAction {
  readonly id: string
  readonly label: string
  readonly icon?: NodeIcon
  /** Drawn but not clickable, with this as the reason. The `AgentsPanel` rule: never a dead control. */
  readonly disabled?: string
}

/**
 * A panel's whole content.
 *
 * A tagged union rather than a record with optional fields, for `AgentRoster`'s reason: an empty
 * `rows` array cannot say whether the extension found nothing, has not looked yet, or failed —
 * and those want three different screens. The first of them is not an edge case either; it is
 * what a panel shows before the user has opened a file.
 */
export type ViewBody =
  /** Nothing to show, and a sentence saying why. Never an empty list with no explanation. */
  | { readonly kind: 'empty'; readonly message: string }
  /** Working. `what` is shown beside the spinner. */
  | { readonly kind: 'loading'; readonly what: string }
  /** The extension itself failed. Drawn as an error, never as an empty list. */
  | { readonly kind: 'failed'; readonly message: string }
  /** A flat list. */
  | { readonly kind: 'list'; readonly rows: readonly ViewRow[] }
  /** A tree. Rows carry their own children. */
  | { readonly kind: 'tree'; readonly rows: readonly ViewRow[] }
  /** A table. Every row's `cells` must be `columns.length` long; shorter rows are padded. */
  | {
      readonly kind: 'table'
      readonly columns: readonly string[]
      readonly rows: readonly { readonly id: string; readonly cells: readonly string[] }[]
    }
  /** Prose. Rendered by cide's markdown pipeline — the same one the preview uses. */
  | { readonly kind: 'markdown'; readonly text: string }

/** Everything cide needs to draw one panel. */
export interface PanelView {
  readonly title?: string
  readonly actions?: readonly ViewAction[]
  readonly body: ViewBody
}

/** Every `ViewBody.kind`, for the check that pins the renderer against this union. */
export const BODY_KINDS: readonly ViewBody['kind'][] = [
  'empty',
  'loading',
  'failed',
  'list',
  'tree',
  'table',
  'markdown',
]

/** Every `NodeIcon`, likewise. */
export const NODE_ICONS: readonly NodeIcon[] = [
  'none',
  'file',
  'folder',
  'symbol',
  'error',
  'warning',
  'info',
  'run',
  'check',
]

/** Every `NodeTone`. */
export const NODE_TONES: readonly NodeTone[] = ['normal', 'dim', 'accent', 'error', 'warning']

/** What a panel with nothing in it looks like before its worker has answered. */
export const PENDING: PanelView = {
  body: { kind: 'loading', what: 'Starting the extension' },
}

/**
 * A view for an extension that will not run.
 *
 * A shared constructor rather than each call site writing its own, because the sentence has to say
 * *which* extension and *why* — a panel reading only "failed" is a panel whose owner cannot be
 * found from what is on screen.
 */
export function failedView(extension: string, why: string): PanelView {
  return { body: { kind: 'failed', message: `${extension}: ${why}` } }
}

/**
 * Is this something an extension actually sent, or something else that arrived on the port?
 *
 * A worker can post anything, including a value that is not an object at all, so every field is
 * checked before it reaches a renderer. Structural and not exhaustive: it establishes that the
 * discriminant is one cide draws and that the collections are collections, which is what stops a
 * malformed post from throwing inside React. A row with a numeric `label` renders as an empty
 * string, which is a cosmetic failure in the extension rather than a crash in the shell.
 */
export function isPanelView(value: unknown): value is PanelView {
  if (typeof value !== 'object' || value === null) return false
  const view = value as { body?: unknown; actions?: unknown }
  if (typeof view.body !== 'object' || view.body === null) return false
  const body = view.body as { kind?: unknown }
  if (typeof body.kind !== 'string') return false
  if (!(BODY_KINDS as readonly string[]).includes(body.kind)) return false
  if (view.actions !== undefined && !Array.isArray(view.actions)) return false
  const rows = (view.body as { rows?: unknown }).rows
  if ((body.kind === 'list' || body.kind === 'tree' || body.kind === 'table') && !Array.isArray(rows)) {
    return false
  }
  return true
}

/**
 * How deep a contributed tree may be drawn, and how many rows in total.
 *
 * Not a security boundary — the worker cannot reach the DOM — but a guard against a panel that
 * renders a hundred thousand rows and freezes the one thread ADR 0001 spends its whole length
 * protecting. A truncated tree says so in its last row; a silently truncated one would be an
 * extension that looks like it lost data.
 */
export const MAX_DEPTH = 24
export const MAX_ROWS = 5_000

/** Flatten a tree to the rows that are actually visible, honouring expansion and the caps. */
export function visibleRows(
  rows: readonly ViewRow[],
  expanded: ReadonlySet<string>,
): { rows: { row: ViewRow; depth: number; hasChildren: boolean }[]; truncated: number } {
  const out: { row: ViewRow; depth: number; hasChildren: boolean }[] = []
  let truncated = 0
  const walk = (list: readonly ViewRow[], depth: number): void => {
    if (depth >= MAX_DEPTH) {
      truncated += list.length
      return
    }
    for (const row of list) {
      if (out.length >= MAX_ROWS) {
        truncated += 1
        continue
      }
      const children = row.children ?? []
      const hasChildren = children.length > 0
      out.push({ row, depth, hasChildren })
      // `expanded` is the live set and `row.expanded` only seeds it, which is what lets a worker
      // re-post a view without stamping on what the user has opened and closed since.
      if (hasChildren && expanded.has(row.id)) walk(children, depth + 1)
    }
  }
  walk(rows, 0)
  return { rows: out, truncated }
}

/** The ids a fresh view should start expanded. */
export function initialExpansion(rows: readonly ViewRow[]): string[] {
  const out: string[] = []
  const walk = (list: readonly ViewRow[], depth: number): void => {
    if (depth >= MAX_DEPTH) return
    for (const row of list) {
      if (row.expanded === true) out.push(row.id)
      if (row.children !== undefined) walk(row.children, depth + 1)
    }
  }
  walk(rows, 0)
  return out
}
