/**
 * SQL — the dialect-agnostic core, plus what PostgreSQL, SQLite and MySQL agree on.
 *
 * # Why this file exists rather than a `{ label: 'SQL', ext: 'sql' }` row on its own
 *
 * `SCRATCH_TYPES` is pinned against `lookup()` by `check-editor.mjs`: for every offered type,
 * the label the picker prints must be the label the status bar will print **and** a grammar
 * must actually load. So a row with no grammar behind it is not "a scratch that opens as plain
 * text" — it is a build failure, which is the gate doing exactly its job. There is no
 * `@codemirror/lang-sql` in this project (`ui/package.json` lists six `@codemirror/*` packages
 * and none of them is a grammar) and adding one is the ~100 KB-of-parser-tables decision the
 * whole `streamGrammar.ts` arrangement was made to avoid. A data module is the price of the
 * row, and it is 60 lines.
 *
 * # The three ways SQL is not C
 *
 * 1. **Keywords are case-insensitive.** `SELECT` and `select` are one word, and the shouting
 *    style is the common one. Hence `caseInsensitiveKeywords`, which is added to
 *    `streamGrammar.ts` for this file and used by nothing else.
 * 2. **A quote is escaped by doubling it, not by a backslash.** `'it''s'` is one string
 *    containing an apostrophe. With `escapes: false` the tokenizer sees two adjacent `string`
 *    runs — `'it'` and `'s'` — which paints identically and, unlike `escapes: true`, does not
 *    then treat `'C:\path\'` as an unterminated string that swallows the rest of the file. The
 *    Go grammar's raw-string comment makes the same trade.
 * 3. **A capitalised word is not a type, and `name(` is not a call.** `Users` is a table;
 *    `capitalisedIsType: false`, or every table name in a shouted query comes out purple. The
 *    call-syntax lookahead has to go with it, and that one was found by the check rather than
 *    reasoned about: `CREATE TABLE Sessions (` and `REFERENCES Projects(id)` are a table
 *    followed by a bracket, indistinguishable from `count(s.id)` to anything without a parser,
 *    so `callSyntax` painted every table in a schema as a function. The aggregates and scalar
 *    functions are named in `BUILTINS` instead, which is exact where the lookahead was a guess
 *    — and the cost is that a user-defined function is drawn as an ordinary identifier, which
 *    is the failure in the harmless direction.
 *
 * `"quoted identifiers"` are coloured as strings, which is the one knowing inaccuracy here:
 * telling them from string literals needs the dialect (MySQL uses backticks and reads `"` as a
 * string by default), and a lexer has no dialect. Colouring them as strings is what every
 * `StreamLanguage` SQL mode does.
 *
 * No hook, therefore **no new tag names** — which matters: `check-editor.mjs` globs this
 * directory, collects every `return '<tag>'`, and fails on any tag `codeIntelGate` does not
 * classify. A hookless spec cannot add one.
 */
import { grammar } from '../streamGrammar'

/*
 * Lowercase, once. `caseInsensitiveKeywords` folds the token before the lookup, so a table
 * written twice in two cases is the thing this list must not become.
 */
const KEYWORDS = [
  'add', 'all', 'alter', 'analyze', 'and', 'as', 'asc', 'attach', 'begin', 'between', 'by',
  'cascade', 'case', 'cast', 'check', 'collate', 'column', 'commit', 'conflict', 'constraint',
  'create', 'cross', 'database', 'default', 'deferrable', 'delete', 'desc', 'distinct', 'do',
  'drop', 'else', 'end', 'escape', 'except', 'exclude', 'exists', 'explain', 'filter',
  'foreign', 'from', 'full', 'grant', 'group', 'having', 'if', 'ignore', 'in', 'index',
  'inner', 'insert', 'intersect', 'into', 'is', 'join', 'key', 'left', 'like', 'limit',
  'materialized', 'natural', 'not', 'nulls', 'offset', 'on', 'or', 'order', 'outer', 'over',
  'partition', 'primary', 'recursive', 'references', 'reindex', 'rename', 'replace',
  'restrict', 'returning', 'revoke', 'right', 'rollback', 'row', 'savepoint', 'select', 'set',
  'table', 'temporary', 'then', 'to', 'transaction', 'trigger', 'union', 'unique', 'update',
  'using', 'vacuum', 'values', 'view', 'when', 'where', 'window', 'with',
]

/** Keyword-shaped literals. `default` is a keyword above; these are values. */
const ATOMS = ['true', 'false', 'null', 'unknown', 'current_date', 'current_time', 'current_timestamp']

const TYPES = [
  'bigint', 'blob', 'bool', 'boolean', 'bytea', 'char', 'character', 'date', 'datetime',
  'decimal', 'double', 'float', 'int', 'int2', 'int4', 'int8', 'integer', 'interval', 'json',
  'jsonb', 'money', 'numeric', 'precision', 'real', 'serial', 'smallint', 'text', 'time',
  'timestamp', 'timestamptz', 'tinyint', 'uuid', 'varchar', 'varying',
]

/** Aggregates and the handful of scalar functions every dialect has. Coloured as calls. */
const BUILTINS = [
  'abs', 'avg', 'coalesce', 'concat', 'count', 'greatest', 'group_concat', 'least', 'length',
  'lower', 'max', 'min', 'now', 'nullif', 'round', 'substr', 'substring', 'sum', 'trim',
  'upper',
]

export const spec = grammar({
  name: 'SQL',
  keywords: KEYWORDS,
  atoms: ATOMS,
  types: TYPES,
  builtins: BUILTINS,
  caseInsensitiveKeywords: true,
  lineComment: '--',
  blockComment: ['/*', '*/'],
  // Standard SQL does not nest block comments, and treating it as if it did mis-colours a
  // `/* … */` containing the characters `/*` inside a string. Same call `clike.ts` makes.
  nestedComments: false,
  // `"` is a quoted identifier and `'` is a string literal. Both are consumed whole and painted
  // as strings; see the module header for why a lexer cannot do better here. The backtick is
  // MySQL's identifier quote and is included for the same reason.
  quotes: '\'"`',
  // A doubled quote, not a backslash. See the module header.
  escapes: false,
  // `Users` is a table, not a type.
  capitalisedIsType: false,
  // And `Sessions (` is a table, not a call. See the module header — `BUILTINS` above does this
  // job exactly, where the lookahead did it by guessing.
  callSyntax: false,
})
