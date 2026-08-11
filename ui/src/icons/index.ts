/**
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
