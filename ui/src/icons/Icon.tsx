/**
 * The app's one icon component: a 24x24 stroke mark that takes its colour from `currentColor`.
 *
 * This is `chrome/ActivityRail.tsx`'s `RailIcon`, promoted. That component was written because a
 * rail of Unicode characters could not be made to look like one set — the measurement is in its
 * header, and it is the whole argument for this file: what a character puts on screen is its ink
 * inside its em box, a ratio belonging to whichever face fontconfig picked, so seven glyphs at
 * seven font sizes produced ink between 0.53em and 0.82em. Every other surface in this app —
 * the git toolbar, the pane title bar, the status bar, six panels, every dialog — was still
 * drawing characters when this landed.
 *
 * # Two provenances, one render path
 *
 * A mark comes either from the vendored set by `name`, or from an extension as a raw `d` that
 * `cide_ext::manifest::is_svg_path` has already validated. Those are genuinely different — only
 * one of them can be name-checked at compile time — so the props are a discriminated union. They
 * collapse to a single `<path>` immediately, which is the point: the code path exercised by the
 * least-trusted input is also the one every built-in icon uses, rather than a second branch that
 * only extension data ever reaches.
 *
 * # Labelling
 *
 * `label` absent means `aria-hidden`, and that is the overwhelming case. Nearly every mark here
 * sits inside a `<button>` that already carries `aria-label` and `title` — the rail, the git
 * toolbar, the pane cluster, the find bar — or beside text that names it. A second accessible
 * name would make a screen reader read every one of them twice, which is the argument
 * `FileIcon`'s `alt=""` already makes for the file tree. `label` is for the handful of places
 * where the icon *is* the only content and no ancestor names it.
 *
 * There is deliberately no `title` prop. A tooltip belongs on the control, where the app already
 * puts it, not on the drawing inside it.
 */
import type { ReactElement } from 'react'

import { ICON_CLASS } from './iconElement'
import { ICON_PATHS, type IconName } from './iconPaths'

/* Re-exported so a component that draws a mark needs one import rather than two. */
export type { IconName }

/* Declared in `iconElement.ts` so that module stays free of `.tsx`; see the note there. */
export type { IconSize } from './iconElement'
import type { IconSize } from './iconElement'

type Common = {
  /** Defaults to `2` — a 16px box, which is the row-and-toolbar size most of the app wants. */
  size?: IconSize
  /**
   * `| undefined` explicitly, because `exactOptionalPropertyTypes` is on and Vite types every
   * `*.module.css` as `Record<string, string>` under `noUncheckedIndexedAccess` — so
   * `styles.foo` is `string | undefined` and a bare `className?: string` cannot be handed one.
   * Every DOM element in the app types it this way, which is why no caller had hit it until a
   * mark needed a class of its own (`chrome/AppHeader.module.css`'s spinning chip).
   */
  className?: string | undefined
  /** Supply only when the icon is the sole content and nothing around it carries the name. */
  label?: string
}

export type IconProps =
  | (Common & { name: IconName; d?: undefined })
  /** An extension's raw 24x24 `d`, already through `is_svg_path` on the Rust side. */
  | (Common & { d: string; name?: undefined })

export function Icon({ size = 2, className, label, ...source }: IconProps): ReactElement {
  const d = source.name === undefined ? source.d : ICON_PATHS[source.name]
  return (
    <svg
      className={className === undefined ? ICON_CLASS : `${ICON_CLASS} ${className}`}
      data-size={size}
      /*
       * The mark's name, in the markup, for the same reason `data-audit` exists: this app has no
       * DOM in its test harness, so half a dozen `check-*.mjs` scripts SSR a component and assert
       * on the string. They used to read the glyph out of the element's text — `>✗<` — and a
       * drawn mark has no text at all, so every one of those assertions would have had to be
       * deleted rather than moved. A name attribute is strictly better than the glyph was: it
       * survives a redraw, it cannot be confused with a neighbouring character, and it says
       * which mark rather than which codepoint.
       *
       * Absent for an extension's raw `d`, which has no name to give.
       */
      data-icon={source.name}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      /* Not a tab stop in any engine, and IE-era `focusable` still matters to some AT. */
      focusable="false"
      {...(label === undefined ? { 'aria-hidden': true } : { role: 'img', 'aria-label': label })}
    >
      <path d={d} />
    </svg>
  )
}

/**
 * Narrow a name that could not be typed at its source.
 *
 * `sidebar/AgentsPanel/model.ts` and `sidebar/TasksPanel/model.ts` are **import-free on purpose**
 * — `check-agents.mjs` compiles them standalone with no `--rootDir` and imports the output
 * directly, so a single `import type` would pull a third file into the program, move tsc's
 * inferred common source directory and break that path. Their glyph tables therefore return
 * `string`, and this is where the string becomes a name.
 *
 * A **runtime** narrowing rather than a cast, and the difference is what you see when it is
 * wrong: a stale table entry draws a visible "no such thing" mark, where `as IconName` would put
 * `d={undefined}` into the DOM and draw nothing at all — invisible in review, and invisible in a
 * screenshot of a state that happens not to be showing. `check-ui-icons.mjs` is what stops the
 * fallback from ever being reached in the first place.
 */
export function asIcon(name: string): IconName {
  return name in ICON_PATHS ? (name as IconName) : 'circle-slash'
}
