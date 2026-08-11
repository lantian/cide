/**
 * The hook a surface uses. One import, two lines at the call site:
 *
 * ```tsx
 * const { onContextMenu, menu } = useContextMenu({
 *   label: 'File tree',
 *   items: ({ target }) => [
 *     { id: 'open', label: 'Open', command: 'file.open', run: () => open(pathOf(target)) },
 *     { kind: 'separator' },
 *     { id: 'delete', label: 'Delete', danger: true, disabledReason: 'Read-only project' },
 *   ],
 * })
 * return <div onContextMenu={onContextMenu}>{rows}{menu}</div>
 * ```
 *
 * `menu` is rendered wherever it is convenient — it portals to the app root, so where it
 * appears in the caller's tree has no effect on where it appears on screen, and an
 * `overflow: hidden` pane cannot clip it.
 *
 * `items` is a function, not an array, and is called at open time. That is what lets a tree
 * build its menu from the row that was actually clicked, and it is also why a menu never
 * shows stale enablement: nothing is computed until the gesture happens.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ContextMenu } from './ContextMenu'
import { menuLifetime } from './menuState'
import {
  anchorToRect,
  isEmptyMenu,
  resolveMenu,
  type MenuEntry,
  type ResolvedEntry,
} from './model'
import { buildKeymap, type Keymap } from '@/keys/keymap'
import { useWorkspace } from '@/store/workspace'

/** What `items` is told about the gesture that opened the menu. */
export interface MenuInvocation {
  /** The element under the pointer — the row, the tab, the tree node. */
  readonly target: HTMLElement | null
  /** Window coordinates. */
  readonly x: number
  readonly y: number
}

export interface UseContextMenuOptions {
  /** Accessible name for the menu, e.g. `File tree`. Not shown. */
  readonly label: string
  /** Built at open time. Return `[]` (or only separators) to decline to open at all. */
  readonly items: (at: MenuInvocation) => readonly MenuEntry[]
  /**
   * Where shortcut hints come from. Defaults to the window's own resolved keymap — the same
   * bindings the key gate dispatches from — so a rebind moves the hint with it. Pass one only
   * from a fixture, or from a surface that deliberately shows a different map.
   */
  readonly keymap?: Pick<Keymap, 'chipFor'> | null | undefined
}

export interface ContextMenuHandle {
  /** Put this on the surface's root element. It suppresses and stops the event itself. */
  readonly onContextMenu: (ev: React.MouseEvent) => void
  /** For a Shift+F10 handler or a `⋯` button. `target` anchors a keyboard invocation. */
  readonly openAt: (x: number, y: number, target?: HTMLElement | null) => void
  /** Same, but positioned under an element rather than at a point. */
  readonly openFor: (target: HTMLElement) => void
  readonly close: () => void
  readonly isOpen: boolean
  /** Render this. It portals; its position in the caller's tree does not matter. */
  readonly menu: React.ReactNode
}

/** Not `[]`: a fresh literal each render would re-run `buildKeymap` every time. */
const NO_BINDINGS: never[] = []

/**
 * The window's keymap, built once per bootstrap.
 *
 * Read from the workspace store rather than taken as a prop. Menus are wired deep inside
 * trees, tab strips and panes, and threading a `keymap` prop down to each of them would be
 * five call sites in `App.tsx` that all have to be remembered — the shape that has already
 * left three controls in this app wired to nothing. The store holds exactly the array
 * `App.tsx` passes to `useKeyGate`.
 */
function useWindowKeymap(): Keymap {
  const bindings = useWorkspace((s) => s.boot?.keymap ?? NO_BINDINGS)
  return useMemo(() => buildKeymap(bindings), [bindings])
}

interface OpenMenu {
  readonly entries: readonly ResolvedEntry[]
  readonly at: { x: number; y: number }
}

export function useContextMenu(options: UseContextMenuOptions): ContextMenuHandle {
  const { label, items, keymap } = options
  const windowKeymap = useWindowKeymap()
  const chipFor = (keymap ?? windowKeymap).chipFor

  const [open, setOpen] = useState<OpenMenu | null>(null)
  /** Where focus was when the menu opened, so it can be handed back. */
  const returnTo = useRef<HTMLElement | null>(null)

  const close = useCallback(() => {
    /*
     * Focus goes back where it came from, and only if that element is still in the document
     * — a menu whose action deleted the row it hung off would otherwise focus a detached
     * node and leave the window with no focus at all. When it is gone, doing nothing is
     * right: focus falls to `<body>` and the next Tab starts from the top rather than from
     * nowhere.
     *
     * Outside the `setOpen` updater on purpose. An updater is called twice under StrictMode
     * and may be replayed by React at will, so it is the wrong place for a DOM side effect;
     * `close` is idempotent instead — the ref is cleared on the first call, so the extra
     * calls that a pointerdown-then-blur pair produces move nothing.
     */
    const previous = returnTo.current
    returnTo.current = null
    setOpen(null)
    if (previous !== null && previous.isConnected) previous.focus()
  }, [])

  const openAt = useCallback(
    (x: number, y: number, target?: HTMLElement | null) => {
      const entries = resolveMenu(items({ target: target ?? null, x, y }), { chipFor })
      // An empty box at the pointer is worse than no menu: it says the surface is broken
      // rather than that it has nothing to offer. All-disabled still opens — see
      // `isInertMenu` — because a reason is an answer.
      if (isEmptyMenu(entries)) return
      returnTo.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null
      setOpen({ entries, at: { x, y } })
    },
    [items, chipFor],
  )

  const openFor = useCallback(
    (target: HTMLElement) => {
      const point = anchorToRect(target.getBoundingClientRect())
      openAt(point.x, point.y, target)
    },
    [openAt],
  )

  const onContextMenu = useCallback(
    (ev: React.MouseEvent) => {
      // `preventDefault` here is belt to `installNativeMenuSuppression`'s braces: the global
      // capture listener has already prevented it, and this keeps the surface correct even if
      // someone mounts it in a harness where the global was never installed.
      ev.preventDefault()
      // A tree row inside a tree panel inside a pane: each may have its own menu, and the
      // innermost is the one the user aimed at.
      ev.stopPropagation()
      const target = ev.target instanceof HTMLElement ? ev.target : null
      /*
       * Keyboard invocation (Shift+F10, the Menu key) arrives as a `contextmenu` event too,
       * and WebKit reports 0,0 for it. `detail`/`button` are not reliable discriminators
       * across engines; the exact origin is, and a genuine right-click on the single pixel at
       * 0,0 anchoring to the element instead is a harmless miss.
       */
      if (ev.clientX === 0 && ev.clientY === 0 && ev.currentTarget instanceof HTMLElement) {
        openFor(target ?? ev.currentTarget)
        return
      }
      openAt(ev.clientX, ev.clientY, target)
    },
    [openAt, openFor],
  )

  // Publishes the flag `App.tsx` can feed the key gate. Mount/unmount of the *menu*, not of
  // the hook, so a surface that never opens one contributes nothing.
  useEffect(() => {
    if (open === null) return
    menuLifetime.opened()
    return () => menuLifetime.closed()
  }, [open])

  return {
    onContextMenu,
    openAt,
    openFor,
    close,
    isOpen: open !== null,
    menu:
      open === null ? null : (
        <ContextMenu label={label} entries={open.entries} at={open.at} onClose={close} />
      ),
  }
}
