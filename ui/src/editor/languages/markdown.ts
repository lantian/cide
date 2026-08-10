/**
 * Markdown — almost entirely hook, because almost nothing in it is a lexeme.
 *
 * A heading is a property of a line, emphasis is a pair of delimiters around arbitrary
 * text, and a fenced block turns the tokenizer off until a matching fence. None of that is
 * keywords and strings, so the generic table is left empty and the hook does the work; what
 * falls through is prose, which should be `--text` and is.
 *
 * Code inside a fence is left uncoloured rather than dispatched to the fence's language.
 * Nesting one stream parser inside another needs `StreamLanguage`'s nesting support, which
 * it does not have (`allowsNesting` is false), and the honest alternative — a second
 * tokenizer driven by hand — would double this file to colour a minority of lines.
 */
import type { StringStream } from '@codemirror/language'
import { atLineStart, grammar, type GrammarState, type HookResult } from '../streamGrammar'

const IN_FENCE = 1

/**
 * How far past a `[` the link matcher will look for its `](` and `)`.
 *
 * The bound is the whole point, not a tidy-up. Unbounded, `\[[^\]]*\]\(` scans to the end of
 * the line for every `[` that does not open a link and then backtracks over the whole scan
 * one character at a time — so a line of brackets is quadratic. Measured at 3.8 s for a
 * 160,000-character line of `[`, against 0.13 s for the same line with this bound; the ratio
 * across an 8× length step goes from 51× to 8×.
 *
 * The alternative that lost was an atomic group, `(?=([^\]]{0,512}))\1`, which is another
 * 2× faster and which nobody reading this file in a year would recognise. The cost of the
 * bound is that a link whose text or target runs past 512 characters is drawn as prose
 * rather than as a link, which is a colour nobody will miss.
 */
const LINK_SCAN_LIMIT = 512

/** `[text](url)` and `![alt](url)`, with the trailing `)` optional so a half-typed one colours. */
const LINK = new RegExp(`^!?\\[[^\\]]{0,${LINK_SCAN_LIMIT}}\\]\\([^)]{0,${LINK_SCAN_LIMIT}}\\)?`)

function hook(stream: StringStream, state: GrammarState): HookResult {
  const first = atLineStart(stream)

  if (first && stream.match(/^(?:```|~~~)/)) {
    state.flag = state.flag === IN_FENCE ? 0 : IN_FENCE
    stream.skipToEnd()
    return 'meta'
  }
  if (state.flag === IN_FENCE) {
    stream.skipToEnd()
    return 'monospace'
  }

  if (first) {
    if (stream.match(/^#{1,6}\s.*/)) return 'heading'
    // A setext underline, and the two horizontal-rule spellings.
    if (stream.match(/^(?:={3,}|-{3,}|\*{3,}|_{3,})\s*$/)) return 'heading'
    if (stream.match(/^>+/)) return 'quote'
    if (stream.match(/^(?:[-*+]|\d+\.)\s/)) return 'punctuation'
    // A table row's leading pipe; the rest of the row is prose.
    if (stream.match(/^\|/)) return 'punctuation'
  }

  // Inline code first: everything else may legitimately appear inside a backtick span.
  if (stream.match(/^`[^`]*`?/)) return 'monospace'
  if (stream.match(/^(?:\*\*|__)(?:[^*_]|\*(?!\*)|_(?!_))*(?:\*\*|__)?/)) return 'strong'
  if (stream.match(/^(?:\*|_)(?:[^*_]+)(?:\*|_)?/)) return 'emphasis'
  if (stream.match(LINK)) return 'link'
  if (stream.match(/^<[^>\s]+>/)) return 'link'
  // Bare URLs, which are common in prose here and read as links everywhere else.
  if (stream.match(/^https?:\/\/\S+/)) return 'link'

  // Prose. Consumed a run at a time rather than a character at a time — a token per letter
  // would put one DOM element per letter in the rendered line.
  stream.match(/^[^`*_[<!|>#\s]+/) || stream.next()
  return null
}

export const spec = grammar({
  name: 'Markdown',
  // No quote characters: an apostrophe in prose is not the start of a string.
  quotes: '',
  capitalisedIsType: false,
  callSyntax: false,
  hook,
})
