/**
 * The two icon systems, and the line between them.
 *
 * **`FileIcon` answers "what kind of thing is this".** Multi-colour, fill-based, from the
 * Material Icon Theme, drawn as an `<img>`, and it never changes colour with state. It is the
 * leading mark of a row: the file tree, the search panel, the git changes tree, the git log.
 *
 * **`Icon` answers "what will happen if I press this".** Monochrome, stroke, `currentColor`, so
 * it follows `--dim` / `--text` / `--accent` as a control lights up. It is every action and
 * every status mark in the app.
 *
 * They coexist without reading as two systems because they never occupy the same column and
 * they share one 16px grid — and because the difference in weight *is* the signal: a row's
 * identity stays put while its controls respond. That is also why a monochrome tint over the
 * Material set was rejected in `FileIcon.tsx`'s header — a single tint is a different icon set.
 *
 * File-tree icons, from the Material Icon Theme.
 *
 * Vendored from material-extensions/vscode-material-icon-theme under the MIT licence; the
 * licence text ships beside the icons at `ui/public/icons/LICENSE`. `ui/scripts/vendor-icons.mjs`
 * is what produced both the SVGs and `iconMap.ts`, and re-running it is how the set is updated.
 *
 * Typical use, from a row renderer:
 *
 *     const theme = useIconTheme()          // once, at the panel
 *     <FileIcon row={row} theme={theme} />  // per row; `TreeRow` fits `IconRow` as-is
 */
export { FileIcon, iconUrl, type FileIconProps } from './FileIcon'
export {
  iconFor,
  DEFAULT_FILE_ICON,
  DEFAULT_FOLDER_ICON,
  type IconRow,
  type IconTheme,
} from './iconFor'
export { useIconTheme } from './useIconTheme'
/*
 * `Icon` is re-exported here for symmetry, but **leaf components should import it from
 * `@/icons/Icon` directly**, and that is not a style preference.
 *
 * This barrel also exports `useIconTheme`, which imports `@/store/workspace` — the whole Zustand
 * mirror, and through it the IPC client and the terminal modules. Half a dozen `check-*.mjs`
 * scripts SSR-bundle a single component and run it under node, where `@xterm/addon-clipboard`
 * touches `self` on import and throws outright. So a component that wants nothing but a drawn
 * mark and reaches for it through this file drags the entire app into a harness that has no
 * browser, and the failure it gets is `ReferenceError: self is not defined` from a file it has
 * never heard of.
 */
export { Icon, asIcon, type IconProps, type IconSize } from './Icon'
export { iconElement, ICON_SVG_ATTRS } from './iconElement'
export { ICON_PATHS, type IconName } from './iconPaths'
