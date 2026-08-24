/**
 * Every *decision* code completion makes, in a module that imports nothing.
 *
 * The same split `codeIntelGate.ts` makes against `codeIntel.ts`, for the reason that file states
 * and this feature demonstrates again: the rules here are the ones that are wrong in ways no
 * screenshot shows — a popup that stops narrowing as you type, a cached list believed one
 * keystroke too long, an auto-import row accepted without its import. All of them live inside a
 * CodeMirror extension in the obvious implementation, which is to say inside something no check
 * script can run. Here, `ui/scripts/check-completion.mjs` compiles this file on its own and
 * drives all of it.
 *
 * What is next door in `completion.ts`: the IPC, the document sync, the transactions and the
 * keymap — the parts a headless check could not exercise anyway.
 */

/**
 * The shape of one row as this module sees it.
 *
 * A structural restatement of `cide_ipc::CompletionItem` rather than an import of the generated
 * type, because importing it would end the standalone compile that is this module's whole point.
 * `check-completion.mjs` compares the two field lists, the way `check-outline.mjs` does for
 * `memberNav.ts`, so the restatement cannot drift silently.
 */
export interface CompletionRow {
  readonly label: string
  readonly filterText: string
  readonly detail: string | null
  readonly description: string | null
  readonly kind: string
  readonly insert: string
  readonly snippet: boolean
  readonly sort: number
  readonly resolve: number | null
  readonly deprecated: boolean
}

/**
 * How long a burst of typing settles before the server is asked.
 *
 * The same 150 ms `codeIntel.ts` settles a hover for, and picked for the same reason: it is about
 * the shortest delay that is not perceived as a lag. It is also doing more work here than there —
 * at a comfortable typing speed it collapses a whole word into one request instead of six, and
 * every request it saves is a whole-crate candidate search rust-analyzer does not run.
 *
 * Deliberately **not** a setting. `EditorSettings` carries whether the popup opens on typing at
 * all, which is a question a user has an opinion about; a millisecond box is one they do not, and
 * `EditorSettings::autosave` already writes down why the interval stays in TypeScript while the
 * yes/no crosses the wire.
 */
export const TYPING_DELAY_MS = 150

/**
 * Characters that are worth asking about even with no word typed yet.
 *
 * # Why this list exists when the server has its own
 *
 * The server's `triggerCharacters` list is authoritative and it lives in Rust, where it decides
 * the LSP `context` — see `ProjectDiagnostics::completion`. This list answers a different
 * question, one hop earlier: *is this keystroke worth a round trip at all?* Asking Rust that
 * would mean the round trip has already happened.
 *
 * So it is a deliberate **superset** of what the servers cide ships declare — rust-analyzer's
 * `.` and `::`, gopls's `.`, plus the ones a contributed server plausibly wants (`/` and the
 * quotes for a path or a YAML key, `<` for a generic, `@` and `#` for a decorator or a directive).
 * Being wide costs a request that comes back empty; being narrow costs a feature that silently
 * does not work for somebody's language. The first is recoverable and the second is a bug report
 * that reads *"completion doesn't work in X"* with nothing to see.
 *
 * `:` covers `::` on its own — the caret is after the second colon and the character before it is
 * a colon either way.
 */
const TRIGGERS = new Set(['.', ':', '>', '/', '"', "'", '<', '@', '#', '$', '-'])

/**
 * Is the character one that can appear inside an identifier?
 *
 * `$` and `_` are in, because they are identifier characters in most of what cide highlights, and
 * digits are in because they are legal after the first character. Deliberately **not** a
 * `\p{L}`-only test: a Rust `r#type`, a PHP `$var` and a JS `$` would each stop being a prefix.
 */
function isWordChar(char: string): boolean {
  return /[\p{L}\p{N}_$]/u.test(char)
}

/**
 * Should this position be asked about at all?
 *
 * Three ways to say yes, and the third is the one that is easy to leave out:
 *
 * * **the user asked.** Ctrl+Space is explicit and is never second-guessed — including on an
 *   empty line, which is where it is most useful and where every prefix test says no.
 * * **there is a word in progress.** The ordinary case.
 * * **the character behind the caret opens one.** `.`, `::`, `/`. Without this the popup would
 *   never appear on the gesture people actually make, because immediately after a `.` there is no
 *   prefix and the second test is false.
 *
 * Everything else — whitespace, a closing brace, a semicolon — is a no. That is not a
 * micro-optimisation: `activateOnTyping` fires on every input, so without this every space bar
 * press would start a whole-crate candidate search that is thrown away.
 */
export function shouldAsk(before: string, explicit: boolean): boolean {
  if (explicit) return true
  const last = before.slice(-1)
  if (last === '') return false
  return isWordChar(last) || TRIGGERS.has(last)
}

/**
 * Where the word being completed starts, as an offset back from the caret.
 *
 * CodeMirror needs a `from` for the range its own filtering matches against. This walks back over
 * identifier characters only, so after a `.` it returns the caret itself — an empty range, which
 * is right: the member name has not been started, and a `from` that swallowed the dot would make
 * CodeMirror match every candidate against `.foo` and score them all as misses.
 */
