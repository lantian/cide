/**
 * The parts of a theme switch that are decisions rather than side effects.
 *
 * There is no JS test runner in this project and the app must never be launched to look at
 * a colour, so everything about theming that can be wrong on its own is pulled in here and
 * pinned by `scripts/check-theme.mjs`, which compiles this file with `tsc` and asserts on
 * it — the same shape as `check-exit-marker.mjs`. The rule for what belongs: values in,
 * values out. Reading `getComputedStyle`, writing `data-theme` and repainting an xterm stay
 * with their callers in `useSettings.ts` and `terminal/xterm.ts`, because none of that
 * survives being lifted out of a browser.
 *
 * Deliberately import-free. The check script compiles this one file on its own, and an
 * import of `@/store/workspace` would drag the alias, zustand and the whole IPC client in
 * behind it — which is how a pure module stops being checkable.
 */

/** Structurally the generated `Theme`; kept local so this file imports nothing. */
export type ThemeName = 'dark' | 'light'

/**
 * What a window shows when nothing has told it otherwise.
 *
 * Light, at the user's explicit instruction: "default theme should be light", and light is
 * to be white-based rather than the mock's warm beige. `Grount IDE.dc.html` is treated as
 * an exact spec everywhere else in this repo, so a future reader will find this
 * disagreeing with the mock's `--bg: #f5f3f0` / `--chrome: #e8e4de` and reach for those
 * values. Don't. The user looked at that palette and asked for white instead; this is an
 * override, not drift to be corrected.
 *
 * The authoritative default for a *stored* workspace is Rust's `Theme::default`. This is the
 * frontend's answer for the window that has not heard from it yet, and it is read by
 * `installThemeSync`, which runs before React and settles what this window starts on:
 * `<html data-theme>` if `theme-boot.js` wrote one, this otherwise. That is deliberately
 * *not* the `theme` seed in `store/workspace.ts` — a file this change does not own, and one
 * whose seed still reads `'dark'`. Reading the seed here would have made the mismatch matter;
 * overwriting it at install makes it inert instead, so the seed is a tidy-up rather than a bug.
 */
export const DEFAULT_THEME: ThemeName = 'light'

/** Where the header's toggle goes from here. */
export function otherTheme(theme: ThemeName): ThemeName {
  return theme === 'dark' ? 'light' : 'dark'
}

/**
 * What this window should switch to, given the theme it is showing, the one the workspace
 * mirror now carries, and the one it carried a moment ago. `null` means "nothing to do".
 *
 * The rule is **follow the mirror when the mirror moves** — not "match the mirror". The
 * difference is the whole correctness of the local half of the switch, and getting it wrong
 * is not theoretical: comparing `showing` against `stored` alone reverts the window that made
 * the change. `applyTheme` sets the local field first and persists after, so between the click
 * and the snapshot coming back this window is showing the new theme while the mirror still
 * holds the old one. A rule that only asks "do they differ?" answers with the OLD theme there
 * and writes it back over the user's click: the switch then waits on an IPC round trip — or
 * never happens, if the write fails. The eager local paint this module exists for would be
 * dead code.
 *
 * So a difference is only actionable when `stored` itself moved. That is exactly "another
 * window (or Rust) changed the theme", which is the reported bug, and it is never true of the
 * window that made the change until its own snapshot returns carrying the value it is already
 * showing — where `stored === showing` stops it anyway.
 *
 * `undefined` for `stored` is the pre-bootstrap window: the mirror has no settings yet, so it
 * has no opinion and must not be allowed to overwrite one. `undefined` for `storedBefore` is
 * the bootstrap itself, where `hydrate` sets `boot` and `theme` in one `set` and the two
 * therefore already agree.
 */
export function themeToAdopt(
  showing: ThemeName,
  stored: ThemeName | undefined,
  storedBefore: ThemeName | undefined,
): ThemeName | null {
  if (stored === undefined || stored === showing) return null
  // The mirror stood still and this window moved: a local change waiting on its round trip.
  if (stored === storedBefore) return null
  return stored
}

