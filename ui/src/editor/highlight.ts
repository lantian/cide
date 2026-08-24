/**
 * The one table that says what colour a token is.
 *
 * Two consumers need that answer and they need it in different forms. The buffer needs a
 * CSS class, because a class resolves `var(--purple)` at paint time and therefore follows a
 * live theme switch with no work at all. The minimap needs a concrete `#rrggbb`, because it
 * paints into a canvas and canvas has never heard of a custom property.
 *
 * Keeping both derived from one array is the whole point of this file. The alternative —
 * a `HighlightStyle` here and a colour lookup in `minimap.ts` — was tried and is exactly
 * the sort of duplication that ends with a keyword being purple in the text and blue in
 * the map.
 */
import { HighlightStyle } from '@codemirror/language'
import { tags as t } from '@lezer/highlight'
import type { Tag } from '@lezer/highlight'

/**
 * A token role, its CSS class, and the colour-scheme token that paints it.
 *
 * # The roles are more than the mock's six, and more than they were
 *
 * The first six were the mock's own list for Rust: keywords, types, functions,
 * attributes/strings, comments, `?`. The rest are the roles a real buffer has that a screenshot
 * of forty lines did not happen to show.
 *
 * Twelve of them collapsed further than they had to, and M24 split the ones that were costing
 * something. The headline is `propertyName`: it painted `--text`, and `.cm-editor` is also
 * `color: var(--text)`, so a JSON key — the one piece of structure a JSON file has, which
 * `languages/data.ts` goes to real positional trouble to identify — was pixel-identical to
 * unhighlighted prose. `atom` is the other: it is a subtag of `keyword`, so `true`/`false`/`null`
 * inherited the keyword colour and a config file's booleans were the same purple as a Rust `fn`.
 *
 * The rest of the new roles (`doc`, `escape`, `regexp`, `namespace`, `macro`, `label`, `bracket`,
 * `punctuation`, `strong`, `link`, `variable`) mostly hold the value they used to share, and are
 * split anyway. That is not busywork: a role is the unit an *imported* scheme can speak about, so
 * a role that does not exist is a colour a VS Code theme has no way to give us. See
 * `cide_core::scheme::SCOPES`, which is the other half of this table.
 *
 * # Bare `variableName` is deliberately not a role
 *
 * An ordinary identifier is body text. Colouring every one of them is how a syntax theme turns
 * into noise, and `check-editor.mjs` pins `classOf('variableName') === null` so it stays that way.
 *
 * # Order is precedence
 *
 * `HighlightStyle` resolves a tag through its parent chain and takes the first matching entry, so
 * a role claiming a *child* tag must be listed before the role claiming its parent. Every such
 * pair in this table is called out in a comment where it appears, because the failure is silent:
 * the wrong role wins and nothing anywhere fails.
 */
export interface TokenRole {
  /** Class emitted into the buffer's DOM, and the minimap's lookup key. */
  cls: string
  /**
   * The custom property that colours it, without the `var()`.
   *
   * Always a `--tk-*`, never a chrome token. Until M24 these were `--purple`, `--yellow` and so
   * on — the same six the terminal's ANSI palette reads, so retuning a syntax colour repainted
   * every terminal in the app. `tokens.css` carries the argument at length.
   */
  token: string
  /** Lezer tags that resolve to this role, most specific first. */
  tags: readonly Tag[]
}

