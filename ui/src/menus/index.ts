/**
 * The context-menu system. Import from here, not from the files inside.
 *
 * Importing this module **installs the global suppression of the webview's own menu** as a
 * side effect. That is deliberate: the requirement is that "Inspect element" never appears,
 * and a feature that depends on someone remembering to call an installer is a feature that is
 * off until it is reported as a bug. Any surface that uses a menu at all pulls this in, and
 * the first one to do so covers the whole window. `installNativeMenuSuppression` is still
 * exported so `App.tsx` can call it explicitly — see the note there — which is what makes it
 * true even in a window where no surface has wired a menu yet, and what lets a caller flip
 * `nativeInTextInputs`.
 *
 * `installNativeMenuSuppression` is idempotent, so the explicit call and this one do not
 * stack, and it is a no-op with no DOM, so an SSR check script importing this is harmless.
 */
import { installNativeMenuSuppression } from './native'

installNativeMenuSuppression()

export { installNativeMenuSuppression, type SuppressionOptions } from './native'
export { useContextMenu } from './useContextMenu'
export type {
  ContextMenuHandle,
  MenuInvocation,
  UseContextMenuOptions,
} from './useContextMenu'
export { contextMenuOpen, useContextMenuOpen } from './menuState'
export { ContextMenu, type ContextMenuProps } from './ContextMenu'
export {
  MENU_MARGIN,
  NATIVE_MENU_ATTR,
  NO_ACTION_REASON,
  activeItem,
  anchorToRect,
  focusReturnPlan,
  isEmptyMenu,
  isInertMenu,
  moveFocus,
  placeMenu,
  placeSubmenu,
  resolveMenu,
  wantsNativeMenu,
} from './model'
export type {
  ElementFacts,
  FocusReturnStep,
  MenuEntry,
  MenuItem,
  MenuMotion,
  MenuSeparator,
  Placement,
  Point,
  Rect,
  ResolveOptions,
  ResolvedEntry,
  ResolvedItem,
  ResolvedSeparator,
  Size,
} from './model'
