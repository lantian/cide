/**
 * A diff's two sides, tokenized — and the per-row lookup that decides whether a row may use
 * them. (M25)
 *
 * `panes/GitDiffPane.tsx` draws its rows as plain DOM and always drew them in one colour. The
 * grammars that colour a buffer are stream tokenizers over a *whole document*, and this pane
 * has two of them on the wire already (`FileDiff.old_text` / `new_text`, the same pair
 * `diffRows.wholeFileSegments` reconstructs the file from), so colouring a diff is a lookup
 * rather than a second highlighter: `panes/diffHighlight.ts` runs
 * `editor/markdown/fenceTokens.tokenizeFence` over each side and hands the result here.
 *
 * # The guard is the whole point of this module
 *
 * The text and the patch are read in one backend round trip but travel as two claims —
 * `diffRows.ts`' header sets this out at length — and every way they can disagree (CRLF, an
 * ident-expanded keyword, a rev that moved under an open pane, an off-by-one anywhere in this
 * file) lands as a row whose content is not the text's line at that number. Colouring one of
 * those paints a *plausible* lie: a keyword-red `if` on a line that has no `if` in it, with no
 * gap, no artefact and nothing in any log. So [`lineTokens`] compares the row's content against
 * the line the tokens spell and refuses on any difference. One string compare per drawn row buys
 * "every misalignment degrades to plain text", which is the same trade `wholeFileSegments` makes
 * when it answers `null` rather than draw wrong unchanged lines between right changed ones.
 *
 * The comparison is cheap by construction: [`TokenLine.text`] is joined once per fetch, so a
 * paint does one `===` per row and allocates nothing.
 *
 * # Deliberately import-free
 *
 * `scripts/check-diff-render.mjs` compiles this file standalone with the TypeScript in
 * `node_modules` and asserts on the output, the same way it does `diffRows.ts` and
 * `diffBlame.ts` — whose headers explain the trade. It also keeps `GitDiffPane.tsx` able to
 * import this *statically*: the tokenizer proper reaches `@codemirror/language`, and that pane is
 * SSR-bundled under node by the same check, so the grammar half stays behind a dynamic
 * `import()` and only this data shape crosses into the component. [`TokenRow`] is a structural
 * restatement of the fields `diffRows.RowLine` carries, kept honest by the typed call site in
 * `GitDiffPane.tsx`.
 */

/**
 * One coloured run inside a line. `cls` is null for text with no role.
 *
 * A structural restatement of `editor/markdown/fenceTokens.Token`, for the reason above. The two
 * are pinned to each other by `diffHighlight.ts`, which assigns one to the other — so a field
 * added on that side and not this one is a compile error rather than a drift.
 */
export interface DiffToken {
  readonly text: string
  readonly cls: string | null
}

/**
 * One line's runs, and the line they spell.
 *
 * `text` is not redundant with `tokens`: it is what the guard compares against, and precomputing
 * it is what keeps the guard from joining an array on every row of every paint.
 */
export interface TokenLine {
  readonly text: string
  readonly tokens: readonly DiffToken[]
}

/**
 * Both sides of one file, tokenized whole.
 *
 * Either side may be empty — a file added in this revision has no old side, and a side whose
 * grammar would not load is dropped rather than half-coloured. Both empty is spelled `null` by
 * the producer instead, so the pane's prop is either usable or absent.
 */
export interface DiffTokens {
  readonly oldLines: readonly TokenLine[]
  readonly newLines: readonly TokenLine[]
}

/** The fields of a `diffRows.RowLine` this module reads. See the header for why it is restated. */
export interface TokenRow {
  readonly content: string
  readonly oldLineno: number | null
  readonly newLineno: number | null
}