export const TOKEN_ROLES: readonly TokenRole[] = [
  // `docComment` and `docString` are subtags of `comment` and `string`; both roles that claim
  // those parents are below.
  { cls: 'cide-tk-doc', token: '--tk-doc', tags: [t.docComment, t.docString] },
  { cls: 'cide-tk-comment', token: '--tk-comment', tags: [t.comment] },
  // `?` in Rust, and the equivalent control-flow punctuation elsewhere. Listed before the
  // general operator role because `controlOperator` is a subtag of `operator`.
  { cls: 'cide-tk-control', token: '--tk-control', tags: [t.controlOperator, t.invalid] },
  // `true`, `false`, `null`, `self`. Four of these five are subtags of `keyword` and `bool` is a
  // subtag of `literal`, so this must precede both the keyword role and the number role — which
  // claims `literal` — or a config file's booleans go back to being keywords.
  {
    cls: 'cide-tk-constant',
    token: '--tk-constant',
    tags: [t.atom, t.bool, t.self, t.null, t.unit],
  },
  // `escape` and `regexp` are subtags of `literal`, so both precede the number role for the
  // reason above. `escape` before `string` as well, so a `\n` is visible inside the string it
  // sits in rather than disappearing into it.
  { cls: 'cide-tk-escape', token: '--tk-escape', tags: [t.escape, t.special(t.string)] },
  { cls: 'cide-tk-regexp', token: '--tk-regexp', tags: [t.regexp] },
  // `attributeName` is a subtag of `propertyName`, so this precedes the property role — which is
  // how a `#[derive]` stays an attribute rather than becoming a key.
  {
    cls: 'cide-tk-attribute',
    token: '--tk-attribute',
    tags: [t.meta, t.attributeName, t.processingInstruction],
  },
  // `url` stays here rather than moving to the link role: it is a subtag of `literal`, and the
  // link role sits below the number role, which claims `literal`. A URL in a string is a string.
  { cls: 'cide-tk-string', token: '--tk-string', tags: [t.string, t.url] },
  { cls: 'cide-tk-macro', token: '--tk-macro', tags: [t.macroName] },
  { cls: 'cide-tk-label', token: '--tk-label', tags: [t.labelName] },
  {
    cls: 'cide-tk-function',
    token: '--tk-function',
    tags: [t.function(t.variableName), t.function(t.propertyName)],
  },
  { cls: 'cide-tk-namespace', token: '--tk-namespace', tags: [t.namespace] },
  // `typeOperator` is a subtag of `operator`; the operator role is below.
  { cls: 'cide-tk-type', token: '--tk-type', tags: [t.typeName, t.className, t.typeOperator] },
  // After the constant role, which takes the four `keyword` subtags that wanted their own
  // colour. `keyword` covers the rest (`modifier`, `controlKeyword`, …) on its own.
  { cls: 'cide-tk-keyword', token: '--tk-keyword', tags: [t.keyword] },
  { cls: 'cide-tk-number', token: '--tk-number', tags: [t.number, t.literal] },
  { cls: 'cide-tk-property', token: '--tk-property', tags: [t.propertyName] },
  // Split out of the property role. Nothing in cide's own grammars emits it today; an extension
  // may, and an imported theme's `variable` scope needs somewhere to land.
  { cls: 'cide-tk-variable', token: '--tk-variable', tags: [t.definition(t.variableName)] },
  // `bracket` is a subtag of `punctuation`, which is a sibling of `operator`. All three were one
  // role; they are three so that a scheme can quiet brackets without quieting `+`.
  { cls: 'cide-tk-bracket', token: '--tk-bracket', tags: [t.bracket] },
  {
    cls: 'cide-tk-punctuation',
    token: '--tk-punctuation',
    tags: [t.punctuation, t.separator],
  },
  { cls: 'cide-tk-operator', token: '--tk-operator', tags: [t.operator] },
  { cls: 'cide-tk-heading', token: '--tk-heading', tags: [t.heading] },
  // Bold shared the heading role until M24. Both are subtags of `content`, which nothing
  // claims, so the order between them and the three below carries no precedence.
  { cls: 'cide-tk-strong', token: '--tk-strong', tags: [t.strong] },
  {
    cls: 'cide-tk-emphasis',
    token: '--tk-emphasis',
    tags: [t.emphasis, t.quote, t.monospace],
  },
  { cls: 'cide-tk-link', token: '--tk-link', tags: [t.link] },
]

/**
 * The colour a run of text with no role at all is painted, in the buffer and the map.
 *
 * `--tk-fg` and not `--text`: this is the editor's ink, so an imported scheme owns it. The two
 * hold the same value in the cide scheme, and that is a coincidence a reader should not rely on.
 */
export const PLAIN_TOKEN = '--tk-fg'

/**
 * The buffer's highlighter.
 *
 * `class` rather than `color`: a generated class carrying a literal colour would be baked
 * into a stylesheet at construction time and would still say `#b48ead` after the user
 * switched to the light palette. Naming our own class instead moves the colour into
 * `editor/highlight.css`, where it is a `var(--tk-*)` and follows both the theme and the
 * selected colour scheme for free.
 *
 * This object is also a `Highlighter`, which is what lets `minimap.ts` run `highlightTree`
 * over the same rules rather than reimplementing them.
 */
export const cideHighlightStyle = HighlightStyle.define(
  TOKEN_ROLES.flatMap((role) => role.tags.map((tag) => ({ tag, class: role.cls }))),
)

/** Class → custom-property name, for the canvas that cannot read a class. */
export const TOKEN_VAR_BY_CLASS: ReadonlyMap<string, string> = new Map(
  TOKEN_ROLES.map((role) => [role.cls, role.token]),
)

/**
 * The `Tag`s a stream parser's token *name* stands for. (M20)
 *
 * `@codemirror/language` does this with `createTokenType`, which it does not export, so this is
 * that function's rule and only that rule: split on `.`, look each part up in `tags`, and treat a
 * function-valued part as a modifier applied to what came before. That is how
 * `variableName.function` becomes `t.function(t.variableName)`.
 *
 * It lived inside `ui/scripts/check-editor.mjs` until M20, with a comment calling itself "the
 * smallest possible restatement" — which was right while the check was its only caller. The
 * markdown preview is the second: `markdown/fenceTokens.ts` drives the same parsers by hand,
 * outside any `EditorView`, and needs the same name → class answer the buffer gets. Two copies of
 * a restatement of somebody else's unexported function is one copy too many, so the check now
 * calls this and pins it.
 */
export function tagsFor(name: string): readonly Tag[] {
  let found: readonly Tag[] = []
  for (const part of name.split('.')) {
    const value: unknown = (t as unknown as Record<string, unknown>)[part]
    if (value === undefined) return []
    if (typeof value === 'function') {
      const modifier = value as (tag: Tag) => Tag
      found = found.map(modifier)
    } else {
      found = Array.isArray(value) ? (value as Tag[]) : [value as Tag]
    }
  }
  return found
}

/**
 * The `cide-tk-*` class a stream parser's token name paints as, or null for no role.
 *
 * The same answer `cideHighlightStyle` gives the buffer, by construction — it *is*
 * `cideHighlightStyle`, asked directly rather than through an `EditorView`. That is the whole
 * point: a preview that coloured a `rust` fence from a second table would drift from the buffer
 * beside it in split mode, where the two are eight pixels apart.
 */
export function tokenClassFor(name: string): string | null {
  const tags = tagsFor(name)
  if (tags.length === 0) return null
  return cideHighlightStyle.style(tags) ?? null
}
