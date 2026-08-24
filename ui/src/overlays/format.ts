/**
 * Text the overlays and the sidebar render verbatim from the mock.
 *
 * Pure and import-free, so `ui/scripts/check-picker.mjs` can compile it standalone — the
 * counter is a string the mock states exactly (`6 of 2,418`) and getting the grouping wrong
 * is the kind of thing that reads fine in review and is wrong on screen.
 */

/**
 * Group a non-negative integer with commas: `2418` → `2,418`.
 *
 * Hand-rolled rather than `toLocaleString`, which is locale-sensitive: under a `de-DE`
 * system locale it produces `2.418`, and under `en-IN` it produces `2,418` for this number
 * but `2,41,801` for the next one up. The mock states a comma; a desktop app whose counter
 * changes shape with `LC_ALL` is not transcribing the mock, it is guessing.
 */
export function groupDigits(value: number): string {
  const whole = Math.trunc(Math.abs(value))
  const sign = value < 0 ? '-' : ''
  const digits = String(whole)

  let out = ''
  for (let i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 === 0) out += ','
    out += digits[i]
  }
  return sign + out
}

/** The picker's right-aligned counter: `6 of 2,418`. */
export function matchCounter(shown: number, total: number): string {
  return `${groupDigits(shown)} of ${groupDigits(total)}`
}

/** The name after the last path separator. */
export function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1)
}

/** Everything before the last separator, without a trailing one. `''` for a bare name. */
export function dirname(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut <= 0 ? '' : path.slice(0, cut)
}

/** Colour roles a kind badge can take. Each maps to one token; see `Overlay.module.css`. */
export type BadgeTone =
  | 'accent'
  | 'blue'
  | 'cyan'
  | 'yellow'
  | 'green'
  | 'purple'
  | 'dim'
  | 'faint'

export interface KindBadge {
  label: string
  tone: BadgeTone
}

/**
 * The mock's language badge table, keyed by extension.
 *
 * Duplicated from `chrome/TabStrip.tsx` rather than imported. Not because duplication is
 * good: that copy returns CSS Module class names, and class names are scoped to the module
 * that declares them, so importing it would hand this component a `TabStrip.module.css`
 * class that does not exist in `Overlay.module.css`. Returning a *tone name* instead is what
 * makes this one shareable, and if the tab strip is ever refactored the two should meet
 * here. The table itself is the mock's and is stated in the plan, so both copies have the
 * same source of truth to be checked against.
 */
export function kindBadge(path: string): KindBadge {
  const name = basename(path)
  const dot = name.lastIndexOf('.')
  // `> 0` rather than `>= 0`: a leading dot makes a hidden file, not an extension, so
  // `.gitignore` falls through to the neutral marker instead of claiming a GITIGNORE badge.
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : ''

  switch (ext) {
    case 'rs':
      return { label: 'RS', tone: 'accent' }
    case 'ts':
      return { label: 'TS', tone: 'blue' }
    case 'tsx':
      return { label: 'TSX', tone: 'cyan' }
    case 'js':
      return { label: 'JS', tone: 'yellow' }
    case 'toml':
      return { label: 'TOML', tone: 'green' }
    case 'lock':
      return { label: 'LOCK', tone: 'faint' }
    case 'md':
      return { label: 'MD', tone: 'dim' }
    case 'yml':
    case 'yaml':
      return { label: 'YML', tone: 'purple' }
    case 'json':
      return { label: 'JSON', tone: 'yellow' }
    case 'sh':
      return { label: 'SH', tone: 'green' }
    case 'css':
      return { label: 'CSS', tone: 'purple' }
    case 'html':
      return { label: 'HTML', tone: 'accent' }
    default:
      return { label: '·', tone: 'faint' }
  }
}

/**
 * The badge for a symbol's kind — `FN`, `ST`, `TR`.
 *
 * Here rather than in either picker, so the File Structure popup and Go-to-Symbol cannot label
 * the same kind differently, and so `check:picker` compiles it for free.
 *
 * Takes `string` rather than `SymbolKind` deliberately, for the reason `isSeverity` in
 * `ProblemsPanel/model.ts` does: every caller has a value *annotated* by a wire type, and the
 * whole point of the fallback is that the annotation is a promise from another process. An
 * unrecognised kind gets the neutral marker and is **never dropped** — the same `other`-bucket
 * rule, applied to a glyph.
 */
export function symbolBadge(kind: string): KindBadge {
  switch (kind) {
    case 'function':
      return { label: 'FN', tone: 'blue' }
    case 'method':
      return { label: 'FN', tone: 'blue' }
    case 'struct':
      return { label: 'ST', tone: 'cyan' }
    case 'enum':
      return { label: 'EN', tone: 'cyan' }
    case 'union':
      return { label: 'UN', tone: 'cyan' }
    case 'interface':
      return { label: 'IF', tone: 'purple' }
    case 'trait':
      return { label: 'TR', tone: 'purple' }
    case 'impl':
      return { label: 'IM', tone: 'dim' }
    case 'module':
      return { label: 'MOD', tone: 'green' }
    case 'package':
      return { label: 'PKG', tone: 'green' }
    case 'constant':
      return { label: 'CO', tone: 'yellow' }
    case 'static':
      return { label: 'ST8', tone: 'yellow' }
    case 'variable':
      return { label: 'VAR', tone: 'yellow' }
    case 'typeAlias':
      return { label: 'TY', tone: 'cyan' }
    case 'macro':
      return { label: 'MA', tone: 'accent' }
    case 'field':
    case 'variant':
      return { label: '·', tone: 'faint' }
    default:
      return { label: '·', tone: 'faint' }
  }
}

