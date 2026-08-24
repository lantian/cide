/**
 * Applying a colour scheme, and deciding which one applies. (M24)
 *
 * # What a scheme is, from this side
 *
 * A map from role to `#rrggbb`, written onto `<html>` as `--tk-<role>` custom properties.
 * That is the whole mechanism. `editor/highlight.css` reads those properties, `HighlightStyle`
 * was built with `class:` rather than `color:` so a class resolves them at paint time, and the
 * minimap — which paints into a canvas and cannot read a class — resolves the same properties
 * through `getComputedStyle`. So one property write repaints the buffer, both diff panes, the
 * three-pane merge resolver, the markdown preview's fenced code and the minimap, and none of
 * them has any code in it about schemes.
 *
 * The compiled-in `cide` scheme is **not** a value here. It is what `tokens.css` declares in its
 * two palette blocks, so selecting it means *clearing* the inline properties rather than writing
 * a different set — which is also what makes it the honest fallback for an id that no longer
 * names anything.
 *
 * # Keyed by polarity
 *
 * A scheme owns the editor's background, so a dark scheme under a light window would be an
 * inverted rectangle in a white app. `EditorSettings` therefore stores two ids, one per theme,
 * and [`schemeToApply`] refuses a scheme whose polarity is not the one on screen. That refusal
 * is belt-and-braces — the settings screen only offers matching schemes — but it is the guard
 * that survives a hand-edited `workspace.json`.
 *
 * # Why this module imports nothing
 *
 * `check:scheme` compiles it standalone, the same arrangement `settings/theme.ts` and
 * `editor/highlightLevel.ts` have. An import of the generated wire types would drag the whole
 * IPC contract in behind it to read four fields, and a pure module stops being checkable.
 */

/** Structurally the generated `Theme`; kept local so this file imports nothing. */
export type Polarity = 'dark' | 'light'

/**
 * The scheme that is compiled in rather than imported. Mirrors `cide_ipc::theme::BUILTIN_SCHEME`.
 */
export const BUILTIN_SCHEME = 'cide'

/**
 * The surface a buffer sits on: ground, ink, selection, the gutter's numbers and the caret.
 *
 * No `line`. The caret's line is derived from the selection in CSS rather than being a role a
 * scheme can set — `EditorSurface.module.css` has the argument, and `cide_ipc::theme::fallback`
 * restates it where an imported theme's value would otherwise have been read.
 *
 * Separate from [`SCHEME_TOKENS`] because the contrast gate needs the distinction — a role is
 * measured against this scheme's own `bg`, and measuring it against `--panel` would be checking
 * a pair that never appears on screen together.
 */
export const SCHEME_SURFACE = ['bg', 'fg', 'sel', 'gutter', 'caret'] as const

/**
 * One entry per role in `highlight.ts`'s `TOKEN_ROLES`, in that file's order.
 *
 * Mirrors `cide_ipc::theme::SCHEME_TOKENS`. The two lists, `tokens.css`'s two palette blocks and
 * `highlight.css`'s rules are four copies of one closed set, spelled in four files that cannot
 * import one another; `ui/scripts/check-scheme.mjs` pins them against each other.
 */
export const SCHEME_TOKENS = [
  'doc',
  'comment',
  'control',
  'constant',
  'escape',
  'regexp',
  'attribute',
  'string',
  'macro',
  'label',
  'function',
  'namespace',
  'type',
  'keyword',
  'number',
  'property',
  'variable',
  'bracket',
  'punctuation',
  'operator',
  'heading',
  'strong',
  'emphasis',
  'link',
] as const

/** Surface then tokens — the order `tokens.css` declares them in. */
export const SCHEME_ROLES: readonly string[] = [...SCHEME_SURFACE, ...SCHEME_TOKENS]

/** The custom property a role is written as. */
export function propertyFor(role: string): string {
  return `--tk-${role}`
}

/**
 * Structurally the generated `ColorScheme`, for this file's import-free reason.
 *
 * Deliberately the *narrowest* shape this module actually reads: an id, a polarity and a map.
 * `name` and `source` belong to the picker, not to the applier.
 */
export interface SchemeLike {
  readonly id: string
  readonly polarity: Polarity
  readonly colors: Readonly<Record<string, string>>
}

