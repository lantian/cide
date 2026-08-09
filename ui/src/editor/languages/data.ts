/**
 * JSON, TOML and YAML — the three configuration formats this repo is full of.
 *
 * One module, three exported specs, because they are tiny and because a `Cargo.toml` and a
 * `package.json` are usually open at the same time; a shared chunk means the second one
 * costs nothing.
 *
 * The interesting colour in all three is the *key*, which is a position rather than a
 * lexeme: `"name"` is a string on the right of a colon and a property on the left. Each
 * hook resolves that with one lookahead, which is exactly the amount of structure a lexer
 * can have.
 *
 * Every regex below is written so that a hook which returns a tag has consumed at least one
 * character. `StreamLanguage` throws "Stream parser failed to advance stream" after ten
 * no-op calls, so a lookahead that succeeds and then consumes nothing is not a cosmetic bug
 * — it takes the buffer down.
 */
import type { StringStream } from '@codemirror/language'
import { atLineStart, grammar, type GrammarState, type HookResult } from '../streamGrammar'

/** `"key":` — a quoted string with a colon after it, at any depth. */
function quotedKey(stream: StringStream): HookResult {
  if (stream.match(/^"(?:[^"\\]|\\.)*"(?=\s*:)/)) return 'propertyName'
  return null
}

export const json = grammar({
  name: 'JSON',
  atoms: ['true', 'false', 'null'],
  // JSON proper has no comments; `.jsonc`, `tsconfig.json` and every `settings.json` in
  // this project do. Accepting them everywhere is the useful error.
  lineComment: '//',
  blockComment: ['/*', '*/'],
  quotes: '"',
  hook: quotedKey,
})

function tomlHook(stream: StringStream): HookResult {
  // A table header is the document's structure, so it takes the attribute colour rather
  // than becoming three punctuation tokens around a name.
  if (atLineStart(stream) && stream.match(/^\[\[?[^\]]*\]\]?/)) return 'meta'
  const quoted = quotedKey(stream)
  if (quoted !== null) return quoted
  // A bare key: a dotted identifier at the head of a line with an `=` after it.
  if (atLineStart(stream) && stream.match(/^[A-Za-z0-9_.-]+(?=\s*=)/)) return 'propertyName'
  return null
}

export const toml = grammar({
  name: 'TOML',
  atoms: ['true', 'false'],
  lineComment: '#',
  quotes: '"\'',
  tripleQuotes: true,
  identifierExtra: '-',
  hook: tomlHook,
})

function yamlHook(stream: StringStream, state: GrammarState): HookResult {
  const first = atLineStart(stream)
  // The scratch flag says "the rest of this line is a value", which is what keeps the
  // `://` in a URL from being read as a second key on the same line.
  if (first) state.flag = 0

  if (first && stream.match(/^(?:---|\.\.\.|%[A-Z]+.*)/)) return 'meta'

  // A key, optionally behind the `- ` of a list entry. The leading class excludes `:` so
  // the match cannot succeed having consumed nothing.
  if (state.flag === 0 && stream.match(/^(?:-\s+)?[^\s#:][^:]*(?=:(?:\s|$))/)) {
    state.flag = 1
    return 'propertyName'
  }

  // An anchor or an alias — the one piece of YAML that is genuinely a reference.
  if (stream.match(/^[&*][A-Za-z0-9_-]+/)) return 'labelName'
  return null
}

export const yaml = grammar({
  name: 'YAML',
  atoms: ['true', 'false', 'null', 'yes', 'no', 'on', 'off'],
  lineComment: '#',
  quotes: '"\'',
  hook: yamlHook,
})
