/**
 * Colouring a diff with the buffer's own grammars. (M25)
 *
 * The half of diff highlighting that touches CodeMirror, kept in its own module for one reason:
 * **nothing may import it statically.** `panes/GitDiffPane.tsx` is SSR-bundled and executed under
 * node by `scripts/check-diff-render.mjs`, which is how the claim that what the pane highlights
 * is what it stages gets measured; `editor/diffViewMode.ts` records at length why that bundle is
 * kept free of the editor's dependencies. So the pane reaches this through a dynamic `import()`
 * inside an effect — an effect `renderToStaticMarkup` never runs — and everything that crosses
 * back is the plain data in `panes/diffTokens.ts`.
 *
 * That is a durability decision rather than a fix for a crash, and it is worth saying which:
 * these packages *do* import cleanly under node today, and `check:markdown` already compiles and
 * runs `fenceTokens.ts` there to prove it. The dynamic import is what turns "they happen to be
 * safe" into a property `check:diff-render` asserts on the emitted bundle, and it keeps the
 * grammar chunks out of the diff pane's download until somebody opens a diff.
 *
 * # Not a second highlighter
 *
 * `loadGrammar` hands back the very tokenizer `StreamLanguage.define` is given and
 * `highlight.ts::tokenClassFor` turns its token names into the same `cide-tk-*` classes, so a
 * file open in an editor tab and in a diff beside it is coloured by one table. This is the same
 * road `editor/markdown/fenceTokens.ts` built for the preview, pointed at two documents instead
 * of one fence.
 */
import { exceedsBytes } from '../editor/byteSize'
import { languageIdFor } from '../editor/languages'
import { grammarFor, tokenizeFence } from '../editor/markdown/fenceTokens'
import { DIFF_HIGHLIGHT_LIMIT_BYTES, type DiffTokens, tokenLines } from './diffTokens'

/**
 * Both sides of a diff, tokenized — or `null` for a diff drawn in one colour.
 *
 * `null` when: neither side has text (a hunks-only wire, `textsOmitted`, a binary file), no
 * grammar claims either path, or a side is over {@link DIFF_HIGHLIGHT_LIMIT_BYTES}.
 *
 * **All-or-nothing across the two sides**, which is `cide_git::diff::DiffTexts::combine`'s own
 * rule restated on this side of the wire rather than a second one invented here: one coloured
 * column beside a plain one reads as "the other side is not code", which is a worse answer than
 * two plain columns.
 *
 * Never rejects on a missing grammar — `loadGrammar` answers `null` for a language it cannot
 * load — so the caller's only failure path is the dynamic `import()` itself.
 */
export async function diffTokens(
  path: string,
  oldPath: string | null,
  oldText: string | null,
  newText: string | null,
): Promise<DiffTokens | null> {
  if (oldText === null && newText === null) return null
  if (
    exceedsBytes(oldText ?? '', DIFF_HIGHLIGHT_LIMIT_BYTES) ||
    exceedsBytes(newText ?? '', DIFF_HIGHLIGHT_LIMIT_BYTES)
  ) {
    return null
  }

  /*
   * Two ids, not one, because a rename can change the extension — `util.js` becoming `util.ts` is
   * one diff of two documents in two languages — and the old side is genuinely the other one.
   * `diffBlame` refuses the old side for the same underlying reason: it is a different document
   * at a different revision, and answering for it with the new side's table is a guess.
   */
  const newId = languageIdFor(path)
  const oldId = languageIdFor(oldPath ?? path)
  const [newGrammar, oldGrammar] = await Promise.all([
    newId === null ? null : grammarFor(newId),
    oldId === null ? null : grammarFor(oldId),
  ])

  // `tokenizeFence`'s output feeds `tokenLines` directly, and that assignment is what pins
  // `DiffToken` to `fenceTokens.Token`: a field added on that side and not restated here is a
  // compile error rather than a drift nobody sees.
  const newLines =
    newText === null || newGrammar === null ? [] : tokenLines(tokenizeFence(newGrammar, newText))
  const oldLines =
    oldText === null || oldGrammar === null ? [] : tokenLines(tokenizeFence(oldGrammar, oldText))

  // Both empty is spelled `null` rather than a pair of empty arrays, so the pane's prop is either
  // usable or absent and `lineTokens` never walks two empty sides once per drawn row.
  if (newLines.length === 0 && oldLines.length === 0) return null
  return { oldLines, newLines }
}
