/**
 * Rust. The language the mock specifies colours for, and the one this repo is written in.
 *
 * Four things here are not the generic tokenizer's business, and they are the reason this
 * module has a hook at all:
 *
 * * `'a` is a lifetime, and `'a'` is a character literal. They differ only in what follows
 *   the identifier, so the lifetime has to be recognised before the generic quote handler
 *   opens a string on the apostrophe and swallows the rest of the line.
 * * `#[derive(Debug)]` is an attribute. The mock paints attributes the same `--yellow` as
 *   strings, so it maps to `meta`.
 * * `r#"…"#` is a raw string whose terminator is decided by the opening hash count.
 * * `println!` is a macro, which reads as a call and takes `--blue`.
 */
import type { StringStream } from '@codemirror/language'
import { grammar, type GrammarState, type HookResult } from '../streamGrammar'

const KEYWORDS = [
  'as', 'async', 'await', 'break', 'const', 'continue', 'crate', 'dyn', 'else', 'enum',
  'extern', 'fn', 'for', 'if', 'impl', 'in', 'let', 'loop', 'match', 'mod', 'move', 'mut',
  'pub', 'ref', 'return', 'static', 'struct', 'super', 'trait', 'type', 'union', 'unsafe',
  'use', 'where', 'while', 'yield', 'macro_rules',
]

const ATOMS = ['true', 'false', 'None', 'Some', 'Ok', 'Err', 'self', 'Self']

const TYPES = [
  'bool', 'char', 'f32', 'f64', 'i8', 'i16', 'i32', 'i64', 'i128', 'isize', 'str', 'u8',
  'u16', 'u32', 'u64', 'u128', 'usize',
]

function hook(stream: StringStream, state: GrammarState): HookResult {
  // Raw and byte strings, before the generic quote path sees the quote. The hash count is
  // parked in the state so the closing `"###` can be matched against it.
  const raw = stream.match(/^(?:b?r)(#*)"/) as RegExpMatchArray | null
  if (raw) {
    state.quote = '"'
    state.hashes = raw[1]?.length ?? 0
    return 'string'
  }

  // `#[…]` and `#![…]`. Consumed to the matching bracket rather than to end of line: a
  // trailing `// note` after an attribute is a comment, not part of it.
  if (stream.match(/^#!?\[/)) {
    let depth = 1
    while (depth > 0 && !stream.eol()) {
      const ch = stream.next()
      if (ch === '[') depth++
      else if (ch === ']') depth--
    }
    return 'meta'
  }

  // A lifetime: `'` then an identifier not followed by a closing `'`. The negative
  // lookahead is the whole distinction from a character literal.
  if (stream.match(/^'(?:static|_|[a-z][A-Za-z0-9_]*)(?!')/)) return 'typeName'

  // `println!`, `vec!`, `matches!`. The `(?!=)` keeps `a != b` out of it.
  if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*!(?!=)/)) return 'macroName'

  return null
}

export const spec = grammar({
  name: 'Rust',
  keywords: KEYWORDS,
  atoms: ATOMS,
  types: TYPES,
  lineComment: '//',
  blockComment: ['/*', '*/'],
  // Rust block comments nest, and this is not pedantry: commenting out a region that
  // already contains a `/* */` is routine, and a non-nesting scanner ends the comment at
  // the inner `*/` and paints the rest of the function as code.
  nestedComments: true,
  capitalisedIsType: true,
  callSyntax: true,
  // The mock gives `?` its own `--red`, which is right: it is the one operator that changes
  // where control goes.
  controlOperators: '?',
  hook,
})
