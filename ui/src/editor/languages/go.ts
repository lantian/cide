/**
 * Go.
 *
 * Until M12 this file did not exist and `.go` was pointed at the shared `clike` table, which is
 * Java's keyword set with a C-family shape. That is wrong in ways a reader notices immediately:
 * `func`, `chan`, `defer`, `go` and `range` painted as plain identifiers, `var` and `type`
 * unhighlighted, and every backtick-quoted raw string treated as an unterminated something.
 * Asking for Go support and getting Java's keywords is the kind of near-miss that reads as a bug
 * in the editor rather than as a missing feature.
 *
 * Three things below are not the generic tokenizer's business, and they are why this module has a
 * hook at all:
 *
 * * **Raw strings.** `` `…` `` spans lines and honours no escapes — a regex literal or a Windows
 *   path inside one would otherwise end the string early on its first backslash.
 * * **Rune literals.** `'x'` and `'\n'` are single-quoted in Go, but so is nothing else, and the
 *   generic quote handler is happy to open a string that never closes on an apostrophe in a
 *   comment. Matching the whole literal keeps it contained.
 * * **Labels and struct tags.** Neither is common enough to warrant a role of its own, and both
 *   are left to fall through deliberately — noted so the next reader does not add them twice.
 */
import type { StringStream } from '@codemirror/language'
import { grammar, type GrammarState, type HookResult } from '../streamGrammar'

/**
 * Go's 25 keywords — the whole list, which is short enough to be exhaustive rather than
 * representative. `go`, `defer`, `chan` and `select` are the four the `clike` table was missing
 * that a Go reader would notice first.
 */
const KEYWORDS = [
  'break', 'case', 'chan', 'const', 'continue', 'default', 'defer', 'else', 'fallthrough',
  'for', 'func', 'go', 'goto', 'if', 'import', 'interface', 'map', 'package', 'range',
  'return', 'select', 'struct', 'switch', 'type', 'var',
]

/** `nil`, `iota` and the two booleans: keyword-shaped literals rather than identifiers. */
const ATOMS = ['true', 'false', 'nil', 'iota']

/**
 * The predeclared types, including the aliases.
 *
 * `any` is here rather than in keywords: it is a predeclared *alias* for `interface{}`, so it
 * reads as a type, and a Go file written since 1.18 is full of it.
 */
const TYPES = [
  'any', 'bool', 'byte', 'comparable', 'complex64', 'complex128', 'error', 'float32',
  'float64', 'int', 'int8', 'int16', 'int32', 'int64', 'rune', 'string', 'uint', 'uint8',
  'uint16', 'uint32', 'uint64', 'uintptr',
]

/**
 * The predeclared functions.
 *
 * Coloured as calls, which is what they are — but note they are *not* keywords: `len` can be
 * shadowed by a local variable, and painting it as a keyword would make the shadowed one look
 * like a syntax error.
 */
const BUILTINS = [
  'append', 'cap', 'clear', 'close', 'complex', 'copy', 'delete', 'imag', 'len', 'make',
  'max', 'min', 'new', 'panic', 'print', 'println', 'real', 'recover',
]

function hook(stream: StringStream, state: GrammarState): HookResult {
  /*
   * A raw string. Opened here so the generic quote path never sees the backtick, and parked in
   * the state because it may run to the end of the file: a struct tag, an embedded SQL query and
   * a multi-line template are all raw strings, and closing one at the end of its first line would
   * paint the rest of the literal as code.
   *
   * No escape handling inside it, which is the whole point of the form — `` `C:\new` `` is a
   * backslash and an `n`, not a newline.
   */
  if (stream.eat('`')) {
    state.quote = '`'
    return 'string'
  }

  /*
   * A rune literal, matched whole.
   *
   * `'\n'`, `'\''`, `'\u00e9'` and `'x'` — the escape alternative comes first so `'\''` is not
   * cut short at its middle quote. Matching the entire literal rather than opening a string is
   * what keeps an apostrophe in a comment or an identifier from starting one that never ends.
   */
  if (stream.match(/^'(?:\\(?:[abfnrtv\\'"0]|x[0-9a-fA-F]{2}|u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8})|[^'\\])'/)) {
    return 'string'
  }

  /*
   * Deliberately not handled, so the next reader does not add them twice:
   *
   * * **Labels** (`Outer:` before a `for`). Indistinguishable from a map key or a `case` without
   *   parsing, and colouring every `ident:` would light up half of every composite literal.
   * * **Struct tags** (`` `json:"name"` ``). Already a raw string above, which is what they are;
   *   a role of their own would be a second colour for the same syntax.
   */
  return null
}

export const spec = grammar({
  name: 'Go',
  keywords: KEYWORDS,
  atoms: ATOMS,
  types: TYPES,
  builtins: BUILTINS,
  lineComment: '//',
  blockComment: ['/*', '*/'],
  // Go's block comments do **not** nest — the spec says a `/*` inside one is not special, so the
  // first `*/` ends it. Rust's do, which is why that table sets this and this one must not: a
  // nesting scanner here would swallow everything after a commented-out region that contained a
  // comment, to the end of the file.
  nestedComments: false,
  // `'` is handled by the hook above as a whole rune literal, and backtick by the raw-string
  // branch, so the generic path only ever opens a double-quoted string.
  quotes: '"',
  capitalisedIsType: true,
  callSyntax: true,
  hook,
})
