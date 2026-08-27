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
 * `statusBarCounts` below carries the same distinction to the activity rail's ⚠ badge: it
 * returns `null` when nobody looked, which draws no badge at all, and that is not the same
 * answer as a badge reading zero. The panel and the badge are therefore two renderings of one
 * snapshot and cannot drift into disagreeing about whether the workspace is clean. (It fed the
 * status bar too until M28, which is where the name comes from.)
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
 * The panel renders all four rather than the two the rail's badge counts: dropping info and
 * hint at the model boundary would mean the panel could not show a diagnostic that exists,
 * which is the same class of lie as an empty list.
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
  /**
   * Absolute. What `file.open` and `requestReveal` act on. (M12)
   *
   * Carried beside `path` rather than derived from it, because there is no reliable way back: a
   * multi-root project prefixes the relative form with a root label, and re-joining that onto a
   * root is a second place to get the mapping wrong. Optional so every existing fixture — which
   * omits it — still describes a valid diagnostic; the row falls back to `path`.
   */
  absPath?: string | undefined
  /**
   * Whether this is about the shape of the text or its meaning. (M12)
   *
   * Set by the producer, never inferred from [`source`]: "tree-sitter means syntax" is true today
   * and stops being true the moment a language server reports a parse error. The per-editor
   * *Syntax only* highlighting level reads this.
   */
  kind?: 'syntax' | 'semantic' | undefined
  /** 1-based, as the user counts and as the status bar's `Ln`/`Col` already read. */
  line: number
  column: number
  /**
   * The end of the squiggle, exclusive. (M12)
   *
   * Optional because the panel does not need it — a row shows a position, not a span — and
   * because every fixture written before the editor drew anything omits it. The *editor* needs
   * it, and falls back to a one-character span when it is absent, which is what a producer that
   * reports only a point means anyway.
   */
  endLine?: number | undefined
  endColumn?: number | undefined
  severity: Severity
  message: string
  /** Who said so, e.g. `rust-analyzer`. Absent when the producer did not name itself. */
  source?: string | undefined
  /** The producer's own code, e.g. `E0308`. Rendered dimmed after the message. */
  code?: string | undefined
  /**
   * The file changed on disk after this was published, so `line` may name a different line. (M18)
   *
   * Set in Rust, by `cide_core::diagnostics::DiagnosticStore::snapshot`, from the paths the file
   * watcher marked since the source last spoke about them. The panel neither derives nor clears
   * it — the analyser is the only thing that can answer staleness, and it answers by republishing.
   *
   * Optional because every fixture written before M18 omits it, and because `false` and absent
   * mean the same thing here: nothing has said this row is out of date.
   *
   * The row stays **clickable**. A jump that may be a few lines off is worth more than a dead
   * row, as long as it says which it is — the same trade the whole three-state snapshot makes.
   */
  stale?: boolean | undefined
}

/**
 * What the panel knows right now.
 *
 * - `unavailable` — no diagnostics source is running, and the reason string is what the panel
 *   says. (It was the status bar's tooltip too until M28 — see [`NO_DIAGNOSTICS_SOURCE`].)
 * - `scanning` — a source is attached but has not answered yet. Distinct from `unavailable`
 *   because the honest render differs: "starting up" invites waiting, "not installed" does not.
 * - `ready` — a source answered. `items: []` is then a real, reportable zero.
 */
