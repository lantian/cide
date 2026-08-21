/**
 * What an extension can learn about an open file, and what it can put back. (M22)
 *
 * # The one seam, in both directions
 *
 * Out: which file is focused, what language it is, what its text is, where the caret is. In: an
 * outline, diagnostics, and a request to move the caret. Every one of them is capability-gated in
 * `host.ts`, on the main thread, where the extension cannot reach.
 *
 * # Why the outgoing half is debounced here rather than in the editor
 *
 * `noteEditor` posts the buffer's whole text to every worker with `editor:read`. Called per
 * keystroke on a large file that is a megabyte of `postMessage` per keypress, on the one
 * JavaScript thread ADR 0001 spends its whole length protecting — which is the single most likely
 * way an extension host makes typing slow. So the debounce is here, in the one place every
 * producer goes through, rather than left for each call site to remember.
 *
 * 300 ms, the same number `editor/outlineStore.ts` re-parses on and for the same reason: it is
 * long enough that a burst of typing is one message and short enough that a pause feels like the
 * panel keeping up.
 */
import { ext as extApi } from '@/ipc/client'
import type { Diagnostic, ProjectId, Symbol as OutlineSymbolDto } from '@/ipc/client'
import { publishExtOutline } from '@/editor/outlineStore'
import type { OutlineSymbol, WorkerDiagnostic } from './protocol'
import {
  noteEditor,
  registerDiagnosticsSink,
  registerOutlineSink,
  registerReveal,
} from './host'

/** Matches `outlineStore`'s re-parse debounce — see the module header. */
const DEBOUNCE_MS = 300

let timer: ReturnType<typeof setTimeout> | null = null
let project: ProjectId | null = null

/**
 * Tell the workers which file is focused, after the user stops typing.
 *
 * `read` is called when the timer fires, not now: reading a `Text` out of CodeMirror is cheap but
 * not free, and a caller that read eagerly would pay for it on every keystroke to throw the
 * result away.
 */
export function noteActiveEditor(
  active: { path: string; languageId: string | null; line: number; read: () => string } | null,
): void {
  if (timer !== null) clearTimeout(timer)
  if (active === null) {
    // Immediately, not debounced. "No file is focused" is a state a panel should reflect at once —
    // a stale outline under a heading for a file that is no longer open is worse than an empty one.
    noteEditor(null)
    return
  }
  timer = setTimeout(() => {
    timer = null
    noteEditor({
      path: active.path,
      languageId: active.languageId,
      text: active.read(),
      line: active.line,
    })
  }, DEBOUNCE_MS)
}

/**
 * Wire the host's single-slot seams to this window's editor and stores.
 *
 * One slot each rather than a listener list, on `chrome/panelRequests.ts`'s pattern: there is
 * exactly one outline store and one reveal path per window, and a list would let two of them
 * answer the same request differently.
 *
 * Returns the teardown. `App.tsx` calls it, once, beside the extension store's own attach.
 */
export function attachEditorBridge(reveal: (path: string, line: number, column: number) => void): () => void {
  registerReveal(reveal)

  registerOutlineSink((id, path, symbols) => {
    publishExtOutline(path, `${id.marketplace}.${id.extension}`, symbols.map(toDto))
  })

  registerDiagnosticsSink((id, path, items) => {
    const current = project
    if (current === null) return
    void extApi
      .publishDiagnostics(current, id, path, items.map((item) => toDiagnostic(path, item)))
      .catch(() => {
        // A project closed while a worker was thinking. Nothing an extension can do about it, and
        // a rejection here would surface as an exception in somebody else's code.
      })
  })

  return () => {
    registerReveal(null)
    registerOutlineSink(null)
    registerDiagnosticsSink(null)
    if (timer !== null) clearTimeout(timer)
    timer = null
  }
}

/**
 * Which project a worker's findings belong to.
 *
 * Held here rather than sent with every note, because a worker has no idea what a project is and
 * should not be told: it is given a file path and answers about that file. `App.tsx` sets it
 * whenever the active project changes.
 */
export function setBridgeProject(id: ProjectId | null): void {
  project = id
}

/** A worker's symbol, as `cide_ipc::Symbol`. */
function toDto(symbol: OutlineSymbol): OutlineSymbolDto {
  const end = symbol.endLine ?? symbol.line
  const span = {
    startLine: symbol.line,
    // Column 1: a worker describes a *line*, not a range, because a line is what it can be sure of
    // — it has the text and no parse tree, and a column it guessed would put the breadcrumb's
    // highlight in the wrong place. The consumers all use the line.
    startColumn: 1,
    endLine: end,
    endColumn: 1,
  }
  return {
    // Every contributed symbol is a `Variable`. The kind drives an icon and nothing else, and
    // there is no honest way to map an extension's idea of a statement onto cide's seventeen
    // tree-sitter kinds — a `Function` icon on a `CREATE TABLE` would be a confident wrong answer.
    kind: 'variable',
    name: symbol.name,
    detail: symbol.detail ?? null,
    container: null,
    range: span,
    selection: span,
    children: (symbol.children ?? []).map(toDto),
  }
}

/** A worker's finding, as `cide_ipc::Diagnostic`. */
function toDiagnostic(path: string, item: WorkerDiagnostic): Diagnostic {
  return {
    // `path` twice, and the first is a placeholder: the panel groups on the relative one and acts
    // on the absolute one, and a worker only ever sees the absolute path — it is what it was
    // handed. `ProjectDiagnostics::publish_external` derives the relative form from the project's
    // roots, which is what puts a contributed row under the same heading as a rust-analyzer one
    // for the same file.
    path,
    absPath: path,
    line: item.line,
    column: item.column ?? 1,
    endLine: item.endLine ?? item.line,
    endColumn: item.endColumn ?? (item.column ?? 1) + 1,
    severity: item.severity,
    // `semantic`, always. `DiagnosticKind` is set by the producer and never inferred by the UI —
    // and `syntax` means "the shape of the text is wrong", which is what the *Syntax only*
    // highlighting level filters on. An extension reporting a schema violation is not that.
    kind: 'semantic',
    message: item.message,
    source: '',
    code: item.code ?? null,
    stale: false,
  }
}
