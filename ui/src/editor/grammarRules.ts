/**
 * Turning a contributed language's `rules` into a grammar hook. (M22)
 *
 * A builtin language reaches the irregular parts of its syntax through `GrammarSpec.hook`, which
 * is a function: Rust's lifetimes, Go's rune literals, Markdown's headings. No JSON can express
 * those, so an extension gets this instead — a list of *"this pattern, in this position, is that
 * colour"* rules, compiled here into the one hook the tokenizer already knows how to call.
 *
 * That is deliberately less than a hook and deliberately enough for the shape of hook that turns
 * up over and over. YAML's whole hook is three of these, and translating it faithfully is what
 * tells us the rule language is sufficient rather than merely plausible — `check-editor.mjs`
 * asserts the translation produces the same tokens as the function it replaces, over the same
 * corpus.
 *
 * # Two ways this can take a buffer down, and what stops each
 *
 * **A rule that matches without consuming.** `StreamLanguage` throws *"Stream parser failed to
 * advance stream"* after ten no-op calls, which does not mis-colour a buffer, it unmounts it. A
 * pattern like `(?=:)` is a perfectly reasonable thing for an author to write and does exactly
 * that. So every match is checked against the stream position and a zero-width one is treated as
 * no match at all — the loop moves on, and the generic path gets its turn.
 *
 * **A rule that is quadratic.** `data.ts` records the measurement: YAML's key pattern scans to the
 * end of the line before its lookahead can fail, so running it once per token made a 200,000-
 * character line 4.8 s against 20 ms for the same line tried at the line head only. That is why
 * `at` exists and why `lineStart` is the value an author reaches for; nothing here can stop
 * somebody writing an expensive `anywhere` rule, but the field makes the cheap answer the obvious
 * one.
 *
 * Import-free apart from two types, so `ui/scripts/check-editor.mjs` can compile it standalone.
 */
import type { StringStream } from '@codemirror/language'

import { atLineStart, type HookResult } from './streamGrammar'

/** One rule, as a manifest writes it. Mirrors `cide_ipc::lang::GrammarRule`. */
export interface RuleSpec {
  readonly pattern: string
  readonly tag: string
  readonly at?: 'lineStart' | 'anywhere'
}

/** What went wrong with one rule, for the Extensions panel. */
export interface RuleProblem {
  readonly at: number
  readonly message: string
}

export interface CompiledRules {
  readonly hook: ((stream: StringStream) => HookResult) | null
  readonly problems: readonly RuleProblem[]
}

/**
 * Anchor a pattern at the stream position.
 *
 * `StringStream.match` matches at the current position only when the regex is anchored; an
 * unanchored one is allowed to scan forward, find its pattern later in the line, and consume
 * everything up to it. That is not a subtle mis-colouring — it swallows the tokens in between —
 * and an author writing `key:` rather than `^key:` has no way to know. So the `^` is added when it
 * is missing rather than demanded in the manifest.
 */
function anchored(pattern: string): RegExp {
  return new RegExp(pattern.startsWith('^') ? pattern : `^${pattern}`)
}

/**
 * Compile a rule list into a hook, reporting the rules that could not be used.
 *
 * A bad rule is dropped and the rest still run: an extension whose fourth rule has a typo should
 * colour what its first three describe, not lose its syntax highlighting entirely.
 */
export function compileRules(rules: readonly RuleSpec[]): CompiledRules {
  const problems: RuleProblem[] = []
  const compiled: { lineStart: boolean; re: RegExp; tag: string }[] = []

  rules.forEach((rule, at) => {
    if (rule.pattern === '' || rule.tag === '') {
      problems.push({ at, message: 'a rule needs both a pattern and a tag' })
      return
    }
    let re: RegExp
    try {
      re = anchored(rule.pattern)
    } catch (error) {
      problems.push({ at, message: `${rule.pattern} is not a regular expression: ${String(error)}` })
      return
    }
    compiled.push({ lineStart: rule.at === 'lineStart', re, tag: rule.tag })
  })

  if (compiled.length === 0) return { hook: null, problems }

  const hook = (stream: StringStream): HookResult => {
    // Computed once per token rather than per rule: `atLineStart` walks the line prefix, and a
    // language with six line-start rules would otherwise walk it six times for every token.
    let first: boolean | null = null
    for (const rule of compiled) {
      if (rule.lineStart) {
        first ??= atLineStart(stream)
        if (!first) continue
      }
      const before = stream.pos
      // Called once and the result kept: a second `match` for the same rule would consume a
      // second time, which is a token silently eaten rather than an error anybody sees.
      const hit = stream.match(rule.re)
      if (hit === null || hit === false) continue
      // It matched. If it consumed nothing the rule is a bare lookahead, and reporting a tag for
      // it is what raises "Stream parser failed to advance stream" ten tokens later — so treat it
      // as no match and let the next rule, or the generic path, have the token.
      if (stream.pos > before) return rule.tag
    }
    return null
  }
  return { hook, problems }
}
