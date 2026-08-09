/**
 * TypeScript, TSX, JavaScript and JSX, from one table.
 *
 * Not split per dialect because a lexer cannot tell them apart anyway: `interface` is a
 * keyword in a `.ts` file and an ordinary identifier in a `.js` one, and colouring it as a
 * keyword in both is wrong in a way no reader has ever been misled by. Four separate
 * modules would cost four chunks to say the same thing.
 */
import type { StringStream } from '@codemirror/language'
import { grammar, type GrammarState, type HookResult } from '../streamGrammar'

const KEYWORDS = [
  'abstract', 'any', 'as', 'asserts', 'async', 'await', 'break', 'case', 'catch', 'class',
  'const', 'continue', 'debugger', 'declare', 'default', 'delete', 'do', 'else', 'enum',
  'export', 'extends', 'finally', 'for', 'from', 'function', 'get', 'if', 'implements',
  'import', 'in', 'infer', 'instanceof', 'interface', 'is', 'keyof', 'let', 'namespace',
  'new', 'of', 'override', 'private', 'protected', 'public', 'readonly', 'return',
  'satisfies', 'set', 'static', 'switch', 'throw', 'try', 'type', 'typeof', 'var', 'void',
  'while', 'with', 'yield',
]

const ATOMS = ['true', 'false', 'null', 'undefined', 'this', 'super', 'NaN', 'Infinity']

const TYPES = [
  'bigint', 'boolean', 'never', 'number', 'object', 'string', 'symbol', 'unknown',
  'Array', 'Map', 'Promise', 'Readonly', 'Record', 'Set', 'WeakMap',
]

function hook(stream: StringStream, state: GrammarState): HookResult {
  // Template literals. Tracked with the state's scratch flag rather than the quote slot
  // because the generic string runner would apply this language's `'` and `"` rules to the
  // body, and a backtick string is closed only by a backtick.
  //
  // `${…}` is *not* handled: an interpolation is code and is coloured here as string. Doing
  // it properly needs a nesting depth in the state and a way to hand control back to the
  // generic path mid-token, which is the point at which a lexer stops being the right tool.
  if (state.flag === 1) {
    while (!stream.eol()) {
      if (stream.peek() === '\\') {
        stream.next()
        stream.next()
        continue
      }
      if (stream.eat('`')) {
        state.flag = 0
        return 'string'
      }
      stream.next()
    }
    return 'string'
  }
  if (stream.eat('`')) {
    state.flag = 1
    return hook(stream, state)
  }

  // A decorator is metadata about the thing below it, same as a Rust attribute.
  if (stream.match(/^@[A-Za-z_$][A-Za-z0-9_$]*/)) return 'meta'
  return null
}

export const spec = grammar({
  name: 'TypeScript',
  keywords: KEYWORDS,
  atoms: ATOMS,
  types: TYPES,
  lineComment: '//',
  blockComment: ['/*', '*/'],
  capitalisedIsType: true,
  callSyntax: true,
  hook,
})