/**
 * Which scheme this window should paint, or `null` for the compiled-in one.
 *
 * `null` covers four cases that are all the same answer and must not be distinguished by the
 * caller: the builtin was chosen; the id names nothing (an imported scheme the user has since
 * removed, or a `workspace.json` copied from another machine); the id names a scheme of the
 * wrong polarity; or the scheme is missing the two roles it is not allowed to be missing.
 *
 * The last of those is the guard against a hand-edited file in `~/.config/cide/schemes/`. Rust
 * normalises on the way in and on the way out, so a scheme arriving over the wire is total —
 * but this module is the one that writes to the DOM, and a scheme with no `bg` would paint a
 * transparent editor over whatever is behind it.
 */
export function schemeToApply(
  schemes: readonly SchemeLike[],
  id: string,
  theme: Polarity,
): SchemeLike | null {
  if (id === BUILTIN_SCHEME) return null
  const found = schemes.find((s) => s.id === id)
  if (!found || found.polarity !== theme) return null
  if (!found.colors.bg || !found.colors.fg) return null
  return found
}

/**
 * Write a scheme's properties onto an element, or clear every one of them for `null`.
 *
 * **Always clears the whole set first.** Writing only what the new scheme carries would leave a
 * role from the previous one behind, which is a buffer painted from two themes at once — and it
 * is the state a partial write produces silently, since neither scheme is wrong about the roles
 * it does define.
 *
 * Only roles in [`SCHEME_ROLES`] are written. Rust already drops everything else
 * (`ColorScheme::normalise`), and doing it again here is what lets a future reader treat this
 * function as safe against a value that did not come through that path.
 */
export function applyScheme(root: HTMLElement, scheme: SchemeLike | null): void {
  for (const role of SCHEME_ROLES) {
    root.style.removeProperty(propertyFor(role))
  }
  if (!scheme) return
  for (const role of SCHEME_ROLES) {
    const value = scheme.colors[role]
    if (value) root.style.setProperty(propertyFor(role), value)
  }
}

/** What a window is currently painting: the inputs, and the scheme they resolved to. */
export interface PaintedScheme {
  readonly theme: Polarity
  readonly id: string
  readonly scheme: SchemeLike | null
}

/**
 * Whether the properties on `<html>` still describe what should be on screen.
 *
 * A pure decision here rather than an inline comparison in `useSettings.ts`, on that file's own
 * rule — values in, values out, so the thing that can be wrong on its own is pinned by a check
 * rather than only by a screenshot. `themeToAdopt` in `settings/theme.ts` is the same shape for
 * the same reason.
 *
 * **The scheme is compared by reference, and that is the whole correctness of the guard.** Two
 * things can move while the id stands still:
 *
 *  - The schemes list arriving *after* the setting that names it. An import patches
 *    `editor.colorSchemeDark` and broadcasts `cide://schemes-changed`, and the two reach a
 *    window in whichever order the event loop hands them over. With a `theme:id` key the first
 *    one latches: the id resolves to nothing, the builtin is painted, and the list arriving a
 *    moment later compares equal and is never applied. The import looks like it did nothing.
 *  - A re-import replacing a scheme's colours under the same id, which is how a user picks up a
 *    corrected theme file.
 *
 * Reference identity is exactly the right test for both: `applySnapshot` rebuilds `boot` around
 * a new `workspace` and keeps the same `schemes` array, so an ordinary tree mutation compares
 * equal — while a bootstrap or a scheme broadcast hands over a fresh array, and therefore fresh
 * objects, every time.
 */
export function schemeIsPainted(painted: PaintedScheme | null, next: PaintedScheme): boolean {
  return (
    painted !== null &&
    painted.theme === next.theme &&
    painted.id === next.id &&
    painted.scheme === next.scheme
  )
}

/** One row of the settings picker. */
export interface SchemeChoice {
  readonly value: string
  readonly label: string
}

/**
 * The schemes offered for a theme: the builtin first, then the imported ones of that polarity.
 *
 * Filtered rather than greyed out. A scheme of the other polarity is not a choice this window
 * can honour — see the module header — and a disabled row invites the question "why?" with no
 * room to answer it. The row's hint says the setting is per theme, which is the answer.
 */
export function schemeChoices(
  schemes: readonly { id: string; name: string; polarity: Polarity }[],
  theme: Polarity,
): SchemeChoice[] {
  return [
    { value: BUILTIN_SCHEME, label: 'cide' },
    ...schemes
      .filter((s) => s.polarity === theme)
      .map((s) => ({ value: s.id, label: s.name })),
  ]
}