/**
 * The badge for a completion's kind — `FN`, `ST`, `KW`. (M25)
 *
 * Beside [`symbolBadge`] and deliberately **not** the same function, because the two vocabularies
 * genuinely differ: an outline has `Impl`, which no server ever offers as a completion, and a
 * completion list is a third keywords, snippets and text, none of which an outline contains.
 * Folding them together would have meant a lossy mapping with `·` doing most of the work.
 *
 * What they *do* share is the tone vocabulary and the rule at the bottom, and sharing those is
 * the point of the two living in one file: a `function` is blue in the popup, in the File
 * Structure list and in Go-to-Symbol, or the same symbol reads as three different things.
 *
 * `string` and not `CompletionKind`, for [`symbolBadge`]'s reason: the value is annotated by a
 * wire type, which is a promise from another process, and the fallback exists precisely for when
 * that promise is not kept. An unrecognised kind gets the neutral marker and is **never dropped** —
 * a completion you cannot label is still a completion.
 *
 * The kinds cide's `CompletionKind` can produce are all here; the `default` is for a build of the
 * frontend that is older than the Rust that fed it.
 */
export function completionBadge(kind: string): KindBadge {
  switch (kind) {
    case 'function':
    case 'method':
      return { label: 'FN', tone: 'blue' }
    case 'constructor':
      return { label: 'NEW', tone: 'blue' }
    case 'field':
    case 'property':
      return { label: 'FLD', tone: 'faint' }
    case 'variable':
      return { label: 'VAR', tone: 'yellow' }
    case 'constant':
      return { label: 'CO', tone: 'yellow' }
    case 'struct':
      return { label: 'ST', tone: 'cyan' }
    case 'enum':
      return { label: 'EN', tone: 'cyan' }
    case 'enumMember':
      return { label: 'EM', tone: 'cyan' }
    case 'typeParameter':
      return { label: 'TY', tone: 'cyan' }
    case 'interface':
      return { label: 'IF', tone: 'purple' }
    case 'module':
      return { label: 'MOD', tone: 'green' }
    case 'keyword':
      return { label: 'KW', tone: 'purple' }
    case 'snippet':
      return { label: 'SNP', tone: 'accent' }
    case 'operator':
      return { label: 'OP', tone: 'dim' }
    case 'event':
      return { label: 'EV', tone: 'accent' }
    case 'file':
      return { label: 'FIL', tone: 'green' }
    case 'folder':
      return { label: 'DIR', tone: 'green' }
    case 'text':
      return { label: 'TXT', tone: 'faint' }
    default:
      return { label: '·', tone: 'faint' }
  }
}

/**
 * What the file picker says when it has no rows to draw. (M16)
 *
 * # Why this is a function and not a ternary in the JSX it came out of
 *
 * It was a ternary, with three arms, and the library scope makes it four — at which point it
 * is a *rule*: two of the four are empty answers that look identical and mean opposite things
 * ("still filling, wait" versus "finished, and there is nothing"), and a third is an empty
 * answer with a cause the user can act on. That is precisely the shape this project has
 * shipped inverted before, and a rule inside JSX is a rule no check script can compile.
 *
 * The four states, and the input that decides each:
 *
 * | state | when |
 * | --- | --- |
 * | `indexing` | a walk is filling either matcher, or the project has not started one |
 * | `typeToSearch` | the query is empty and nothing is running |
 * | `noLibraries` | libraries are in scope, nothing is running, and **nothing was resolved** |
 * | `noMatches` | everything else |
 *
 * `noLibraries` is checked before `noMatches` and after `typeToSearch`, and both orderings
 * matter. Before, because "No matches" over a project whose dependencies were never resolved
 * blames the query for an empty candidate set — the failure `libraries.rs` already refuses to
 * ship ("an empty group is indistinguishable from a broken one"). After, because an empty
 * query has not searched for anything yet and has no business reporting on the library scope.
 *
 * `total === 0` and not `total === projectFiles`: the merged `total` counts both sides, so a
 * project with files of its own can never reach zero here — which is right. Nothing resolved
 * is only worth saying when there is nothing at all to say anything else about, and a project
 * with 800 files of its own and no dependencies gets "No matches", which is true.
 */
export type PickerEmptyState = 'indexing' | 'typeToSearch' | 'noLibraries' | 'noMatches'

export function pickerEmptyState(input: {
  /** Either matcher is still filling. */
  running: boolean
  /** The project's walk has not started; distinct from a walk that is running. */
  awaitingIndex: boolean
  query: string
  /** Is the library scope switched on? */
  libraries: boolean
  /** The merged candidate count, or `null` before any frame has arrived. */
  total: number | null
}): PickerEmptyState {
  if (input.running || input.awaitingIndex) return 'indexing'
  if (input.query === '') return 'typeToSearch'
  if (input.libraries && input.total === 0) return 'noLibraries'
  return 'noMatches'
}

/** The sentence for each state. Separated so the rule above can be driven without the words. */
export function pickerEmptyText(state: PickerEmptyState): string {
  switch (state) {
    // One word for two walks, deliberately. Which of the two indexes is still filling is
    // cide's bookkeeping; the counter beside it is already climbing.
    case 'indexing':
      return 'Indexing…'
    case 'typeToSearch':
      return 'Type to search'
    case 'noLibraries':
      return 'No external libraries resolved for this project'
    case 'noMatches':
      return 'No matches'
  }
}
