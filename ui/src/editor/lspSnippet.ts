/**
 * LSP snippet syntax into CodeMirror's — and the refusal that keeps a bad translation out of a
 * user's file.
 *
 * # The two syntaxes are nearly the same, which is the hazard
 *
 * Both spell a numbered placeholder `${1:default}` and a final stop `${0}`, so a naive
 * implementation is to pass the template through untouched, and it works for most of what
 * rust-analyzer and gopls emit. What it does with the rest is insert *visible syntax* into
 * source code — a literal `${1` sitting in the buffer where an argument should be — and it does
 * it silently, which is the failure this module exists to make unreachable.
 *
 * The differences, each of them read out of `@codemirror/autocomplete`'s own parser
 * (`Snippet.parse`, regex `/[#$]\{(?:(\d+)(?::([^{}]*))?|((?:\\[{}]|[^{}])*))\}/`) rather than
 * out of documentation:
 *
 * | LSP | CodeMirror | what happens here |
 * | --- | --- | --- |
 * | `${1:name}`, `${0}` | the same | passed through |
 * | `$1`, `$0` | `${1}`, `${0}` | braced |
 * | `${1\|a,b\|}` | — | the first choice becomes the default |
 * | `$TM_FILENAME`, `${TM_SELECTED_TEXT:x}` | — | not a tab stop; the default is kept, or nothing |
 * | `\$`, `\}`, `\\` | `\{`, `\}` only | unescaped here, re-escaped on the way out |
 *
 * # The two things CodeMirror cannot represent, and what is done about them
 *
 * 1. **`#{` opens a field too, and there is no `\$` escape.** So a literal `$` or `#` that ends
 *    up immediately before a `{` in the output *becomes a placeholder*, and a template containing
 *    a shell interpolation, a Ruby `#{}` or a JS template literal would silently sprout one.
 * 2. **A default cannot contain a brace.** CodeMirror's is `[^{}]*`, so a nested LSP placeholder
 *    — `${1:${2:x}}`, which the spec permits — has no representation at all.
 *
 * In both cases the answer is the same and it is the safe direction: **give up on the snippet and
 * insert plain text.** A completion that inserts `push(value)` with nothing selected is a small
 * loss the user can work with; one that inserts `push(${1:value)` is a bug in their file. Every
 * refusal here is a feature degrading, never an error, and [`SnippetPlan`] carries which happened
 * so a caller could say so if it ever wanted to.
 *
 * # Import-free on purpose
 *
 * `ui/scripts/check-completion.mjs` compiles this file on its own and drives every row of the
 * table above plus both refusals. The same split `codeIntelGate.ts` and `memberNav.ts` make, and
 * for the same reason: a rule written inside a CodeMirror extension is a rule nothing can run.
 */

/** How a completion's insert text should actually be applied. */
export interface SnippetPlan {
  /**
   * `snippet` — hand [`template`] to CodeMirror's `snippet()`; it contains fields.
   *
   * `text` — insert [`template`] literally. Either the server sent plain text, or it sent a
   * snippet this module refused to translate; the caller does the same thing in both cases and
   * deliberately has no reason to care which.
   */
  readonly kind: 'snippet' | 'text'
  /** The CodeMirror template, or the literal text. Never contains LSP syntax. */
  readonly template: string
  /**
   * Set when a snippet was refused rather than absent — see the module header.
   *
   * Nothing renders it today. It exists so the refusal is *observable*: a check script asserts on
   * it, and a future report of "the placeholders stopped working in language X" has somewhere to
   * look other than a diff.
   */
  readonly refused?: 'nested-placeholder' | 'field-opener-in-text'
}

/** One parsed piece of an LSP template. */
type Part =
  | { readonly literal: string }
  | { readonly index: number; readonly text: string }

/**
 * Translate one LSP insert text.
 *
 * `snippet` is what the server said about the format — LSP's `insertTextFormat === 2`. A plain
 * item is returned untouched and **not** scanned for `${`, because its text is not a template and
 * a `$` in it is a `$`. That is why the flag is a parameter rather than being sniffed.
 */
export function toCodeMirrorSnippet(text: string, snippet: boolean): SnippetPlan {
  if (!snippet) return { kind: 'text', template: text }

  const parts = parseLsp(text)
  if (parts === null) {
    return { kind: 'text', template: flatten(parseLsp(text, true) ?? [{ literal: text }]), refused: 'nested-placeholder' }
  }

  /*
   * The refusal test is run against the *literal* parts only, and after unescaping, because that
   * is the string that will sit in the output uninterpreted. A `${` inside a placeholder's
   * default cannot arise — `parseLsp` refuses nesting before it gets here — and a `${` that this
   * module itself is about to emit is a field we meant.
   */
  for (const part of parts) {
    if ('literal' in part && OPENS_A_FIELD.test(part.literal)) {
      return { kind: 'text', template: flatten(parts), refused: 'field-opener-in-text' }
    }
  }

  const out = parts
    .map((part) =>
      'literal' in part ? escapeBraces(part.literal) : renderField(part.index, part.text),
    )
    .join('')
  return { kind: 'snippet', template: out }
}

/**
 * The sequences CodeMirror reads as the start of a field.
 *
 * **Both openers**, and `#{` is the one that is easy to forget: it is undocumented in the prose
 * and present in the parser, and it is a real sequence in Ruby and CoffeeScript source. A
 * translator that guarded only `${` would corrupt exactly the templates a Ruby language server
 * emits.
 */