export type DiagnosticsSnapshot =
  | { kind: 'unavailable'; reason: string; sources?: readonly SourceReport[] | undefined }
  | {
      kind: 'scanning'
      source: string
      /**
       * What the sources that *have* answered found. (M12)
       *
       * The reason this arm carries items at all: with four possible analysers there is one
       * `kind` for the whole snapshot, so rust-analyzer indexing while gopls has answered with
       * twelve findings must be either `scanning` — hiding twelve real findings — or `ready`, a
       * clean-ish bill of health for half the workspace. Both are the failure this union exists
       * to prevent, so the third option is to show what is known beneath a headline naming who
       * has not answered.
       *
       * **This changes none of the load-bearing predicates.** `statusBarCounts` still returns
       * `null`, `checked` still returns `false`, `metaFigure` still returns `—`, and `headline`
       * is still toned `unknown`. A partial answer is still not a count.
       */
      items?: readonly Diagnostic[] | undefined
      sources?: readonly SourceReport[] | undefined
    }
  | {
      kind: 'ready'
      source: string
      items: readonly Diagnostic[]
      sources?: readonly SourceReport[] | undefined
      /**
       * How many items the producer's emit cap dropped. (M12)
       *
       * Non-zero means `items` is a prefix, and [`metaFigure`] reports the sum — a header counter
       * reading `1000` when there are `1214` is the same quiet lie the panel exists to avoid, one
       * layer down.
       */
      truncated?: number | undefined
    }

/**
 * One analyser's line in the panel's source list. (M12)
 *
 * The array of these is what lets the panel say *"rust-analyzer is not installed"* beside
 * *"gopls found nothing"* — which a single `source: string` cannot express, and which is the
 * whole reason the snapshot was widened.
 */
export interface SourceReport {
  /** `rustAnalyzer` | `gopls` | `treeSitter` | `claude`. */
  id: string
  /** What the user sees: `rust-analyzer`. Carried so the panel never rebuilds it. */
  label: string
  status: SourceStatus
  /** How many of the snapshot's items came from here. */
  items: number
}

/** What one analyser is doing. The same three states as the snapshot, one level down. */
export type SourceStatus =
  | { kind: 'unavailable'; reason: string }
  // `percentage` optional rather than required: pre-M25 fixtures and extension-published
  // snapshots omit it, and an absent number means the same thing as a null one — busy,
  // length unknown.
  | { kind: 'scanning'; detail: string; percentage?: number | null }
  | { kind: 'ready' }

/**
 * The sentence the app uses for "nobody looked".
 *
 * Exported rather than written twice: this panel shows it as body text, and
 * `chrome/StatusBar.tsx` showed it as the tooltip on its `✗ — ⚠ —` slot until that slot was
 * removed (M28). Two hand-maintained copies of a user-visible claim are two claims, so
 * `check-problems.mjs` still pins the bar — inverted now, asserting it reaches for neither the
 * constant nor a copy of the string. A re-added slot has to import this one.
 */
export const NO_DIAGNOSTICS_SOURCE =
  'No language server is running for this project. Rust needs rust-analyzer and Go needs gopls on PATH.'

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

/** The rail badge's counts, or `null` when nobody looked. Same snapshot the panel renders. */
/**
 * Whether any analyser is currently working, for the rail's busy dot and nothing else.
 *
 * True when the snapshot itself is still `scanning` (the first pass, before anything has
 * answered) **or** any listed source reports `scanning` (a later pass: `ready` overall
 * while rust-analyzer re-indexes after a branch switch). The two arms cover different
 * sessions of the same fact — "something is looking right now" — and either alone misses
 * the other's case.
 */
export function anyScanning(snapshot: DiagnosticsSnapshot | null): boolean {
  if (snapshot === null) return false
  if (snapshot.kind === 'scanning') return true
  return (snapshot.sources ?? []).some((source) => source.status.kind === 'scanning')
}

export function statusBarCounts(
  snapshot: DiagnosticsSnapshot,
): { errors: number; warnings: number; pending: boolean } | null {
  // `scanning` counts too, marked pending. It used to collapse to `null` like `unavailable`
  // on the argument that counts are *pending*, not zero — but the two states then shared
  // the bar's `✗ —` slot AND its "no language server is running" tooltip, so a server that
  // was merely indexing (or stuck indexing — one leaked progress token holds the whole
  // snapshot at `scanning` for ever) read as a server that did not exist, while the editor
  // visibly underlined the diagnostics it was publishing. Servers stream diagnostics while
  // they scan; showing the live counts with a pending flag is both more honest and immune
  // to a stuck scan. `null` now means exactly "nothing is looking": unavailable, or no
  // source at all — the one state the sentence in the tooltip is true for.
  if (snapshot.kind !== 'ready' && snapshot.kind !== 'scanning') return null
  // `items` is optional off the `ready` arm — a scanning snapshot may not have published yet.
  const counts = countBySeverity(snapshot.items ?? [])
  return {
    errors: counts.error,
    warnings: counts.warning,
    pending: snapshot.kind === 'scanning',
  }
}

