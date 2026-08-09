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
import { atLineStart, grammar, type HookResult } from '../streamGrammar'

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

function yamlHook(stream: StringStream): HookResult {
  const first = atLineStart(stream)

  if (first && stream.match(/^(?:---|\.\.\.|%[A-Z]+.*)/)) return 'meta'

  // A key, optionally behind the `- ` of a list entry. The leading class excludes `:` so
  // the match cannot succeed having consumed nothing.
  //
  // Tried only at the head of the line, which is where a block-mapping key is, and this is
  // load-bearing rather than tidy. `[^:]*` scans to the end of the line before its lookahead
  // can fail, so running it once per token makes a long line quadratic: a 200,000-character
  // line of punctuation is 200,000 token calls each scanning 200,000 characters, measured at
  // 4.8 s against 20 ms for the same line at line-start only. CodeMirror parses under a time
  // budget so that never froze the window — it just meant the colour never arrived and a
  // core burned until the buffer closed. Gating on the line head also replaces the
  // "rest of this line is a value" scratch flag that kept `://` in a URL from reading as a
  // second key, since nothing past the head is tested at all now.
  if (first && stream.match(/^(?:-\s+)?[^\s#:][^:]*(?=:(?:\s|$))/)) return 'propertyName'

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