const OPENS_A_FIELD = /[#$]\{/

/**
 * A field, in CodeMirror's spelling.
 *
 * The default is emitted only when it is non-empty: `${1:}` and `${1}` mean the same thing to
 * CodeMirror's parser, and the shorter one is what a reader of the template expects.
 */
function renderField(index: number, text: string): string {
  return text === '' ? `\${${index}}` : `\${${index}:${text}}`
}

/**
 * Escape a literal for CodeMirror.
 *
 * Only braces, and only because a `\{` inside a *field's default* is how CodeMirror spells a
 * literal brace. Outside a field a bare `{` is already literal, so this is belt and braces —
 * which is worth having, since the alternative when it is wrong is a silently mangled buffer.
 */
function escapeBraces(text: string): string {
  return text.replace(/[{}]/g, (brace) => `\\${brace}`)
}

/** Everything a template would insert, with no fields — the plain-text fallback. */
function flatten(parts: readonly Part[]): string {
  return parts.map((part) => ('literal' in part ? part.literal : part.text)).join('')
}

/**
 * Parse an LSP template into literals and fields, or `null` when it cannot be represented.
 *
 * `lenient` is for the fallback path: it parses the same grammar but never refuses, so a template
 * that was rejected for nesting can still have its *text* recovered. Without it the fallback
 * would insert the raw template, syntax and all — which is the exact failure the refusal exists
 * to avoid, arrived at by another road.
 */
function parseLsp(template: string, lenient = false): Part[] | null {
  const parts: Part[] = []
  let literal = ''
  let at = 0

  const flushLiteral = (): void => {
    if (literal !== '') {
      parts.push({ literal })
      literal = ''
    }
  }

  while (at < template.length) {
    const char = template[at]

    // `\$`, `\}`, `\\` are the spec's three escapes. Anything else after a backslash is a
    // backslash followed by that character — a Windows path in a snippet is not an escape
    // sequence, and treating it as one would eat separators.
    if (char === '\\' && at + 1 < template.length) {
      const next = template[at + 1] as string
      if (next === '$' || next === '}' || next === '\\') {
        literal += next
        at += 2
        continue
      }
      literal += char
      at += 1
      continue
    }

    if (char !== '$') {
      literal += char
      at += 1
      continue
    }

    // `$1` — the brace-free form. Digits only; `$foo` is a variable and is handled below.
    const bare = /^\$(\d+)/.exec(template.slice(at))
    if (bare !== null) {
      flushLiteral()
      parts.push({ index: Number(bare[1]), text: '' })
      at += bare[0].length
      continue
    }

    // `$TM_FILENAME` — a variable with no braces. Dropped: cide resolves none of them, and
    // leaving the name in would insert `$TM_FILENAME` as source text.
    const bareVar = /^\$([A-Za-z_][A-Za-z0-9_]*)/.exec(template.slice(at))
    if (bareVar !== null) {
      at += bareVar[0].length
      continue
    }

    if (template[at + 1] !== '{') {
      // A lone `$` — legal, literal, and common in shell and PHP.
      literal += char
      at += 1
      continue
    }

    const closed = matchBrace(template, at + 1)
    if (closed === null) {
      // Unbalanced. Not a template any more; treat the rest as literal rather than guessing.
      literal += template.slice(at)
      break
    }
    const inner = template.slice(at + 2, closed)
    at = closed + 1

    const field = parseField(inner)
    if (field === null) {
      // A variable in braces — `${TM_SELECTED_TEXT}` or `${TM_FILENAME:default}`. The default is
      // the useful half and the name is not resolvable, so the default is kept as literal text.
      const withDefault = /^[A-Za-z_][A-Za-z0-9_]*:([\s\S]*)$/.exec(inner)
      if (withDefault !== null) literal += withDefault[1] as string
      continue
    }

    // Nesting. CodeMirror's default is `[^{}]*` and simply cannot hold this, so the whole
    // template is refused — see the module header.
    if (!lenient && OPENS_A_FIELD.test(field.text)) return null

    flushLiteral()
    parts.push({
      index: field.index,
      // In lenient mode a nested default is flattened rather than refused, so the fallback text
      // is the user's arguments and not a fragment of syntax.
      text: lenient ? flatten(parseLsp(field.text, true) ?? []) : field.text,
    })
  }

  flushLiteral()
  return parts
}

/** The body of a `${...}` as an index and its default, or `null` when it names a variable. */
function parseField(inner: string): { index: number; text: string } | null {
  // `${1|a,b,c|}` — a choice. CodeMirror has no equivalent, and the first option is what every
  // editor pre-selects, so it becomes the default. Checked before the plain form because a
  // choice's body also starts with digits.
  const choice = /^(\d+)\|([\s\S]*)\|$/.exec(inner)
  if (choice !== null) {
    const first = (choice[2] as string).split(',')[0] ?? ''
    return { index: Number(choice[1]), text: first }
  }
  const plain = /^(\d+)(?::([\s\S]*))?$/.exec(inner)
  if (plain === null) return null
  return { index: Number(plain[1]), text: plain[2] ?? '' }
}

/**
 * The index of the `}` closing the `{` at `open`, honouring escapes and nesting.
 *
 * A scan rather than a regex, because the thing being matched is recursive and the escape rule
 * means a `\}` in a default must not close the field. Getting this wrong truncates a template at
 * its first inner brace, which for `${1:{}}` is a snippet that inserts an unbalanced brace.
 */
function matchBrace(template: string, open: number): number | null {
  let depth = 0
  for (let at = open; at < template.length; at += 1) {
    const char = template[at]
    if (char === '\\') {
      at += 1
      continue
    }
    if (char === '{') depth += 1
    else if (char === '}') {
      depth -= 1
      if (depth === 0) return at
    }
  }
  return null
}
