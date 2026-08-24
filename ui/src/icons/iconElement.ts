/**
 * The same mark, built imperatively, for the places that have no React.
 *
 * `panes/mergeGutter.ts` builds its accept/revert buttons with `document.createElement` inside a
 * CodeMirror `GutterMarker.toDOM()`, and set `button.textContent` to a glyph. There is no
 * component tree there to render an `<Icon>` into, so the alternative to this file is that the
 * merge gutter stays on characters while everything around it does not — which is exactly the
 * half-glyphs-half-paths state `chrome/ActivityRail.tsx` argued against.
 *
 * The envelope is shared with `Icon.tsx` through {@link ICON_SVG_ATTRS} rather than written
 * twice, because two builders that drift are two icon sets.
 */
import { ICON_PATHS, type IconName } from './iconPaths'

const SVG_NS = 'http://www.w3.org/2000/svg'

/**
 * A rung of `--icon-0` … `--icon-3`. The box and its stroke are in `Icon.module.css`.
 *
 * Declared here rather than in `Icon.tsx`, which is where it is used most, because this module
 * has to stay importable by code that is compiled **standalone**: `check-editor.mjs` runs plain
 * `tsc` over `editor/folding.ts` with no `--jsx` and no path aliases, and a type-only import of
 * a `.tsx` is still a `.tsx` the compiler must parse. `Icon.tsx` re-exports it, so the name is
 * reachable from either side.
 */
export type IconSize = 0 | 1 | 2 | 3

/**
 * Everything that makes a mark look like this app's marks, minus the box and the stroke width —
 * those come from `Icon.module.css` via `data-size`, so there is still exactly one place that
 * decides how big an icon is.
 */
export const ICON_SVG_ATTRS: Readonly<Record<string, string>> = {
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  'stroke-linecap': 'round',
  'stroke-linejoin': 'round',
  focusable: 'false',
  'aria-hidden': 'true',
}

/**
 * An `<svg>` element carrying one vendored mark.
 *
 * `className` is taken rather than assumed: a CSS module's class names are hashed, so the caller
 * is the only one that can name the rule it wants applied.
 */
export function iconElement(name: IconName, size: IconSize = 2, className?: string): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, 'svg')
  for (const [key, value] of Object.entries(ICON_SVG_ATTRS)) svg.setAttribute(key, value)
  svg.setAttribute('data-size', String(size))
  if (className !== undefined) svg.setAttribute('class', className)
  const path = document.createElementNS(SVG_NS, 'path')
  path.setAttribute('d', ICON_PATHS[name])
  svg.append(path)
  return svg
}
