/**
 * Python.
 *
 * The only structural thing needed here is triple-quoted strings, which the generic
 * tokenizer knows about because Python and TOML both have them; the hook covers decorators
 * and `f"…"` prefixes.
 */
import type { StringStream } from '@codemirror/language'
import { grammar, type HookResult } from '../streamGrammar'

const KEYWORDS = [
  'and', 'as', 'assert', 'async', 'await', 'break', 'class', 'continue', 'def', 'del',
  'elif', 'else', 'except', 'finally', 'for', 'from', 'global', 'if', 'import', 'in', 'is',
  'lambda', 'match', 'nonlocal', 'not', 'or', 'pass', 'raise', 'return', 'try', 'while',
  'with', 'yield',
]

const ATOMS = ['True', 'False', 'None', 'self', 'cls', 'NotImplemented', 'Ellipsis']

const BUILTINS = [
  'abs', 'all', 'any', 'bool', 'bytes', 'dict', 'enumerate', 'float', 'int', 'isinstance',
  'len', 'list', 'max', 'min', 'open', 'print', 'range', 'repr', 'set', 'sorted', 'str',
  'sum', 'tuple', 'type', 'zip',
]

function hook(stream: StringStream): HookResult {
  if (stream.match(/^@[A-Za-z_][A-Za-z0-9_.]*/)) return 'meta'
  // A string prefix (`f`, `rb`, `u`) is part of the literal. Consumed without returning so
  // the quote that follows is handled by the generic string path on the same token.
  stream.match(/^(?:[fFrRbBuU]{1,2})(?=["'])/)
  return null
}

export const spec = grammar({
  name: 'Python',
  keywords: KEYWORDS,
  atoms: ATOMS,
  builtins: BUILTINS,
  lineComment: '#',
  tripleQuotes: true,
  capitalisedIsType: true,
  callSyntax: true,
  hook,
})
