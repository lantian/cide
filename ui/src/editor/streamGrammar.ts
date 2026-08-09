/**
 * One tokenizer, driven by per-language data.
 *
 * The alternative was `@codemirror/lang-*` — a Lezer grammar per language, each ~50-150 KB
 * of generated parser tables, and eleven more dependencies to keep in lockstep with
 * `@codemirror/language`. The milestone explicitly rules out the other end of that scale
 * (`web-tree-sitter`, ~1 MB per grammar). What is actually needed for "edit + syntax
 * highlight, no LSP" is a lexer: the colours in the mock are keywords, types, calls,
 * attributes, strings, comments and `?`, none of which needs a parse tree.
 *
 * So: `StreamLanguage` over this, one small data module per language, each its own dynamic
 * `import()`. A language costs about a kilobyte and this engine is shared between them.
 *
 * What is knowingly given up: anything that needs structure. A type used as a value is
 * still coloured as a type, `fn` inside a string of Rust source is not a keyword only
 * because the string is consumed whole, and there is no folding or indentation beyond the
 * bracket heuristic. That is the honest limit of a lexer, and re-reading the mock, it is
 * also the limit of what the design asks for.
 */
import type { StringStream } from '@codemirror/language'

/** A hook's answer: a tag name, or null to fall through to the generic path. */
export type HookResult = string | null

export interface GrammarState {
  /** Block-comment nesting depth; 0 when not in one. */
  comment: number
  /** The quote character that opened the string we are inside, or null. */
  quote: string | null
  /** Set while inside a triple-quoted string, which ends only on a matching triple. */
  triple: boolean
  /** Rust raw strings: the number of `#` that must close them. -1 when not in one. */
  hashes: number
  /** Scratch slot a language hook may use; copied by value. */
  flag: number
}

export interface GrammarSpec {
  /** Shown in the breadcrumb readout: `Rust · UTF-8 · LF · …`. */
  name: string
  keywords?: readonly string[]
  /** Coloured as keywords too; kept separate so a language can tag control flow if it wants. */
  types?: readonly string[]
  /** `true`, `null`, `self` — anything that reads as a keyword-shaped literal. */
  atoms?: readonly string[]
  /** Standard-library or builtin functions, coloured as calls. */
  builtins?: readonly string[]
  lineComment?: string
  blockComment?: readonly [string, string]
  /** Rust and Zig nest `/* … *\/`; C does not, and treating C as nesting mis-colours it. */
  nestedComments?: boolean
  /** Characters that open a string. Defaults to `"` and `'`. */
  quotes?: string
  /** Whether a backslash escapes the next character inside a string. */
  escapes?: boolean
  /** Python and TOML close a `"""` only on another `"""`. */
  tripleQuotes?: boolean
  /** Extra characters that continue an identifier, beyond letters, digits and `_`. */
  identifierExtra?: string
  /** Whether `Foo` — an initial capital — reads as a type. Wrong for shell and YAML. */
  capitalisedIsType?: boolean
  /** Whether `name(` reads as a call. Wrong for shell, where `(` is a subshell. */
  callSyntax?: boolean
  /** Operator characters that get the `--red` control-operator role, such as Rust's `?`. */
  controlOperators?: string
  /**
   * Run before everything else, once per token.
   *
   * This is where a language's genuinely irregular parts live — Rust lifetimes, which look
   * like an unterminated character literal, or a Markdown heading, which is a property of
   * the line rather than of the characters. Returning null falls through to the generic
   * path, so a hook only has to describe what is unusual.
   */
  hook?: (stream: StringStream, state: GrammarState) => HookResult
}

/**
 * Whether nothing but whitespace precedes the stream's position on this line.
 *
 * `stream.sol()` is the obvious thing and the wrong one for a hook: the generic path eats
 * leading whitespace and returns before the hook runs, so by the time a hook sees an
 * indented `key:` the position is past column 0 and `sol()` is already false. This asks the
 * question the hooks actually mean — "is this the first real token of the line" — which is
 * also what makes an indented YAML key and a top-level one the same case.
 */
export function atLineStart(stream: StringStream): boolean {
  for (let i = 0; i < stream.pos; i++) {
    const ch = stream.string[i]
    if (ch !== ' ' && ch !== '\t') return false
  }
  return true
}

const OPERATOR_CHARS = '+-*/%=<>!&|^~:?.'
const PUNCTUATION_CHARS = ',;'
const BRACKET_CHARS = '()[]{}'

function isIdentifierStart(ch: string, extra: string): boolean {
  return /[A-Za-z_$]/.test(ch) || extra.includes(ch)
}

function isIdentifierPart(ch: string, extra: string): boolean {
  return /[A-Za-z0-9_$]/.test(ch) || extra.includes(ch)
}

/**
 * Build a `StreamParser` for one language.
 *
 * Returned as a plain object rather than a `StreamLanguage` so each language module can
 * decide what to wrap it in — and so this file can be unit-tested without constructing an
 * `EditorState`.
 */