/**
 * The theme name a string is, or `null` if it is not one.
 *
 * For reading `<html data-theme>` back: `public/theme-boot.js` writes it from the `?theme=`
 * parameter before the first frame, so on the way up the attribute is a *better* answer than
 * the store's compiled-in seed. Anything else in it — a stale value, a hand-edited DOM — must
 * fall through to the default rather than be handed to `set` as a `Theme`.
 */
export function asThemeName(value: string | null | undefined): ThemeName | null {
  return value === 'dark' || value === 'light' ? value : null
}

/**
 * xterm colour slot ← design token. The whole of what a terminal knows about the theme.
 *
 * A walkable table rather than an object literal built inline, because the failure it
 * guards against is silent. `parseColor` in xterm's `ThemeService` returns its own built-in
 * default for any value it cannot parse, and an undefined custom property reads as `''`,
 * which `css.toColor` throws on. So a token the light palette forgot to define does not
 * error and does not warn — that terminal simply keeps xterm's stock black background while
 * the rest of the window goes white. From the user's chair that is indistinguishable from a
 * theme switch that failed to reach every terminal, which is the bug being fixed here.
 * `check-theme.mjs` walks this table against `styles/tokens.css` for exactly that reason.
 *
 * Slot names are xterm's `ITheme` keys and the order is fixed, because
 * [`paletteSignature`] joins the values in it.
 */
export const TERMINAL_SLOTS: readonly (readonly [slot: string, token: string])[] = [
  ['background', '--panel'],
  ['foreground', '--text'],
  ['cursor', '--accent'],
  ['cursorAccent', '--panel'],
  ['selectionBackground', '--sel'],
  ['black', '--term-black'],
  ['red', '--red'],
  ['green', '--green'],
  ['yellow', '--yellow'],
  ['blue', '--blue'],
  ['magenta', '--purple'],
  ['cyan', '--cyan'],
  ['white', '--text'],
  ['brightBlack', '--faint'],
  ['brightRed', '--red'],
  ['brightGreen', '--green'],
  ['brightYellow', '--yellow'],
  ['brightBlue', '--blue'],
  ['brightMagenta', '--purple'],
  ['brightCyan', '--cyan'],
  ['brightWhite', '--text-hi'],
]

/** Every design token a terminal reads, deduplicated. What `tokens.css` must define. */
export const TERMINAL_TOKENS: readonly string[] = [
  ...new Set(TERMINAL_SLOTS.map(([, token]) => token)),
]

/**
 * Resolve the table with a caller-supplied reader — `getComputedStyle` in the app, a plain
 * lookup in the check.
 *
 * Empty values are kept rather than dropped. Dropping them would be identical in effect
 * (xterm substitutes its default either way) while hiding the problem from
 * [`unresolvedSlots`], which is the only place a missing token becomes visible without a
 * screen to look at.
 */
export function terminalPalette(read: (token: string) => string): Record<string, string> {
  const palette: Record<string, string> = {}
  for (const [slot, token] of TERMINAL_SLOTS) palette[slot] = read(token)
  return palette
}

/** Slots whose token resolved to nothing — each one a terminal colour xterm will invent. */
export function unresolvedSlots(palette: Record<string, string>): string[] {
  return TERMINAL_SLOTS.filter(([slot]) => (palette[slot] ?? '').length === 0).map(
    ([slot]) => slot,
  )
}

/**
 * A value that changes exactly when a terminal's colours do.
 *
 * `retheme` runs once per theme change per window and assigning `term.options.theme` costs
 * a full re-render plus a texture-atlas re-acquire per terminal, so the pass is skipped for
 * terminals already wearing these colours. Comparing against the *resolved* values rather
 * than against the theme name is what makes that safe: a terminal built while `data-theme`
 * was still unset holds whatever the bare `:root` painted, and a name comparison would call
 * that "already light" and leave it alone.
 *
 * Joined in table order rather than `JSON.stringify`d so key insertion order cannot make
 * two equal palettes look different. The separator is written as an escape rather than as a
 * literal NUL byte: a literal one makes git call this file binary, and a palette table whose
 * diff nobody can read is how a wrong colour ships.
 */
export function paletteSignature(palette: Record<string, string>): string {
  return TERMINAL_SLOTS.map(([slot]) => palette[slot] ?? '').join('\u0000')
}
