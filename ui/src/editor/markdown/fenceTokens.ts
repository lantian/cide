/**
 * Colouring a fenced code block with the buffer's own grammars. (M20)
 *
 * ```rust
 * fn main() {}
 * ```
 *
 * — the three lines above, in the preview, coloured by the same `languages/rust.ts` table that
 * colours a `.rs` buffer. Not a second highlighter: `loadGrammar` hands back the very tokenizer
 * `StreamLanguage.define` is given, and `highlight.ts::tokenClassFor` turns its token names into
 * the same `cide-tk-*` classes. In split mode the buffer and the preview are eight pixels apart,
 * and two highlighters that could disagree would be seen disagreeing.
 *
 * # The preview colours code the buffer does not, and that is on purpose
 *
 * `languages/markdown.ts` says of the buffer: *"Code inside a fence is left uncoloured rather
 * than dispatched to the fence's language. Nesting one stream parser inside another needs
 * `StreamLanguage`'s nesting support, which it does not have (`allowsNesting` is false)."* That
 * is still true, and it is a constraint on **CodeMirror**, not on markdown. Out here there is no
 * outer parser to nest inside — the fence's text is a string and the grammar is a function — so
 * the thing the buffer cannot do costs about forty lines. The difference is visible and is
 * written down in README rather than left to be discovered.
 *
 * # Why there is a cap
 *
 * The buffer tokenizes the lines CodeMirror's viewport asks for. A preview has no viewport: it is
 * a DOM tree, all of it, and a 200 KB fence would be tokenized in full on the frame the document
 * is parsed. [`FENCE_HIGHLIGHT_LIMIT_BYTES`] is where that stops being worth it; past it the
 * fence renders as plain monospace, which is what the buffer shows anyway.
 */
import { fenceAlias, loadGrammar, languageIdFor, type LanguageId } from '../languages'
import { tokenClassFor } from '../highlight'
import { utf8ByteLength } from '../byteSize'
import { FENCE_HIGHLIGHT_LIMIT_BYTES } from './view'
import { StringStream } from '@codemirror/language'
import type { Grammar } from '../streamGrammar'

/** One coloured run inside a line. `cls` is null for text with no role. */
export interface Token {
  readonly text: string
  readonly cls: string | null
}

/**
 * Fence info words that are not file extensions.
 *
 * Deliberately spelled as **extensions** rather than as language ids, so that every value here
 * goes through `languageIdFor` like everything else and this table cannot name a language the
 * registry does not have. `ui/scripts/check-markdown.mjs` resolves every one of them and fails on
 * a dead entry.
 *
 * `console`, `shell-session` and `sh-session` are what people fence a terminal transcript with,
 * and shell is the closest thing to right for it. `text`, `plain` and `txt` are deliberately
 * absent: they resolve to nothing, which is exactly what they are asking for.
 */
/**
 * Which grammar a fence's info string asks for, or null.
 *
 * A fence is labelled by a *word*, not by a filename, so the word is turned into one — `x.rs` —
 * and put through the same `lookup` the editor uses. That is what makes ` ```tsx ` and a `.tsx`
 * file agree without a second table of extensions.
 */
export function fenceLanguage(info: string | null): LanguageId | null {
  if (info === null) return null
  const word = info.trim().toLowerCase()
  if (word === '') return null
  // A language's own alias list first — `golang`, `console`, `c++` — then the word read as an
  // extension, which is what makes ` ```tsx ` and a `.tsx` file agree without a second table.
  //
  // The alias table used to live here and mapped a word to an *extension* so it would route
  // through `lookup` too. That indirection went in M22 with the closed language union: a language
  // now names its own fence words in `cide_ipc::lang::builtins()`, beside the extensions it
  // claims, and an extension's manifest can name some as well.
  return fenceAlias(word) ?? languageIdFor(`x.${word}`)
}

/**
 * Tokenize one fence's text with `spec`, line by line.
 *
 * The state is carried across lines, which is the whole reason this cannot be done per line
 * independently: a Rust block comment, or a Python `"""…"""`, opened on line 3 and closed on line 9
 * is one token, and a per-line tokenizer colours the six lines between them as code.
 *
 * `token()` returning without advancing the stream is a grammar bug that hangs the browser, and
 * `check-editor.mjs` already asserts every grammar advances — the guard here is for the case that
 * check cannot reach: an input no corpus contains. It costs one comparison per token.
 */
