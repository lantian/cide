/**
 * The wire's `DiagnosticsSnapshot` into the panel's.
 *
 * # Why there are two of them at all
 *
 * `model.ts` **imports nothing** — that is what lets `check-problems.mjs` compile it alone with a
 * bare `tsc` and drive it under node, which is the whole of this panel's test story. So it cannot
 * import `generated.ts`, and it restates the shape structurally instead. This module is the one
 * place the two meet, and it is allowed to import both.
 *
 * # What actually differs, and why it is not laziness
 *
 * Two things, and both are deliberate on their own side:
 *
 * * **`code`.** The wire says `string | null`, because every outbound optional in `cide-ipc` is
 *   `T | null` — `#[ts(optional)]` changes only the emitted type and not what serde writes, so on
 *   an outbound DTO it would promise an absent field and send a null one. The panel says
 *   `code?: string | undefined`, because that is what its fixtures have always been. Under
 *   `exactOptionalPropertyTypes` those are genuinely different types, and the conversion is a
 *   `?? undefined`.
 * * **`sources`.** The wire's `SourceStatus` is a tagged union with the same three names as the
 *   snapshot's own `kind`, which is convenient and would be confusing to collapse.
 *
 * Everything else passes through untouched, which is the point: this is a conversion, not a
 * second model. If it ever grows a *decision* — a default, a fallback, a piece of wording — that
 * decision belongs in `model.ts` where the check script can reach it.
 */
import type {
  Diagnostic as WireDiagnostic,
  DiagnosticsSnapshot as WireSnapshot,
  SourceReport as WireSourceReport,
} from '@/ipc/client'
import type { Diagnostic, DiagnosticsSnapshot, SourceReport } from './model'

function item(from: WireDiagnostic): Diagnostic {
  return {
    path: from.path,
    absPath: from.absPath,
    line: from.line,
    column: from.column,
    endLine: from.endLine,
    endColumn: from.endColumn,
    severity: from.severity,
    kind: from.kind,
    message: from.message,
    source: from.source,
    // `null` on the wire, absent in the panel. See the header.
    code: from.code ?? undefined,
  }
}

function report(from: WireSourceReport): SourceReport {
  return {
    id: from.id,
    label: from.label,
    status: from.status,
    items: from.items,
  }
}

/** The snapshot as the panel reads it. */
export function adaptSnapshot(from: WireSnapshot): DiagnosticsSnapshot {
  switch (from.kind) {
    case 'unavailable':
      return { kind: 'unavailable', reason: from.reason, sources: from.sources.map(report) }
    case 'scanning':
      return {
        kind: 'scanning',
        source: from.source,
        items: from.items.map(item),
        sources: from.sources.map(report),
      }
    case 'ready':
      return {
        kind: 'ready',
        source: from.source,
        items: from.items.map(item),
        sources: from.sources.map(report),
        truncated: from.truncated,
      }
  }
}
