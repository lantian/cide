/**
 * `when`-clause evaluation.
 *
 * `Binding::when` and `Command::when` carry a context expression — `terminalFocused`,
 * `claudePaneFocused && !overlayOpen` — and `cide-core::commands` states plainly that "the
 * frontend supplies the context flags". This is that evaluator, and it is the only one:
 * both the keymap and the palette's applicability filter go through [`evaluateWhen`], so a
 * command that a key can reach is exactly a command the palette will offer.
 *
 * The grammar is the small subset the shipped vocabulary needs: identifiers, `!`, `&&`,
 * `||` and parentheses, with `&&` binding tighter than `||`. VS Code's real grammar also has
 * `==`, `=~`, `in` and key-value contexts; none of those appear in `commands.rs` and adding
 * them speculatively would be four more ways to be subtly wrong about a clause nobody
 * writes.
 *
 * An unknown identifier is `false` rather than an error: contexts are added over time, and
 * a binding guarded on a flag this build does not know about must not fire.
 *
 * A clause that does not parse is also `false`. That is the safe direction — a broken
 * clause leaves the key unbound, so the keystroke reaches the terminal or the editor
 * untouched. `true` would have a typo silently *steal* a key, which is strictly worse: the
 * user loses a keystroke they rely on and nothing on screen explains why.
 *
 * DOM-free and import-free on purpose; `ui/scripts/check-key-gate.mjs` compiles it directly.
 */

/** Context flags the app publishes for `when` clauses to test. Unknown names read `false`. */
export type KeyContext = Readonly<Record<string, boolean>>

type Token =
  | { kind: 'ident'; text: string }
  | { kind: 'and' }
  | { kind: 'or' }
  | { kind: 'not' }
  | { kind: 'open' }
  | { kind: 'close' }

/** `null` when the text contains something this grammar does not accept. */
function tokenize(text: string): Token[] | null {
  const out: Token[] = []
  let i = 0
  while (i < text.length) {
    const ch = text[i] as string
    if (ch === ' ' || ch === '\t' || ch === '\n' || ch === '\r') {
      i += 1
      continue
    }
    if (ch === '(') {
      out.push({ kind: 'open' })
      i += 1
      continue
    }
    if (ch === ')') {
      out.push({ kind: 'close' })
      i += 1
      continue
    }
    if (ch === '!') {
      out.push({ kind: 'not' })
      i += 1
      continue
    }
    if (text.startsWith('&&', i)) {
      out.push({ kind: 'and' })
      i += 2
      continue
    }
    if (text.startsWith('||', i)) {
      out.push({ kind: 'or' })
      i += 2
      continue
    }
    const match = /^[A-Za-z_][A-Za-z0-9_.]*/.exec(text.slice(i))
    if (match === null) return null
    out.push({ kind: 'ident', text: match[0] })
    i += match[0].length
  }
  return out
}

/**
 * Recursive descent over the token list.
 *
 * Written as a closure over a cursor rather than a class: the whole grammar is three
 * productions and a class would add a constructor and a field for a value that never
 * escapes this function.
 */
function parseAndEval(tokens: Token[], ctx: KeyContext): boolean | null {
  let at = 0
  const peek = (): Token | undefined => tokens[at]

  function primary(): boolean | null {
    const token = peek()
    if (token === undefined) return null
    if (token.kind === 'not') {
      at += 1
      const inner = primary()
      return inner === null ? null : !inner
    }
    if (token.kind === 'open') {
      at += 1
      const inner = or()
      if (inner === null) return null
      if (peek()?.kind !== 'close') return null
      at += 1
      return inner
    }
    if (token.kind === 'ident') {
      at += 1
      return ctx[token.text] === true
    }
    return null
  }

  function and(): boolean | null {
    let left = primary()
    if (left === null) return null
    while (peek()?.kind === 'and') {
      at += 1
      const right = primary()
      if (right === null) return null
      // No short-circuit: the right-hand side still has to be *parsed*, or a syntax error
      // hiding behind a false left operand would silently evaluate to `false` — the same
      // answer a correct clause gives, so the mistake would never surface.
      left = left && right
    }
    return left
  }

  function or(): boolean | null {
    let left = and()
    if (left === null) return null
    while (peek()?.kind === 'or') {
      at += 1
      const right = and()
      if (right === null) return null
      left = left || right
    }
    return left
  }

  const value = or()
  if (value === null || at !== tokens.length) return null
  return value
}

/**
 * Does this clause hold in this context?
 *
 * `null`, `undefined` and an all-whitespace clause mean "always", matching `when: None` on
 * the Rust side.
 */
export function evaluateWhen(clause: string | null | undefined, ctx: KeyContext): boolean {
  if (clause === null || clause === undefined) return true
  const text = clause.trim()
  if (text === '') return true

  const tokens = tokenize(text)
  if (tokens === null) return false
  return parseAndEval(tokens, ctx) ?? false
}
