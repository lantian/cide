/**
 * C, C++ and Go, which share enough shape for one table plus a keyword union.
 *
 * The union is a deliberate inaccuracy: `func` is not a C keyword and `template` is not a
 * Go one, so a `.c` file containing an identifier named `func` colours it purple. Splitting
 * them would cost two more chunks and fix a case that does not occur in practice, and the
 * failure is cosmetic in a direction the reader immediately discounts.
 */
import type { StringStream } from '@codemirror/language'
import { atLineStart, grammar, type HookResult } from '../streamGrammar'

const KEYWORDS = [
  // C and C++
  'alignas', 'alignof', 'auto', 'break', 'case', 'catch', 'class', 'concept', 'const',
  'consteval', 'constexpr', 'continue', 'decltype', 'default', 'delete', 'do', 'else',
  'enum', 'explicit', 'export', 'extern', 'for', 'friend', 'goto', 'if', 'inline',
  'namespace', 'new', 'noexcept', 'operator', 'private', 'protected', 'public', 'register',
  'requires', 'return', 'sizeof', 'static', 'static_assert', 'struct', 'switch', 'template',
  'thread_local', 'throw', 'try', 'typedef', 'typename', 'union', 'using', 'virtual',
  'volatile', 'while',
  // Go
  'chan', 'defer', 'fallthrough', 'func', 'go', 'import', 'interface', 'map', 'package',
  'range', 'select', 'type', 'var',
]

const ATOMS = ['true', 'false', 'nil', 'NULL', 'nullptr', 'this', 'iota']

const TYPES = [
  'bool', 'byte', 'char', 'complex64', 'complex128', 'double', 'error', 'float', 'float32',
  'float64', 'int', 'int8', 'int16', 'int32', 'int64', 'long', 'rune', 'short', 'signed',
  'size_t', 'string', 'uint', 'uint8', 'uint16', 'uint32', 'uint64', 'uintptr', 'unsigned',
  'void',
]

function hook(stream: StringStream): HookResult {
  // A preprocessor directive is metadata about the translation unit, so it gets the same
  // `--yellow` an attribute does. Consumed to end of line: continuations are rare enough
  // that mishandling them costs one line of colour.
  if (atLineStart(stream) && stream.match(/^#\s*[a-z_]+/)) {
    stream.skipToEnd()
    return 'meta'
  }
  return null
}

export const spec = grammar({
  name: 'C-like',
  keywords: KEYWORDS,
  atoms: ATOMS,
  types: TYPES,
  lineComment: '//',
  blockComment: ['/*', '*/'],
  capitalisedIsType: true,
  callSyntax: true,
  hook,
})
