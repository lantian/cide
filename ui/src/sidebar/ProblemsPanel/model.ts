/**
 * The problems panel's pure core: the diagnostics state machine, its counters, and the one
 * sentence two surfaces have to agree on.
 *
 * # Why a three-state snapshot rather than an array
 *
 * A problems panel has exactly one failure mode it cannot survive: showing a confident empty
 * list when nothing has looked. "No problems" and "no analyser" are opposite claims about the
 * workspace, and an `Diagnostic[]` cannot tell them apart — `[]` means both. So the wire
 * shape here is a tagged union, and `unavailable` is a first-class state rather than the
 * absence of one.
 *
 * `StatusBar` already made this call for the bottom bar: its `diagnostics` prop is
 * `Diagnostics | null`, where `null` means nobody looked and prints `✗ — ⚠ —`. `statusBarCounts`
 * below is the bridge, so the panel and the bar are two renderings of one snapshot and cannot
 * drift into disagreeing about whether the workspace is clean.
 *
 * # Why this file imports nothing
 *
 * `ui/scripts/check-problems.mjs` compiles this module alone with `tsc` and imports the
 * emitted JS under node. That works only while the module has no imports at all — not even
 * type-only ones through the `@/*` alias, which a bare `tsc` cannot resolve. Keep it that
 * way; anything needing React, the store or a wire type belongs in `ProblemsPanel.tsx`.
 */

/**
 * The severities a language server can report, in LSP's own set.
 *
 * v1 renders all four rather than the two the status bar counts: dropping info and hint at
 * the model boundary would mean the panel could not show a diagnostic that exists, which is
 * the same class of lie as an empty list.
 */
export type Severity = 'error' | 'warning' | 'info' | 'hint'

/**
 * The four severities, as data, so membership is a lookup rather than a type assertion.
 *
 * `Array.includes` and not `value in SOME_OBJECT`: `in` walks the prototype chain, so
 * `'constructor' in {error: 0, …}` is `true` and every object-keyed lookup below would hand
 * back `Object.prototype.constructor` — a function — where a number or a glyph was expected.
 * That is not hypothetical robustness: severities arrive from a process we do not control, and
 * a function reaching the comparator makes it return `NaN` (which in V8 leaves the whole list
 * unsorted) while a function reaching JSX is not a valid React child.
 */
const SEVERITIES: readonly string[] = ['error', 'warning', 'info', 'hint']

/**
 * Is this one of the four severities we know how to render?
 *
 * Takes `string` rather than `Severity` on purpose: every caller has a value *annotated*
 * `Severity` by a wire type, and the whole point of this guard is that the annotation is a
 * promise from another process rather than a fact.
 */
export function isSeverity(value: string): value is Severity {
  return SEVERITIES.includes(value)
}

export interface Diagnostic {
  /** Workspace-relative, forward slashes — the form the tree and the tab strip already use. */
  path: string
  /** 1-based, as the user counts and as the status bar's `Ln`/`Col` already read. */
  line: number
  column: number
  severity: Severity
  message: string
  /** Who said so, e.g. `rust-analyzer`. Absent when the producer did not name itself. */
  source?: string | undefined
  /** The producer's own code, e.g. `E0308`. Rendered dimmed after the message. */
  code?: string | undefined
}

/**
 * What the panel knows right now.
 *
 * - `unavailable` — no diagnostics source is running. The v1 state, and the reason string is
 *   what both this panel and the status bar's tooltip say.
 * - `scanning` — a source is attached but has not answered yet. Distinct from `unavailable`
 *   because the honest render differs: "starting up" invites waiting, "not installed" does not.
 * - `ready` — a source answered. `items: []` is then a real, reportable zero.
 */
export type DiagnosticsSnapshot =
  | { kind: 'unavailable'; reason: string }
  | { kind: 'scanning'; source: string }
  | { kind: 'ready'; source: string; items: readonly Diagnostic[] }

/**
 * The sentence the app uses for "nobody looked".
 *
 * Exported rather than written twice: `chrome/StatusBar.tsx` shows it as the tooltip on its
 * `✗ — ⚠ —` slot and this panel shows it as body text, and two hand-maintained copies of a
 * user-visible claim are two claims. `check-problems.mjs` asserts the status bar imports this
 * constant instead of holding its own string.
 */
