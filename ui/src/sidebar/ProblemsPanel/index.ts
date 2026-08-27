/**
 * The ⚑ Problems sidebar view.
 *
 * ## Wiring it into the shell
 *
 * The rail already emits `'problems'`; `App.tsx` renders nothing for it, which is why the
 * button looks broken. One line next to the other two sidebar views fixes that:
 *
 * ```tsx
 * import { ProblemsPanel } from '@/sidebar/ProblemsPanel'
 * ...
 * {view === 'files' && <Explorer … />}
 * {view === 'git' && <GitPanel project={activeProjectId} />}
 * {view === 'problems' && <ProblemsPanel project={activeProjectId} />}
 * ```
 *
 * No `snapshot` prop: omitting it *is* the v1 answer (`NO_SOURCE` — nothing is analysing this
 * workspace). Passing `snapshot={NO_SOURCE}` explicitly would say the same thing twice.
 *
 * ## Keeping the rail's badge in step
 *
 * `statusBarCounts` is the bridge: `App.tsx` derives `diagCounts` from the same filtered
 * snapshot the panel renders and hands its `errors` to the rail's ⚠ button, so the badge and
 * the list cannot disagree about whether the workspace is clean. `null` — nobody looked —
 * draws no badge, which is distinct from a badge reading zero.
 *
 * The status bar was its second consumer and is not any more (M28): `✗ n ⚠ n` beside the
 * branch was the same figure one row down, and it went with the diff counters next to it. The
 * name of the function is the last trace of that; it is kept because the ids are what the
 * check script imports.
 *
 * ## When a language server lands
 *
 * Replace the default with a live snapshot and nothing else here changes — the list, the
 * per-file grouping and the click-to-open path are already written and already tested
 * (`ui/scripts/check-problems.mjs`). The three states in `DiagnosticsSnapshot` are the whole
 * contract: `unavailable`, `scanning`, `ready`.
 *
 * `model.ts` is deliberately not re-exported through anything that pulls in React: the check
 * script compiles that one module with `tsc` and imports the output under node, the same
 * arrangement `GitPanel/model.ts` has.
 */
export { ProblemsPanel, type ProblemsPanelProps } from './ProblemsPanel'
export {
  NO_DIAGNOSTICS_SOURCE,
  NO_SOURCE,
  STALE_NOTE,
  checked,
  countBySeverity,
  groupByFile,
  headline,
  isSeverity,
  metaFigure,
  severityRank,
  sourceRows,
  statusBarCounts,
  summaryLine,
  type Diagnostic,
  type DiagnosticsSnapshot,
  type FileGroup,
  type Headline,
  type HeadlineTone,
  type Severity,
  type SeverityCounts,
  type SourceRow,
} from './model'