/**
 * Above this, in bytes and on either side, a diff is drawn uncoloured.
 *
 * Between the two limits that already exist, and for a stated reason rather than by splitting the
 * difference. `EditorSurface.HIGHLIGHT_LIMIT_BYTES` is 1 MB, but a buffer tokenizes only what
 * CodeMirror's viewport asks for. `markdown/view.FENCE_HIGHLIGHT_LIMIT_BYTES` is 64 KB, and a
 * fence is eager and viewport-free like this pane — but a fence is an incidental block inside
 * somebody else's document, whereas this text *is* the document the reader opened.
 *
 * So: the editor's own order of magnitude, halved because there are two sides and both are
 * tokenized. The wire cap is 2 MB a side (`cide_git::revision::MAX_BLOB_BYTES`), so this bites
 * first and the pane never waits on a tokenize it would not finish.
 */
export const DIFF_HIGHLIGHT_LIMIT_BYTES = 512 * 1024

/**
 * A tokenizer's per-line runs into lines a row can be looked up in.
 *
 * `raw[i]` must be the runs of `text.split('\n')[i]`, which is what `tokenizeFence` returns.
 * This applies `diffRows.splitLines`' rule to them and nothing else — drop the phantom entry a
 * trailing newline creates, strip one trailing `\r` per line — so that the lines here are
 * numbered exactly as `wholeFileSegments` numbers the same text.
 *
 * **The `\r` strip is load-bearing and is the reason this is not a `map`.** libgit2 filters the
 * worktree side of a diff, so a CRLF file's hunk lines arrive with the `\r` already gone while
 * the text is raw disk bytes. Without the strip every line of such a file would spell one
 * character more than the row it describes, the guard in [`lineTokens`] would refuse all of
 * them, and a Windows-authored file would silently never be coloured — a failure with no symptom
 * except an absence.
 */
export function tokenLines(raw: readonly (readonly DiffToken[])[]): TokenLine[] {
  const out: TokenLine[] = []
  for (let i = 0; i < raw.length; i++) {
    const tokens = raw[i] ?? []
    let text = ''
    for (const token of tokens) text += token.text
    // The phantom entry a trailing newline creates: `"a\n".split('\n')` is `['a', '']`, and
    // `splitLines` pops exactly that one. Only the last entry, and only when it is empty — a
    // blank line in the middle of a file is a real line.
    if (i === raw.length - 1 && text === '' && raw.length > 1) break
    if (text.endsWith('\r')) {
      // Strip the `\r` from the last run that has one, not from the joined string alone, or the
      // runs would spell one character more than `text` says they do.
      const trimmed: DiffToken[] = tokens.map((token) => token)
      const last = trimmed[trimmed.length - 1]
      if (last !== undefined) trimmed[trimmed.length - 1] = { text: last.text.slice(0, -1), cls: last.cls }
      out.push({ text: text.slice(0, -1), tokens: trimmed })
      continue
    }
    out.push({ text, tokens })
  }
  return out
}

/**
 * The runs to draw this row with, or `null` for "draw it plain".
 *
 * New side first, old side second. A context row carries both numbers and one content — it is a
 * single entry in the unified diff, drawn once or twice — so either side answers and the new one
 * is preferred; an addition has only `newLineno`; a deletion has only `oldLineno`, and that is
 * the whole reason both sides are tokenized rather than just the one the reader is reviewing.
 *
 * `null` rather than a throw when there are no tokens at all, matching `diffBlame.blameFor`: this
 * is called once per drawn row from inside a render, and the absent case is the common one.
 */
export function lineTokens(
  tokens: DiffTokens | null,
  row: TokenRow,
): readonly DiffToken[] | null {
  if (tokens === null) return null
  const fromNew = at(tokens.newLines, row.newLineno)
  if (fromNew !== null && fromNew.text === row.content) return fromNew.tokens
  const fromOld = at(tokens.oldLines, row.oldLineno)
  if (fromOld !== null && fromOld.text === row.content) return fromOld.tokens
  return null
}

/**
 * One line of a side, by its 1-based number, or `null`.
 *
 * `0` is not a line — git numbers a zero-length range by the line *before* it — and past-the-end
 * is what a rev that moved under an open pane looks like. Both fall out of the bounds test rather
 * than being special-cased, and `--noUncheckedIndexedAccess` makes the `undefined` explicit.
 */
function at(lines: readonly TokenLine[], lineno: number | null): TokenLine | null {
  if (lineno === null || lineno < 1) return null
  return lines[lineno - 1] ?? null
}