export const NO_DIAGNOSTICS_SOURCE =
  'Diagnostics need a language server. None runs in v1; counts arrive with the language-server milestone.'

/**
 * The snapshot v1 always has.
 *
 * A named constant rather than an inline literal at each call site, so that the day a
 * `DiagnosticStore` exists there is exactly one place that stops returning it. (Checked at the
 * time of writing: `cide-core` has no such store — `KeymapDiagnostic` is a settings-file
 * concern, unrelated to source diagnostics — so there is nothing to read from yet, and the
 * panel is written against this union rather than against a placeholder it would outgrow.)
 */
export const NO_SOURCE: DiagnosticsSnapshot = {
  kind: 'unavailable',
  reason: NO_DIAGNOSTICS_SOURCE,
}

/*
 * Sort weight: worst first within a file, so the first row of a group is the worst news.
 *
 * Precisely typed `Record<Severity, number>`, which is safe only because every read goes
 * through `isSeverity` first. The looser `Record<string, number | undefined>` was the
 * alternative and it is a trap: an `undefined` check does not catch `'constructor'`, whose
 * lookup returns a function rather than `undefined`.
 */
const SEVERITY_RANK: Record<Severity, number> = {
  error: 0,
  warning: 1,
  info: 2,
  hint: 3,
}

/**
 * Rank an unvalidated severity.
 *
 * Sorts unknown values last rather than throwing: the producer is an external process, and a
 * future LSP severity must not be able to blank the panel. Kept as a function so the fallback
 * has somewhere to be commented.
 */
export function severityRank(severity: Severity): number {
  // The `Severity` annotation is a promise from a process we do not control, not a guarantee.
  // An unrecognised value sorts after every known one instead of returning a non-number and
  // turning the comparator into `NaN`, which in V8 leaves the whole list unsorted.
  return isSeverity(severity) ? SEVERITY_RANK[severity] : UNKNOWN_SEVERITY_RANK
}

const UNKNOWN_SEVERITY_RANK = 4

export interface SeverityCounts {
  error: number
  warning: number
  info: number
  hint: number
  /**
   * Diagnostics whose severity is none of the four above.
   *
   * A fifth bucket rather than a drop, and this is the panel's central invariant: the counts
   * must total the number of items. Dropping an unrecognised severity made `countBySeverity`
   * return all zeroes for a non-empty list, and `summaryLine` then printed **“No problems
   * found”** as the headline and as the group's own count, directly above the rows it had
   * just refused to count — the exact confident-clean-bill-of-health failure the whole
   * three-state snapshot exists to prevent, one layer further in.
   *
   * Not folded into `error`: an unknown severity is not known to be an error, and the status
   * bar's ✗ must stay a count of things we actually recognise as errors.
   */
  other: number
}

/**
 * Count by severity, losing nothing.
 *
 * `counts.error + … + counts.other === items.length` for every input, which is what lets
 * `summaryLine` promise that a non-empty list never renders as "No problems found".
 */
export function countBySeverity(items: readonly Diagnostic[]): SeverityCounts {
  const counts: SeverityCounts = { error: 0, warning: 0, info: 0, hint: 0, other: 0 }
  for (const item of items) {
    // Guarded because `items` crosses a process boundary. The unrecognised ones land in
    // `other` rather than being dropped: see the field's comment for what dropping cost.
    if (isSeverity(item.severity)) counts[item.severity] += 1
    else counts.other += 1
  }
  return counts
}

/** The status bar's `Diagnostics | null`, derived from the same snapshot the panel renders. */
export function statusBarCounts(
  snapshot: DiagnosticsSnapshot,
): { errors: number; warnings: number } | null {
  // Both non-`ready` states collapse to `null`. A scanning source has counts *pending*, not
  // zero, and the bar's `✗ —` is exactly the right thing to show while it starts up.
  if (snapshot.kind !== 'ready') return null
  const counts = countBySeverity(snapshot.items)
  return { errors: counts.error, warnings: counts.warning }
}

/** True only when something actually looked. The panel gates every count on this. */
export function checked(snapshot: DiagnosticsSnapshot): boolean {
  return snapshot.kind === 'ready'
}

export interface FileGroup {
  path: string
  items: Diagnostic[]
  counts: SeverityCounts
}