/** True only when something actually looked. The panel gates every count on this. */
export function checked(snapshot: DiagnosticsSnapshot): boolean {
  return snapshot.kind === 'ready'
}

export interface FileGroup {
  path: string
  items: Diagnostic[]
  counts: SeverityCounts
  /**
   * Any row in this group is marked out of date. (M18)
   *
   * Derived here rather than re-scanned in the component, so the heading's warning and the rows'
   * own marks are one decision. `some` and not `every`: a file whose findings come from two
   * sources may have one of them current and the other pending a re-check, and the heading has to
   * warn about the group it heads.
   */
  stale: boolean
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
    groups.push({
      path,
      items: bucket,
      counts: countBySeverity(bucket),
      stale: bucket.some((item) => item.stale === true),
    })
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
export function headline(snapshot: DiagnosticsSnapshot, hidden = 0): Headline {
  if (snapshot.kind === 'unavailable') {
    return {
      tone: 'unknown',
      text: 'No diagnostics source is running',
      detail: snapshot.reason,
    }
  }
  if (snapshot.kind === 'scanning') {
    const known = snapshot.items ?? []
    return {
      tone: 'unknown',
      text: `Waiting for ${snapshot.source}`,
      // Deliberately not "no problems yet": a partial answer rendered as a clean bill of
      // health is the same failure as the empty list, one round trip earlier.
      //
      // When other sources *have* answered, saying so is what stops a user reading an empty
      // panel as "nothing is wrong" while twelve findings sit behind a filter.
      detail:
        known.length === 0
          ? 'The analyser has not reported yet. Counts appear when it does.'
          : `${snapshot.source} has not reported yet. ${summaryLine(countBySeverity(known))} from the sources that have.`,
    }
  }

  const counts = countBySeverity(snapshot.items)
  if (snapshot.items.length === 0) {
    /*
     * The axiom, applied one layer further in. If the filters hid everything, the workspace is
     * *not* clean — the user chose not to look at what is there — and headlining it as "No
     * problems found" would be the same confident-empty-list failure, produced by the panel
     * itself rather than by a missing analyser.
     */
    if (hidden > 0) {
      return {
        tone: 'unknown',
        text: 'No problems match the current filters',
        detail: `${hidden} hidden by your severity and source settings.`,
      }
    }
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
    detail:
      hidden > 0
        ? `Reported by ${snapshot.source}. ${hidden} more hidden by your settings.`
        : `Reported by ${snapshot.source}.`,
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
 * when it withholds its row count during the first walk, and the one the status bar used to
 * make with `✗ —`: a digit in a counter is a claim.
 */
export function metaFigure(snapshot: DiagnosticsSnapshot): string {
  if (snapshot.kind !== 'ready') return '—'
  // The *true* total, not the number of rows on screen. A cap that reports its prefix as the
  // whole is the same class of quiet lie as an unchecked zero.
  return String(snapshot.items.length + (snapshot.truncated ?? 0))
}

/* ------------------------------------------------------------------- staleness (M18) */

/**
 * What a group whose file has moved since it was checked says about itself.
 *
 * Kept here, beside `headline`, because the wording *is* the feature — the same argument that put
 * `headline` in this module. The reported bug was not that a stale row existed; it was that the
 * row looked exactly like a current one, so clicking it landed the user in a comment with nothing
 * on screen to explain why.
 *
 * Deliberately short. It sits under a group heading in a 252px panel, and a sentence that wraps to
 * three lines above every group would be read once and then ignored for ever.
 */
export const STALE_NOTE = 'Changed since it was checked — positions may be out of date.'

/* -------------------------------------------------------------- the source list (M18) */

/**
 * One row in the panel's footer: an analyser, what it is doing, and whether it can be restarted.
 *
 * # Why this exists at all
 *
 * `SourceReport[]` has been on the wire since M12 and `adapt.ts` has been handing it to the panel
 * that whole time, and the panel drew **none of it**. So `rust-analyzer is not installed` — the
 * one thing that explains an empty list — was carried across the IPC boundary and dropped, and
 * `diagnostics.restart` (which is real, tested Rust, registered in the contract, and wrapped in
 * `client.ts`) had no caller anywhere in the app. That is this repository's named recurring
 * defect: a mechanism designed, documented, and never connected at the last inch.
 *
 * # Why `restartable` is a field and not "always true"
 *
 * `ProjectDiagnostics::restart` returns immediately for anything that is not a process, and
 * tree-sitter and Claude are not processes. A Restart button on those rows would be a control
 * that does nothing when pressed — the exact bug this panel's own header describes as "the bug
 * this whole task is about".
 */
export interface SourceRow {
  /**
   * What `restart` is called with.
   *
   * `rustAnalyzer`, `gopls`, `treeSitter`, `claude` — and, since M22, any language server an
   * extension contributed, under its own binary name, or an `ext:<marketplace>.<extension>` for
   * findings a worker published itself.
   */
  id: string
  /** What the user sees: `rust-analyzer`. Carried on the wire; never rebuilt here. */
  label: string
  /** One line under the label: the progress detail, the reason, or a count. */
  detail: string
  status: SourceStatus['kind']
  /**
   * The server's own progress number while `scanning`, `null` when it did not send one —
   * rust-analyzer's indexing phases do, its `cargo check` does not. The row draws a
   * determinate bar for a number and an indeterminate sweep for `null`; drawing a bar
   * frozen at 0% for the sweep case is the "is it hung?" this field exists to answer.
   */
  percentage: number | null
  /** Whether this source is a process cide can start again. See [`NOT_A_PROCESS`]. */
  restartable: boolean
}

/**
 * The ids that are **not** processes, and therefore the only ones with nothing to restart.
 *
 * A denylist since M22, and the inversion is the whole point. It was `['rustAnalyzer', 'gopls']`,
 * which was a complete list of the language servers cide could run — right up until an extension
 * could contribute one, at which point `sqls` reported findings into the panel and the footer
 * offered no way to restart it. A closed list of servers cannot survive a set that is open, and
 * there is no signal in the row that would have failed loudly; the button was simply absent.
 *
 * `tree-sitter` is cide's own parser and `claude` is a one-shot; everything else that can publish
 * is a process cide spawned and can spawn again. `DiagnosticSourceId::is_server` is the same rule
 * in Rust, and `cmd::ext`'s `source_of` is why an *extension's own* findings — which are published
 * by a worker and not by a process — carry an `ext:` id that is neither of these and is excluded
 * below.
 */
const NOT_A_PROCESS: readonly string[] = ['treeSitter', 'claude']

/** Whether this source is a language server cide started, and can start again. */
function restartable(id: string): boolean {
  return !NOT_A_PROCESS.includes(id) && !id.startsWith('ext:')
}

/**
 * The footer rows, from whichever snapshot arm is in hand.
 *
 * `sources` is optional on every arm — it was added in M12 and every pre-M12 fixture omits it —
 * so a snapshot without it yields no rows and the footer simply is not drawn. That is the right
 * degradation: an empty list of analysers is not a claim, where an invented row would be.
 */
export function sourceRows(snapshot: DiagnosticsSnapshot): SourceRow[] {
  const reports = snapshot.sources ?? []
  return reports.map((report) => ({
    id: report.id,
    label: report.label,
    detail: sourceDetail(report),
    status: report.status.kind,
    percentage: report.status.kind === 'scanning' ? (report.status.percentage ?? null) : null,
    restartable: restartable(report.id),
  }))
}

/**
 * The line under one analyser's name.
 *
 * The `unavailable` reason first and whole, because it is the only actionable sentence the panel
 * ever has — *"rust-analyzer is not on PATH. Install it with `rustup component add
 * rust-analyzer`"* — and truncating it here would be hiding the answer inside the surface that
 * exists to give it.
 */
function sourceDetail(report: SourceReport): string {
  if (report.status.kind === 'unavailable') return report.status.reason
  if (report.status.kind === 'scanning') {
    // The server's own progress line when it has one. `Starting…` rather than an empty string,
    // because a blank line under a name reads as "this row failed to load".
    return report.status.detail === '' ? 'Starting…' : report.status.detail
  }
  if (report.items === 0) return 'Ready — nothing found.'
  return report.items === 1 ? 'Ready — 1 finding.' : `Ready — ${report.items} findings.`
}

/* --------------------------------------------------------------------- the display filter */

/**
 * How much of a buffer's diagnostics are drawn. IDEA's highlighting-level widget.
 *
 * Per *editor*, not global — a 40,000-line generated file is the case it exists for. All three
 * levels keep the grammar: turning highlighting down is about problems, not colour.
 */
export type HighlightLevel = 'none' | 'syntax' | 'all'

/**
 * The user's three axes, as one value.
 *
 * Note what is **not** here: the per-source *process* toggles. Those gate whether an analyser runs
 * at all, upstream in Rust, so by the time a diagnostic reaches this module its source has already
 * decided to speak. [`sources`] below is the *display* half — muting `clippy` inside a
 * rust-analyzer that is still running.
 */
export interface DiagnosticFilters {
  readonly severities: Readonly<Record<Severity, boolean>>
  /**
   * Producer name → shown. **A source absent from the map is shown.**
   *
   * `countBySeverity`'s `other`-bucket rule applied to producers: this is a list of *exceptions*,
   * not a registry, and an analyser nobody has heard of must not be silently hidden. `clippy`
   * arrives inside rust-analyzer's stream under its own name and was never written here by anyone.
   */
  readonly sources: Readonly<Record<string, boolean>>
  readonly level: HighlightLevel
}

/** Everything on, which is what a caller with no settings yet should pass. */
export const ALL_VISIBLE: DiagnosticFilters = {
  severities: { error: true, warning: true, info: true, hint: true },
  sources: {},
  level: 'all',
}

/**
 * Is this diagnostic shown?
 *
 * One predicate, three consumers — the editor's squiggles, the panel's rows, and (through
 * [`applyFilters`] → `statusBarCounts`) the activity rail's ⚠ badge. That is what makes "the
 * toggles affect all three consistently" a property of the code rather than a promise in a
 * comment.
 */
export function visible(item: Diagnostic, filters: DiagnosticFilters): boolean {
  if (filters.level === 'none') return false
  // `Syntax only` reads the producer's own classification rather than guessing from `source`.
  // An item with no `kind` is treated as semantic, which is the conservative direction: it is
  // hidden at this level rather than shown under a claim nobody made.
  if (filters.level === 'syntax' && item.kind !== 'syntax') return false
  if (filters.sources[item.source ?? ''] === false) return false
  // An unrecognised severity is *shown*. Same rule as the `other` bucket: a value from another
  // process must not be filtered out by a table that has not heard of it.
  if (!isSeverity(item.severity)) return true
  return filters.severities[item.severity]
}

/** A snapshot with the hidden items removed, and how many went. */
export interface FilteredSnapshot {
  snapshot: DiagnosticsSnapshot
  hidden: number
}

/**
 * Apply the filters once, so every surface reads the same answer.
 *
 * `unavailable` passes through untouched: filtering cannot manufacture a state where something
 * looked. That is the one transformation this function must never perform.
 */
export function applyFilters(
  snapshot: DiagnosticsSnapshot,
  filters: DiagnosticFilters,
): FilteredSnapshot {
  if (snapshot.kind === 'unavailable') return { snapshot, hidden: 0 }
  const before = snapshot.items ?? []
  const items = before.filter((item) => visible(item, filters))
  const hidden = before.length - items.length
  if (snapshot.kind === 'scanning') {
    return { snapshot: { ...snapshot, items }, hidden }
  }
  return { snapshot: { ...snapshot, items }, hidden }
}