export function tokenizeFence(spec: Grammar, text: string): Token[][] {
  const state = spec.startState()
  const out: Token[][] = []

  for (const line of text.split('\n')) {
    const tokens: Token[] = []
    const stream = new StringStream(line, 4, 2)
    if (line === '') {
      out.push(tokens)
      continue
    }
    while (!stream.eol()) {
      const before = stream.pos
      const name = spec.token(stream, state)
      if (stream.pos === before) {
        // Nothing consumed: take one character as plain text rather than spin.
        stream.pos = before + 1
      }
      const slice = line.slice(before, stream.pos)
      const cls = name === null ? null : tokenClassFor(name)
      const last = tokens[tokens.length - 1]
      // Adjacent runs with the same role are one span; a token per identifier would be several
      // thousand DOM nodes for a long fence, all of them styled identically.
      if (last !== undefined && last.cls === cls) tokens[tokens.length - 1] = { text: last.text + slice, cls }
      else tokens.push({ text: slice, cls })
      stream.start = stream.pos
    }
    out.push(tokens)
  }

  return out
}

/** Whether a fence is small enough to be worth colouring. */
export function fenceIsHighlightable(text: string): boolean {
  return utf8ByteLength(text) <= FENCE_HIGHLIGHT_LIMIT_BYTES
}

/**
 * Every grammar this window has already loaded.
 *
 * Module-level for the reason `editor/diffViewMode.ts` gives about its own cache: every open tab
 * stays mounted, so a per-component load would be one dynamic `import()` per markdown pane per
 * fence. The `import()` itself is idempotent, but the promise plumbing around it is not free and
 * the re-render it causes very much is not.
 */
const loaded = new Map<LanguageId, Grammar | null>()
const promises = new Map<LanguageId, Promise<Grammar | null>>()

/** The grammar for `id` if it is already here. Null means "not loaded, or will not load". */
export function grammarIfReady(id: LanguageId): Grammar | null {
  return loaded.get(id) ?? null
}

/**
 * The grammar for `id`, loaded at most once per window.
 *
 * The promise-shaped half of this cache, and the one every other entry point is built on. A
 * caller that is *already* async — `panes/diffHighlight.ts`, which has to await a dynamic
 * `import()` before it can tokenize anything — wants this rather than the callback below, and
 * giving it a second cache of its own would be the duplication this file's header refuses.
 *
 * `loadGrammar` never rejects (it answers `null` for a language with no module and for a chunk
 * that will not load), so neither does this, and no caller needs a `.catch` to keep a render
 * from breaking on a missing grammar.
 */
export function grammarFor(id: LanguageId): Promise<Grammar | null> {
  const already = promises.get(id)
  if (already !== undefined) return already
  const run = loadGrammar(id).then((spec) => {
    loaded.set(id, spec)
    return spec
  })
  promises.set(id, run)
  return run
}

/**
 * Ask for a grammar, and call `then` once — and only if — it arrives and is new.
 *
 * Deliberately not a promise the caller awaits: the caller is a React render, and a render that
 * awaits is a render that has already returned. The contract is "paint uncoloured now, and I will
 * tell you when there is something better", which is also what makes a fence in a language whose
 * chunk fails to load render as monospace instead of as nothing.
 *
 * **Every requester is called back, not just the first.** This used to return early when a
 * grammar was already in flight, which silently dropped the callback of anyone who asked during
 * that window — so a second consumer painted plain until some unrelated re-render happened to
 * come along. It was invisible while the markdown preview was the only caller, because one
 * re-render there refreshes every fence in the document at once; it stops being invisible the
 * moment a second component tree wants the same language, which `panes/diffHighlight.ts` is.
 * Building on `grammarFor` fans the one load out to every awaiter instead.
 */
export function requestGrammar(id: LanguageId, then: () => void): void {
  if (loaded.has(id)) return
  void grammarFor(id).then((spec) => {
    if (spec !== null) then()
  })
}

/** Testing seam. Never called by the app. */
export function resetGrammarCacheForTest(): void {
  loaded.clear()
  promises.clear()
}