export function wordStart(before: string): number {
  let at = before.length
  while (at > 0 && isWordChar(before[at - 1] as string)) at -= 1
  return at
}

/** What `completion.ts` hands CodeMirror for one row, minus the parts that need a view. */
export interface MappedCompletion {
  readonly label: string
  readonly displayLabel: string
  /**
   * Absent, not `undefined`, when the server sent none.
   *
   * `exactOptionalPropertyTypes` is on in this project and CodeMirror's `Completion.detail` is
   * `detail?: string`, so an explicit `undefined` is a type error rather than a shrug. Building
   * the key conditionally is also the honest shape: "the server said nothing" and "the server
   * said nothing, and here is a slot holding that" are the same fact told twice.
   */
  readonly detail?: string
  readonly type: string
  readonly boost: number
}

/**
 * One row into the shape CodeMirror scores and draws.
 *
 * # `label` is the filter text, and `displayLabel` is the label
 *
 * This looks backwards and is the single most consequential line in the file. CodeMirror matches
 * typing against `Completion.label` and draws `displayLabel` when one is present. LSP's `label`
 * is a *display* string — rust-analyzer sends `push(…)`, ellipsis included — and its `filterText`
 * is what typing is meant to match. Feeding the display string to the matcher scores `push(…)`
 * against `pus`, ranks it below unrelated rows, and produces the specific complaint that the
 * popup "stops narrowing when you type", with everything still visibly present.
 *
 * # `boost` carries the server's ranking, compressed
 *
 * The server has already sorted by relevance and `sort` is that rank. CodeMirror's `boost` is
 * capped at ±99 and is added to its own fuzzy score, so it cannot carry a rank of 1500 and must
 * not try: the intent is *"break ties the way the server would"*, not "override the matcher".
 * The first `BOOST_DEPTH` rows get a descending nudge and everything past them gets none, which
 * keeps rust-analyzer's genuinely-good first suggestions on top while leaving the fuzzy match in
 * charge of anything the user has actually typed towards.
 *
 * A deprecated row is pushed to the bottom of the boost range rather than hidden. Hiding it would
 * be an editor deciding a user may not call a function they can see in their own dependency.
 */
export function mapCompletion(row: CompletionRow): MappedCompletion {
  const boost = row.deprecated
    ? -99
    : row.sort < BOOST_DEPTH
      ? Math.round(((BOOST_DEPTH - row.sort) / BOOST_DEPTH) * BOOST_CEILING)
      : 0
  return {
    label: row.filterText,
    displayLabel: row.label,
    // `detail` and not `description`: this is the string CodeMirror draws hard against the label,
    // and for an auto-import row it is the only visible warning — `(use std::collections::HashMap)`
    // — that accepting will also edit the top of the file. The type is drawn separately.
    ...(row.detail !== null ? { detail: row.detail } : {}),
    type: row.kind,
    boost,
  }
}

/** How many of the server's top rows get a boost at all. */
const BOOST_DEPTH = 40

/**
 * The largest boost handed out. Well under CodeMirror's ±99 limit, on purpose.
 *
 * The remaining headroom is what keeps a boost from being able to outrank the fuzzy score
 * outright: a row the user has typed six matching characters towards should win over the
 * server's first suggestion, and at a ceiling of 99 it would not.
 */
const BOOST_CEILING = 50

/**
 * May CodeMirror keep filtering this list locally as the user types on?
 *
 * `false` means every further keystroke re-queries the server. Two independent reasons to refuse,
 * and both have to be checked:
 *
 * * **`incomplete`** — LSP's `isIncomplete`. The server computed this list *for this prefix* and
 *   is telling us it will answer differently for a longer one. rust-analyzer sets it constantly,
 *   because its candidate set genuinely changes shape as a prefix narrows.
 * * **`truncated`** — cide dropped rows at `MAX_COMPLETIONS`. The rows that would match what the
 *   user is about to type may be among the ones that were dropped, so filtering what is left
 *   would confidently show a subset and look complete.
 *
 * Getting this wrong is invisible in the common case and wrong in the interesting one: the popup
 * keeps working, and simply stops offering the thing the user is typing towards.
 */
export function canFilterLocally(incomplete: boolean, truncated: boolean): boolean {
  return !incomplete && !truncated
}

/**
 * Does accepting this row need a round trip first?
 *
 * `resolve` is non-null exactly when the server deferred this row's edits — in practice the
 * auto-import candidates, which for rust-analyzer is the only way they exist at all (it offers
 * none to a client that cannot resolve lazily). Everything else accepts with no request.
 */
export function needsResolve(row: CompletionRow): boolean {
  return row.resolve !== null
}

/**
 * What to say when a resolve fails, given the row.
 *
 * A sentence and not silence, uniquely among this feature's failure paths. Every other one is
 * behind a popup that opened by itself and is dropped without a word; this one is behind a **Tab
 * the user pressed**, and the accept has been refused. Saying nothing would read as the key being
 * broken.
 *
 * The refusal itself is the important half and it is `completion.ts`'s: inserting `HashMap`
 * without its `use` line leaves the file not compiling, which is worse than not completing.
 */
export function resolveFailureSentence(label: string, reason: string): string {
  return `Could not complete ${label}: ${reason}`
}
