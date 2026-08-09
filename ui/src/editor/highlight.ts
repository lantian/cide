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
 * A token role, its CSS class, and the design token that colours it.
 *
 * The first six roles are the mock's own list for Rust: keywords `--purple`, types
 * `--cyan`, functions `--blue`, attributes/strings `--yellow`, comments `--faint`, `?`
 * `--red`. The rest are the roles a real buffer has that a screenshot of forty lines did
 * not happen to show, and they are deliberately quiet — a syntax theme that colours
 * punctuation as loudly as it colours keywords reads as noise.
 */
export interface TokenRole {
  /** Class emitted into the buffer's DOM, and the minimap's lookup key. */
  cls: string
  /** The `tokens.css` custom property, without the `var()`. */
  token: string
  /** Lezer tags that resolve to this role, most specific first. */
  tags: readonly Tag[]
}

export const TOKEN_ROLES: readonly TokenRole[] = [
  // `?` in Rust, and the equivalent control-flow punctuation elsewhere. Listed before the
  // general operator role because `controlOperator` is a subtag of `operator`, and
  // `HighlightStyle` resolves to the first matching entry.
  { cls: 'cide-tk-control', token: '--red', tags: [t.controlOperator, t.invalid] },
  { cls: 'cide-tk-comment', token: '--faint', tags: [t.comment] },
  {
    cls: 'cide-tk-attribute',
    token: '--yellow',
    tags: [t.meta, t.attributeName, t.processingInstruction],
  },
  { cls: 'cide-tk-string', token: '--yellow', tags: [t.string, t.regexp, t.escape, t.url] },
  {
    cls: 'cide-tk-function',
    token: '--blue',
    tags: [t.function(t.variableName), t.function(t.propertyName), t.macroName, t.labelName],
  },
  {
    cls: 'cide-tk-type',
    token: '--cyan',
    tags: [t.typeName, t.className, t.namespace, t.typeOperator],
  },
  // After the four `keyword` subtags that want their own colour would go here — there are
  // none; the mock paints every keyword shade the same purple, and `keyword` covers its
  // subtags (`self`, `null`, `atom`, `modifier`, `controlKeyword`, …) on its own.
  { cls: 'cide-tk-keyword', token: '--purple', tags: [t.keyword] },
  { cls: 'cide-tk-number', token: '--green', tags: [t.number, t.bool, t.literal] },
  { cls: 'cide-tk-property', token: '--text', tags: [t.propertyName, t.definition(t.variableName)] },
  { cls: 'cide-tk-operator', token: '--dim', tags: [t.operator, t.punctuation, t.bracket] },
  { cls: 'cide-tk-heading', token: '--accent', tags: [t.heading, t.strong] },
  { cls: 'cide-tk-emphasis', token: '--dim', tags: [t.emphasis, t.quote, t.link, t.monospace] },
]

/** The colour a run of text with no role at all is painted, in the buffer and the map. */
export const PLAIN_TOKEN = '--text'

/**
 * The buffer's highlighter.
 *
 * `class` rather than `color`: a generated class carrying a literal colour would be baked
 * into a stylesheet at construction time and would still say `#b48ead` after the user
 * switched to the light palette. Naming our own class instead moves the colour into
 * `EditorSurface.module.css`, where it is `var(--purple)` and follows the theme for free.
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
