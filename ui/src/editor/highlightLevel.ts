/**
 * The per-editor highlighting level — IDEA's widget, keyed by path.
 *
 * # Per path, not per pane
 *
 * A split can show one file in two panes, and they must agree: turning highlighting down in one
 * half and leaving squiggles in the other is a state the user cannot reason about. The same
 * keying `revealRequest.ts` uses, for the same reason.
 *
 * # Not persisted, and that is the important half
 *
 * IDEA persists this per file, and cide deliberately does not. A `none` set three weeks ago to
 * read one generated file, restored silently on the next launch, is a user concluding their
 * language server is broken while the app shows a confidently clean gutter — the same
 * confident-empty-list failure the Problems panel is built to prevent, arriving through a setting
 * instead of through a snapshot.
 *
 * So the override lives for the session and the *default* is what persists
 * (`InspectionSettings::default_highlight_level`). A user who wants a file quiet forever turns the
 * default down; a user who wants it quiet now does it here and gets it back on relaunch.
 *
 * # Why this module imports nothing
 *
 * `check-editor.mjs` compiles it standalone.
 */

/** What the editor draws. `syntax` shows only the producer's syntax-classified findings. */
export type Level = 'none' | 'syntax' | 'all'

const overrides = new Map<string, Level>()
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

export function subscribeHighlightLevels(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/**
 * The level for a path: its override, or the workspace default.
 *
 * `fallback` is passed rather than read, so this module stays import-free and so the caller — who
 * already has the settings in hand — does not pay for a second lookup.
 */
export function levelFor(path: string, fallback: Level): Level {
  return overrides.get(path) ?? fallback
}

/** Is this path overridden at all? What the context menu ticks. */
export function overrideFor(path: string): Level | null {
  return overrides.get(path) ?? null
}

/** Set the override. Passing the workspace default still records an override — see below. */
export function setLevel(path: string, level: Level): void {
  // Recorded even when it equals the default, deliberately: "I chose all problems for this file"
  // and "this file follows the default" are different intentions, and a later change to the
  // default should move the second and not the first.
  overrides.set(path, level)
  emit()
}

/** Drop the override, so the path follows the workspace default again. */
export function clearLevel(path: string): void {
  if (overrides.delete(path)) emit()
}

/** How many paths are quieter than the default. What the panel's detail line reports. */
export function reducedCount(fallback: Level): number {
  const rank: Record<Level, number> = { none: 0, syntax: 1, all: 2 }
  let count = 0
  for (const level of overrides.values()) {
    if (rank[level] < rank[fallback]) count += 1
  }
  return count
}

/** Testing seam. Never called by the app. */
export function resetLevelsForTest(): void {
  overrides.clear()
}