export function grammar(spec: GrammarSpec) {
  const keywords = new Set(spec.keywords ?? [])
  const types = new Set(spec.types ?? [])
  const atoms = new Set(spec.atoms ?? [])
  const builtins = new Set(spec.builtins ?? [])
  const quotes = spec.quotes ?? '"\''
  const extra = spec.identifierExtra ?? ''
  const escapes = spec.escapes ?? true

  /** Consume the rest of a block comment, updating the nesting depth. Returns true at end. */
  const runComment = (stream: StringStream, state: GrammarState): void => {
    const [open, close] = spec.blockComment ?? ['/*', '*/']
    while (!stream.eol()) {
      if (stream.match(close)) {
        state.comment--
        if (state.comment === 0) return
        continue
      }
      if (spec.nestedComments && stream.match(open)) {
        state.comment++
        continue
      }
      stream.next()
    }
  }

  /** Consume the rest of a string. Leaves `state.quote` set if it runs off the line. */
  const runString = (stream: StringStream, state: GrammarState): void => {
    const quote = state.quote
    if (quote === null) return
    while (!stream.eol()) {
      if (escapes && state.hashes < 0 && stream.peek() === '\\') {
        stream.next()
        stream.next()
        continue
      }
      // A Rust raw string ends at `"` followed by exactly as many `#` as opened it.
      if (state.hashes >= 0) {
        if (stream.peek() === quote) {
          const mark = stream.pos
          stream.next()
          let seen = 0
          while (seen < state.hashes && stream.eat('#')) seen++
          if (seen === state.hashes) {
            state.quote = null
            state.hashes = -1
            return
          }
          stream.pos = mark + 1
        } else {
          stream.next()
        }
        continue
      }
      if (state.triple) {
        if (stream.match(quote.repeat(3))) {
          state.quote = null
          state.triple = false
          return
        }
        stream.next()
        continue
      }
      if (stream.eat(quote)) {
        state.quote = null
        return
      }
      stream.next()
    }
    // A single-quoted string that reaches the end of the line is almost always an apostrophe
    // or a lifetime rather than a two-line string, so it is closed here. Double quotes are
    // left open, because a multi-line string really is a shape languages have.
    if (!state.triple && state.hashes < 0 && quote === "'") state.quote = null
  }

  return {
    name: spec.name,

    startState(): GrammarState {
      return { comment: 0, quote: null, triple: false, hashes: -1, flag: 0 }
    },

    // Written out rather than left to the default, which copies one level and would share
    // nothing here anyway — but the default is documented as a shallow copy, and a state
    // that grows an array later would silently start aliasing across the parse cache.
    copyState(state: GrammarState): GrammarState {
      return { ...state }
    },

    token(stream: StringStream, state: GrammarState): string | null {
      if (state.comment > 0) {
        runComment(stream, state)
        return 'comment'
      }
      if (state.quote !== null) {
        runString(stream, state)
        return 'string'
      }
      if (stream.eatSpace()) return null

      const hooked = spec.hook?.(stream, state)
      if (hooked !== null && hooked !== undefined) return hooked

      if (spec.lineComment && stream.match(spec.lineComment)) {
        stream.skipToEnd()
        return 'comment'
      }
      if (spec.blockComment && stream.match(spec.blockComment[0])) {
        state.comment = 1
        runComment(stream, state)
        return 'comment'
      }

      // `peek` answers `undefined` at end of line. The generic path is never entered
      // there, but the type says it can be and a cast would be the wrong way to say so.
      const ch = stream.peek()
      if (ch === undefined) {
        stream.next()
        return null
      }

      if (quotes.includes(ch)) {
        stream.next()
        state.quote = ch
        // `=== true` and not a truthiness test: `StringStream.match` answers `true` on a hit
        // and **`null`** on a miss, so `!== false` reads every ordinary `"…"` in a TOML or
        // Python file as the opening of a triple-quoted string — which swallows the rest of
        // the document. This cost an afternoon; the smoke run over `Cargo.toml` is what
        // showed it, as 5,771 string tokens and two keys.
        state.triple = spec.tripleQuotes === true && stream.match(ch.repeat(2)) === true
        runString(stream, state)
        return 'string'
      }

      if (/[0-9]/.test(ch)) {
        // One pattern for every numeric shape the supported languages have: an optional
        // radix prefix, digits with `_` separators, an optional fraction and exponent, and
        // a trailing type suffix (`1_000u64`, `1.5f32`, `0xffu8`).
        stream.match(/^(?:0[xXbBoO][0-9a-fA-F_]+|[0-9][0-9_]*(?:\.[0-9_]+)?(?:[eE][-+]?[0-9]+)?)/)
        stream.match(/^[a-zA-Z_][a-zA-Z0-9_]*/)
        return 'number'
      }

      if (isIdentifierStart(ch, extra)) {
        stream.next()
        while (!stream.eol()) {
          const next = stream.peek()
          if (next === undefined || !isIdentifierPart(next, extra)) break
          stream.next()
        }
        const word = stream.current()
        if (keywords.has(word)) return 'keyword'
        if (atoms.has(word)) return 'atom'
        if (types.has(word)) return 'typeName'
        if (builtins.has(word)) return 'variableName.function'
        if (spec.capitalisedIsType && /^[A-Z]/.test(word)) return 'typeName'
        // Lookahead only, never consumed: `stream.match(/…/, false)` leaves the position
        // alone, so the `(` is still there to be tokenized as a bracket on the next call.
        if (spec.callSyntax && stream.match(/^\s*\(/, false)) return 'variableName.function'
        return 'variableName'
      }

      if (spec.controlOperators?.includes(ch)) {
        stream.next()
        return 'controlOperator'
      }
      if (OPERATOR_CHARS.includes(ch)) {
        stream.eatWhile((c: string) => OPERATOR_CHARS.includes(c))
        return 'operator'
      }
      if (BRACKET_CHARS.includes(ch)) {
        stream.next()
        return 'bracket'
      }
      if (PUNCTUATION_CHARS.includes(ch)) {
        stream.next()
        return 'punctuation'
      }

      stream.next()
      return null
    },

    languageData: {
      commentTokens: spec.blockComment
        ? { line: spec.lineComment, block: { open: spec.blockComment[0], close: spec.blockComment[1] } }
        : { line: spec.lineComment },
    },
  }
}
