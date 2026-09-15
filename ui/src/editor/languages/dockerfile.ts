/**
 * Dockerfile, and `Containerfile`.
 *
 * # Why this is not the shell grammar with a keyword list
 *
 * It was, until M45 — `"dockerfile"` sat in the shell entry's `filenames` and a `FROM` line was
 * highlighted as a shell command that happened to start with a word. Four things make a
 * Dockerfile genuinely a different language, and each of them is a hook below:
 *
 *  - **An instruction is only an instruction at the start of a line.** `RUN` in
 *    `RUN echo run` is a keyword once and a word twice. A keyword *list* has no such notion, so
 *    every occurrence would paint, and `apt-get install copy` would light up in the middle of a
 *    shell command.
 *  - **`# syntax=` and `# escape=` are directives, not comments.** They are only directives in
 *    the leading comment block, they change how the file is parsed, and colouring them as prose
 *    is how a broken `# syntax=` goes unnoticed.
 *  - **A stage has a name.** `FROM node:20 AS build` and `COPY --from=build` are a definition and
 *    a reference to it, and they are the structure of every multi-stage file.
 *  - **`${VAR}` expands inside strings**, with `:-` and `:+` defaults, which no shell-quoting
 *    rule reproduces.
 *
 * # What this deliberately does not do
 *
 * The body of a `RUN` is shell and is **not** highlighted as shell. A nested grammar would need
 * the stream parser to carry a second state machine, and the failure it produces when it goes
 * wrong is a whole file painted as an unterminated string. The instruction, its flags, its
 * variable expansions and its strings are coloured; the command inside is left as plain text,
 * which is honest rather than approximate.
 */
import type { StringStream } from '@codemirror/language'
import { atLineStart, grammar, type HookResult } from '../streamGrammar'

/**
 * Every instruction, including the ones nobody writes any more.
 *
 * `MAINTAINER` is deprecated and still appears in a great many files; a file that uses it should
 * not look broken. The list is exhaustive against the Dockerfile reference rather than a
 * selection, because an instruction that is *missing* here reads on screen as a typo.
 */
const INSTRUCTIONS = [
  'ADD', 'ARG', 'CMD', 'COPY', 'ENTRYPOINT', 'ENV', 'EXPOSE', 'FROM', 'HEALTHCHECK', 'LABEL',
  'MAINTAINER', 'ONBUILD', 'RUN', 'SHELL', 'STOPSIGNAL', 'USER', 'VOLUME', 'WORKDIR',
]

/**
 * An instruction at the start of a line, case-insensitively.
 *
 * Docker itself accepts any case and the convention is upper — a file written in lower case is
 * valid and common enough that refusing to colour it would look like cide not understanding the
 * file. The trailing `\s` is required: it is what stops `FROMAGE` matching `FROM`.
 */
const INSTRUCTION = new RegExp(`^(${INSTRUCTIONS.join('|')})(?=\\s|$)`, 'i')

/**
 * A parser directive: `# syntax=docker/dockerfile:1`, `# escape=\`.
 *
 * Only in the leading comment block, which is why `state.directives` below stops being true at
 * the first non-comment line. A `# syntax=` further down the file is a *comment* — BuildKit
 * ignores it — and colouring it as a directive would tell the reader the opposite of the truth
 * about a file that is not doing what they think.
 */
const DIRECTIVE = /^#\s*(syntax|escape|check)\s*=/i

/**
 * How far past a `${` the closing `}` is looked for.
 *
 * `shell.ts`'s bound, for its reason and its measurement: `\{[^}]*\}` scans to the end of the
 * line for every `${` that is never closed and then backtracks over the whole scan, so a line of
 * `${${${…` is quadratic.
 */
const BRACE_SCAN_LIMIT = 512

/** `$VAR` and `${VAR}`, including `${VAR:-default}` and `${VAR:+alt}`. */
const EXPANSION = new RegExp(
  `^\\$(?:\\{[^}]{0,${BRACE_SCAN_LIMIT}}\\}|[A-Za-z_][A-Za-z0-9_]*)`,
)

/** A flag on an instruction: `--from=build`, `--mount=type=cache,target=/root/.cache`, `--chown`. */
const FLAG = /^--[A-Za-z][A-Za-z0-9-]*/

/** `AS <name>`, which names a build stage. */
const AS = /^as(?=\s)/i

/**
 * `ONBUILD` is the one place two instructions are adjacent, and this asks the line about it.
 *
 * Stateless on purpose. `GrammarState` is a fixed shape shared by every grammar and has no field
 * for "the previous token was ONBUILD"; the first version of this widened it with a cast, which
 * is a type hole in a file that has no other reason for one. The stream carries the whole line
 * and the current token's start offset, so the question can be asked directly — and the answer
 * is the same one carried state would have given, without a second place for it to go stale.
 */
function afterOnbuild(stream: StringStream): boolean {
  return /^\s*onbuild\s+$/i.test(stream.string.slice(0, stream.start))
}

/**
 * The irregular half of the language. Everything a keyword list cannot express.
 *
 * # The one deliberate inaccuracy, stated
 *
 * A parser directive must be in the file's **leading comment block**, and whether a line is
 * still in that block is a question about the document that a stream parser cannot ask — it sees
 * one line at a time. So this asks the narrower question it can: does this line's first token
 * spell a directive. A `# syntax=` in the middle of a file is therefore coloured as a directive
 * when BuildKit would read it as an ordinary comment.
 *
 * That is wrong in the safe direction and the choice is the point. The failure it produces is a
 * comment that looks important; the failure the other way round is a **broken `# syntax=` that
 * looks like prose**, which is the one that costs somebody an afternoon.
 */
function hook(stream: StringStream): HookResult {
  if (atLineStart(stream)) {
    // Directives first: `#` would otherwise make this a comment, because the generic path takes
    // `lineComment` before the hook gets to look at what follows it.
    if (stream.match(DIRECTIVE, false)) {
      stream.skipToEnd()
      return 'meta'
    }
    if (stream.match(INSTRUCTION)) return 'keyword'
    return null
  }

  // The instruction after `ONBUILD`, and only there.
  if (afterOnbuild(stream) && stream.match(INSTRUCTION)) return 'keyword'

  // `AS build` — the definition half of a multi-stage file.
  if (stream.match(AS)) return 'keyword'
  // `--from=build`, `--mount=…`, `--chown=…`.
  if (stream.match(FLAG)) return 'propertyName'
  // Coloured as a name being read rather than written, which is `shell.ts`'s choice and the
  // closest honest role.
  if (stream.match(EXPANSION)) return 'propertyName'
  return null
}

export const spec = grammar({
  name: 'Dockerfile',
  // Deliberately **empty**. Every keyword in this language is position-sensitive and is decided
  // by the hook; a list here would paint `RUN` inside `RUN echo run`.
  keywords: [],
  lineComment: '#',
  // Stated rather than left to the default, because `check:editor` compares this against the
  // Rust fold spec's `quotes` and a default on one side with a value on the other is a
  // disagreement it reports. Both appear in `ENV` and `LABEL` values and inside a `RUN` line.
  quotes: '"\'',
  // A hyphen continues a word, so `--no-install-recommends` is one token rather than a word and
  // four operators. `shell.ts`'s reason, and a Dockerfile is mostly shell arguments.
  identifierExtra: '-',
  // `FROM node:20` is not a type, and `RUN foo(bar)` is a shell subshell rather than a call —
  // both heuristics would paint a great deal of every file wrong. `shell.ts` turns both off for
  // the same reason.
  capitalisedIsType: false,
  callSyntax: false,
  hook,
})
