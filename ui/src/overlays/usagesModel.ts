/**
 * The Find usages popup's arithmetic and its sentences.
 *
 * Pure and import-free so `ui/scripts/check-picker.mjs` can compile it standalone — the same
 * arrangement `overlays/score.ts`, `overlays/format.ts` and `sidebar/SearchModel.ts` are in. What
 * lives here is the half where being wrong is invisible on a screenshot: which of four
 * indistinguishable-looking empty states is showing, and whether a filter that hides every row
 * still says how many there were.
 *
 * What is deliberately **not** here: the grouping and the highlight split. Both already exist, in
 * `sidebar/SearchModel.ts`, and a usage row draws the same thing a search hit does — a file
 * heading and a clipped source line with the occurrence marked. The popup imports `groupHits` and
 * `splitHighlight` rather than growing a second copy; `cide_ipc::Usage` carries `rel`, `text` and
 * byte `start`/`end` for exactly that reason, and the Rust type says so.
 */

/** The fields of the generated `Usage` this module reads. Structural, like `SearchModel`'s `Hit`. */
export interface UsageLike {
  readonly rel: string
  readonly text: string
}

/**
 * Does this row survive the filter box?
 *
 * **Substring, case-insensitive, over the path and the line** — not `overlays/score.ts`'s fuzzy
 * ranking. That scorer is a port of a command-title matcher tuned for short, curated strings;
 * running it over source lines produces confident nonsense, because almost every subsequence of
 * characters occurs somewhere in forty lines of code. A user filtering a usages list is narrowing
 * ("only the ones in `parser.rs`", "only the `mut` ones"), which is what a substring test does.
 *
 * The query is matched against `rel` and `text` **separately**, not against their concatenation,
 * so a query straddling the two — `rs fn` — matches nothing rather than matching by accident.
 */
export function matchesUsage(usage: UsageLike, query: string): boolean {
  const needle = query.trim().toLowerCase()
  if (needle === '') return true
  return (
    usage.rel.toLowerCase().includes(needle) || usage.text.toLowerCase().includes(needle)
  )
}

/** Every row that survives the filter, in order. */
export function filterUsages<T extends UsageLike>(rows: readonly T[], query: string): T[] {
  return rows.filter((row) => matchesUsage(row, query))
}

/**
 * How many files a run of rows covers, counting **consecutive runs**.
 *
 * The same rule `groupHits` groups by, and it has to be: a count derived from a `Set` of paths
 * would disagree with the number of headings drawn the moment a path appeared in two runs, and
 * "12 usages in 3 files" over four headings is the kind of wrong that is only ever noticed by the
 * person who does not trust it.
 */
export function fileCount(rows: readonly { readonly path: string }[]): number {
  let count = 0
  let previous: string | null = null
  for (const row of rows) {
    if (row.path !== previous) count += 1
    previous = row.path
  }
  return count
}

/** `foo` when the caller knows the identifier, and a bare `the symbol` when it does not. */
export function subject(name: string | null): string {
  return name === null || name.trim() === '' ? 'the symbol' : `‘${name}’`
}

/**
 * Which question produced this list. (M18)
 *
 * One popup, two questions. `textDocument/references` and `textDocument/implementation` return the
 * same shape and want the same 0/1/≥2 handling and the same filter box, so building a second
 * overlay would have been two copies of the virtualizer, the keyboard model and the grouping — and
 * two places for them to drift.
 *
 * What must **not** be shared is the prose. A user who pressed Ctrl+Alt+B and reads *"No usages of
 * ‘Reader’ outside its declaration"* has been told their interface is unused, which is a
 * different and alarming claim from *"nothing implements it"*. Every sentence below therefore
 * branches on this, and it is a parameter rather than a second set of functions so that adding a
 * third question means adding a case rather than a file.
 */
export type UsagesKind = 'usages' | 'implementations'

/**
 * The dialog's accessible name and its heading: `Usages of ‘parse’`.
 *
 * `kind` defaults to `'usages'`, which keeps every pre-M18 call site and fixture reading exactly
 * as it did.
 */
export function usagesLabel(name: string | null, kind: UsagesKind = 'usages'): string {
  return kind === 'implementations'
    ? `Implementations of ${subject(name)}`
    : `Usages of ${subject(name)}`
}

/**
 * What the popup's status line says, given everything it knows.
 *
 * One function and not a chain of ternaries in the component, because the four states below look
 * identical on screen — a single grey line of text — and getting two of them the wrong way round
 * is invisible in review and misleading in use. In particular: **"searching" and "found nothing"
 * must never be confused.** A popup that appears empty and then fills reads as "no usages" for
 * however long the search takes, which on a cold rust-analyzer is twenty seconds of a confident
 * lie. `null` means "draw the rows instead".
 */
export function usagesStatus(state: {
  /** The search is still running. */
  readonly searching: boolean
  /** What the source status says it is doing, if anything — `Indexing…`, `Building CrateGraph`. */
  readonly detail?: string | null | undefined
  /** The server's own sentence, when it could not be asked. */
  readonly failed?: string | null | undefined
  /** Rows before the filter. */
  readonly total: number
  /** Rows after it. */
  readonly shown: number
  /** The server offered more than the cap. */
  readonly truncated: boolean
  readonly name: string | null
  readonly query: string
  /** Which question this list answers. Defaults to `'usages'`. (M18) */
  readonly kind?: UsagesKind | undefined
}): string | null {
  const kind = state.kind ?? 'usages'
  const verb = kind === 'implementations' ? 'implementations' : 'usages'
  if (state.failed !== null && state.failed !== undefined && state.failed !== '') {
    return state.failed
  }
  if (state.searching) {
    // The detail is the *server's* word for what it is doing, appended rather than replacing:
    // "Finding usages…" alone makes a twenty-second wait look like a hang, and "Indexing…" alone
    // loses what the user actually asked for.
    const detail = state.detail ?? ''
    return detail === ''
      ? `Finding ${verb} of ${subject(state.name)}…`
      : `Finding ${verb} of ${subject(state.name)}… — ${detail}`
  }
  if (state.total === 0) {
    // Two different claims, and conflating them is the reason `kind` exists at all: "no usages
    // outside its declaration" tells a user their interface is unused, which is not what "nothing
    // implements it" says and is not what they asked.
    return kind === 'implementations'
      ? `Nothing implements ${subject(state.name)}.`
      : `No usages of ${subject(state.name)} outside its declaration.`
  }
  if (state.shown === 0) {
    // Not "No usages": there are usages, and the user's own filter is hiding them. Saying the
    // first would be the app disowning a state the user just created.
    return `No usage matches ‘${state.query.trim()}’.`
  }
  if (state.truncated) {
    return `Showing the first ${state.total} ${verb} — the search stopped at its cap.`
  }
  return null
}

/**
 * The sentence for a Find usages that resolved nothing at all.
 *
 * Kept apart from "used nowhere" above because they are two different facts, and the protocol
 * distinguishes them: `null` from `textDocument/references` means there is no symbol at this
 * position, `[]` means the symbol has no other occurrence. Rendering the first as the second tells
 * a user who pressed ⌥F7 on a keyword that their function is unused.
 */
export function noSymbolSentence(): string {
  return 'There is no declaration under the caret.'
}

/** And the one for a search that came back empty — the notice, not the popup. */
export function noUsagesSentence(name: string | null, kind: UsagesKind = 'usages'): string {
  return kind === 'implementations'
    ? `Nothing implements ${subject(name)}.`
    : `No usages of ${subject(name)} outside its declaration.`
}