/**
 * Group by file and put both levels in a total order.
 *
 * Total, not merely "sorted enough": a partial order leaves rows free to swap on re-render
 * when two diagnostics tie, and a list that reshuffles under the pointer is unusable. Files
 * go by path, rows by severity then line then column then message — severity first because
 * the group's first row is what the collapsed state would summarise.
 *
 * Comparison is by `<`, not `localeCompare`: these are paths, and the order has to be the
 * same on every machine regardless of locale.
 */
export function groupByFile(items: readonly Diagnostic[]): FileGroup[] {
  const byPath = new Map<string, Diagnostic[]>()
  for (const item of items) {
    const bucket = byPath.get(item.path)
    if (bucket === undefined) byPath.set(item.path, [item])
    else bucket.push(item)
  }

  const groups: FileGroup[] = []
  for (const [path, bucket] of byPath) {
    bucket.sort(
      (a, b) =>
        severityRank(a.severity) - severityRank(b.severity) ||
        a.line - b.line ||
        a.column - b.column ||
        (a.message < b.message ? -1 : a.message > b.message ? 1 : 0),
    )
    groups.push({ path, items: bucket, counts: countBySeverity(bucket) })
  }
  groups.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))
  return groups
}

/** What the panel says at the top, and how loudly. */
export type HeadlineTone = 'unknown' | 'clean' | 'counts'

export interface Headline {
  tone: HeadlineTone
  /** The claim itself. Never contains a count unless `tone === 'counts'`. */
  text: string
  /** The line under it: why the claim is what it is, or what to do about it. */
  detail: string
}

/**
 * The one claim the panel makes about the workspace.
 *
 * Split out of the component and tested here because the wording *is* the feature: the whole
 * point of this surface in v1 is that it says "nobody looked" instead of "you are fine", and
 * a component test would be checking JSX rather than the claim.
 */
export function headline(snapshot: DiagnosticsSnapshot): Headline {
  if (snapshot.kind === 'unavailable') {
    return {
      tone: 'unknown',
      text: 'No diagnostics source is running',
      detail: snapshot.reason,
    }
  }
  if (snapshot.kind === 'scanning') {
    return {
      tone: 'unknown',
      text: `Waiting for ${snapshot.source}`,
      // Deliberately not "no problems yet": a partial answer rendered as a clean bill of
      // health is the same failure as the empty list, one round trip earlier.
      detail: 'The analyser has not reported yet. Counts appear when it does.',
    }
  }

  const counts = countBySeverity(snapshot.items)
  if (snapshot.items.length === 0) {
    return {
      tone: 'clean',
      text: 'No problems found',
      // The source is named because "no problems" is only as good as who checked.
      detail: `${snapshot.source} reported nothing in this workspace.`,
    }
  }
  return {
    tone: 'counts',
    text: summaryLine(counts),
    detail: `Reported by ${snapshot.source}.`,
  }
}

/**
 * `2 errors, 1 warning` — singularised, and severities with none are omitted entirely.
 *
 * Omitting the zeroes rather than printing `0 hints` keeps the line short enough for a 252px
 * panel and stops a zero from reading as a claim of its own.
 */
export function summaryLine(counts: SeverityCounts): string {
  const parts: string[] = []
  const push = (n: number, one: string, many: string) => {
    if (n > 0) parts.push(`${n} ${n === 1 ? one : many}`)
  }
  push(counts.error, 'error', 'errors')
  push(counts.warning, 'warning', 'warnings')
  push(counts.info, 'info', 'infos')
  push(counts.hint, 'hint', 'hints')
  // Same word either way: "1 other" / "3 other" reads as a residual bucket, where "3 others"
  // reads as a noun the panel never defined. Short matters at 252px.
  push(counts.other, 'other', 'other')
  // Reachable only when every bucket is zero, which — because `other` catches everything
  // `isSeverity` rejects — happens only for a genuinely empty list. That is what makes this
  // fallback a true statement rather than a headline printed over visible rows.
  return parts.length === 0 ? 'No problems found' : parts.join(', ')
}

/**
 * The header's right-aligned meta figure.
 *
 * `—` rather than `0` whenever nothing looked. This is the same decision the explorer makes
 * when it withholds its row count during the first walk, and the same one the status bar
 * makes with `✗ —`: a digit in a counter is a claim.
 */
export function metaFigure(snapshot: DiagnosticsSnapshot): string {
  if (snapshot.kind !== 'ready') return '—'
  return String(snapshot.items.length)
}
