/**
 * sh, bash and zsh.
 *
 * `capitalisedIsType` and `callSyntax` are both off, and both for the same reason: shell
 * looks like other languages and behaves like none of them. `PATH` is a variable, not a
 * type, and `(` after a word opens a subshell rather than an argument list — so the two
 * heuristics that earn their keep everywhere else would paint half of every script wrong.
 */
import type { StringStream } from '@codemirror/language'
import { atLineStart, grammar, type HookResult } from '../streamGrammar'

const KEYWORDS = [
  'if', 'then', 'elif', 'else', 'fi', 'case', 'esac', 'for', 'select', 'while', 'until',
  'do', 'done', 'in', 'function', 'time', 'coproc', 'return', 'break', 'continue', 'local',
  'declare', 'typeset', 'readonly', 'export', 'unset', 'shift', 'trap', 'set',
]

const BUILTINS = [
  'cd', 'echo', 'printf', 'read', 'test', 'eval', 'exec', 'exit', 'source', 'alias',
  'command', 'pushd', 'popd', 'wait', 'kill', 'jobs', 'umask', 'getopts',
]

function hook(stream: StringStream): HookResult {
  // The shebang names the interpreter and is not a comment about the code, but `#` would
  // make it one; catching it first is also what stops it being mistaken for a directive.
  if (atLineStart(stream) && stream.match(/^#!.*/)) return 'meta'
  // `$VAR`, `${VAR}`, `$(cmd)` opener, `$1`. Coloured as a name being read rather than
  // written, which is the closest honest role.
  if (stream.match(/^\$(?:\{[^}]*\}|[A-Za-z_][A-Za-z0-9_]*|[0-9*@#?$!-])/)) return 'propertyName'
  return null
}

export const spec = grammar({
  name: 'Shell',
  keywords: KEYWORDS,
  builtins: BUILTINS,
  lineComment: '#',
  // A hyphen continues a word: `--no-verify` is one token, not a word and two operators.
  identifierExtra: '-',
  capitalisedIsType: false,
  callSyntax: false,
  hook,
})
