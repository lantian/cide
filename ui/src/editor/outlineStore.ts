/**
 * One outline per open path, fetched from Rust and cached here.
 *
 * The cache is the whole point. `symbols_outline` parses a file in single-digit milliseconds,
 * which is fine per Ctrl+F12 and ruinous per caret move: the breadcrumb updates thirty times a
 * second under a held arrow key, and a round trip each time is the freeze shape `cmd/picker.rs`'s
 * header was written to avoid. So the file is parsed once per load, and every question in between
 * is answered locally by `memberNav.ts`.
 *
 * # Re-parsing while the user types
 *
 * [`scheduleOutline`] is the debounced path, fed by `EditorSurface`'s `onDocChanged`. The debounce
 * is the whole design: a parse is single-digit milliseconds, but a *round trip* per keystroke is
 * the freeze shape `cmd/picker.rs`'s header describes, and the outline is only read when the caret
 * moves or a popup opens — neither of which needs it to be fresh to the character.
 *
 * The text is read at fire time rather than captured at schedule time, so a burst of keystrokes
 * re-parses the buffer as it *ends up*, not as it was 300 ms ago.
 *
 * Module-level rather than React state, and subscribed through `useSyncExternalStore`, because
 * three unrelated surfaces read it — the status bar's trail, the File Structure popup, and the
 * member walk in `keys/dispatch.ts`, which runs outside React entirely.
 */
import { symbols as symbolsApi } from '@/ipc/client'
import type { FileOutline, ProjectId } from '@/ipc/client'
import type { OutlineNode } from './memberNav'

interface Entry {
  outline: FileOutline
  /** Bumped on every write, so a subscriber can tell a re-parse from a no-op. */
  rev: number
}

/** How long after the last keystroke a buffer is re-parsed. */
const DEBOUNCE_MS = 300

const entries = new Map<string, Entry>()
const timers = new Map<string, ReturnType<typeof setTimeout>>()
const inflight = new Set<string>()
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

export function subscribeOutlines(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** The outline for a path, or `null` if nothing has fetched one yet. */
export function outlineOf(path: string): FileOutline | null {
  return entries.get(path)?.outline ?? null
}

/**
 * The one empty array this module ever hands out.
 *
 * **`useSyncExternalStore` compares snapshots with `Object.is`.** Returning a fresh `[]` makes
 * every read look like a change, so React re-renders, reads again, sees another new array, and
 * loops — "Maximum update depth exceeded", thrown out of `EditorPane`. React 19 unmounts the
 * whole tree on an unhandled throw, so the symptom is not a slow editor: it is an **empty
 * window**, with every pane and terminal gone.
 *
 * And it fires on the very first render of any editor, because "no outline yet" is where every
 * buffer starts.
 *
 * Frozen as well as shared, so a caller that mutates what it was given fails loudly here instead
 * of quietly corrupting the next reader's view.
 */
const NONE: readonly OutlineNode[] = Object.freeze([])

/**
 * The symbols for a path, or the shared empty array when the outline is absent, unsupported or
 * failed.
 *
 * **The identity of the empty case is load-bearing** — see [`NONE`]. Returning `[]` here is not a
 * style choice; it is an infinite render loop.
 *
 * `[]` for all three deliberately: a *caller* of this wants something to walk, and the three
 * states differ only in what the popup should *say*, which is `outlineOf`'s job. Collapsing them
 * here would be the mistake if this were the only accessor — it is not.
 */
export function symbolsOf(path: string): readonly OutlineNode[] {
  const outline = entries.get(path)?.outline
  return outline !== undefined && outline.kind === 'ready'
    ? (outline.symbols as readonly OutlineNode[])
    : NONE
}

/**
 * Fetch this path's outline now, unless one is already in flight for it.
 *
 * `text` is the **live buffer**. Omitting it makes Rust read the file from disk, which is the
 * honest fallback for a path no editor is showing — but never the right answer for one that is:
 * the breadcrumb and the member walk are positional, so unsaved lines above the caret would make
 * an on-disk outline name the wrong function for as long as the buffer stayed dirty.
 */
export function fetchOutline(project: ProjectId, path: string, text?: string): void {
  if (inflight.has(path)) return
  inflight.add(path)
  void symbolsApi
    .outline(project, path, text)
    .then((outline) => {
      const previous = entries.get(path)
      entries.set(path, { outline, rev: (previous?.rev ?? 0) + 1 })
      emit()
    })
    .catch(() => {
      // A missing handler in this build, or a path the project does not contain. Neither is
      // worth a toast: the surfaces that read this all render an honest empty state, and a
      // rejected promise here would take the whole React tree down under React 19.
    })
    .finally(() => {
      inflight.delete(path)
    })
}

/**
 * Re-parse after the user stops typing.
 *
 * `read` is called when the timer fires, not now — see the header. A second call while a timer is
 * armed restarts it, which is the right shape here and the wrong one for `gitStatusStore`: a
 * restarting debounce can be starved by a steady stream of triggers, and typing *is* a steady
 * stream, but the thing being starved would be a parse nobody is looking at yet. The moment the
 * user stops, it fires.
 */
export function scheduleOutline(project: ProjectId, path: string, read: () => string): void {
  const existing = timers.get(path)
  if (existing !== undefined) clearTimeout(existing)
  timers.set(
    path,
    setTimeout(() => {
      timers.delete(path)
      fetchOutline(project, path, read())
    }, DEBOUNCE_MS),
  )
}

/**
 * Forget a path when the last editor showing it goes away.
 *
 * Not on every unmount: a split shows one file in two panes, and the first to close would
 * otherwise take the cache the second is still reading.
 */
export function forgetOutline(path: string): void {
  const timer = timers.get(path)
  if (timer !== undefined) {
    clearTimeout(timer)
    timers.delete(path)
  }
  entries.delete(path)
  emit()
}

/** Testing seam. Never called by the app. */
export function resetOutlinesForTest(): void {
  for (const timer of timers.values()) clearTimeout(timer)
  timers.clear()
  entries.clear()
  inflight.clear()
}
