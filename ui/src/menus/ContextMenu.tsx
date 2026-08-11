/**
 * The box itself: measure, place, paint, and hand focus back when it goes.
 *
 * Everything it *decides* lives in `model.ts` and is checked by `ui/scripts/check-menus.mjs`.
 * What is left here is the part that genuinely needs a DOM — reading the rendered size,
 * portalling, wiring listeners — and it is kept deliberately thin so that the untested half
 * is the half that cannot be tested.
 *
 * Callers do not normally render this. `useContextMenu` does, and hands back the node.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import styles from './ContextMenu.module.css'
import {
  activeItem,
  moveFocus,
  placeMenu,
  type Placement,
  type ResolvedEntry,
} from './model'

export interface ContextMenuProps {
  /** Accessible name for the menu, e.g. `File tree`. */
  label: string
  entries: readonly ResolvedEntry[]
  /** Window coordinates of the pointer, or of the anchor for a keyboard invocation. */
  at: { x: number; y: number }
  onClose: () => void
}

/**
 * The portal target.
 *
 * A sibling of `#root` rather than a node inside it. This app is a grid of panes and almost
 * every one of them is an `overflow: hidden` box — a menu rendered in place would be clipped
 * by the pane it belongs to, which is the specific failure the portal exists to avoid.
 * `document.body` is safe for `position: fixed` here because nothing between it and the menu
 * establishes a containing block (no `transform`, `filter` or `contain` on `html`/`body`);
 * `#root` is not, because a pane ancestor may.
 *
 * Created lazily and left in place. It holds nothing when no menu is open, and creating it in
 * `index.html` would make the portal depend on a file this feature does not own.
 */
const ROOT_ID = 'cide-menu-root'

function menuRoot(): HTMLElement {
  const existing = document.getElementById(ROOT_ID)
  if (existing !== null) return existing
  const created = document.createElement('div')
  created.id = ROOT_ID
  document.body.appendChild(created)
  return created
}

export function ContextMenu({ label, entries, at, onClose }: ContextMenuProps): React.ReactNode {
  const box = useRef<HTMLDivElement>(null)
  const items = useRef(new Map<number, HTMLButtonElement>())
  const [placement, setPlacement] = useState<Placement | null>(null)
  const [active, setActive] = useState<number | null>(null)

  /*
   * Measure then place, in a layout effect so the browser never paints the intermediate.
   *
   * The natural height is what is measured — `maxHeight` is applied only in the committed
   * style — because a box that is already capped measures as capped and would report that it
   * fits wherever it was put.
   */
  useLayoutEffect(() => {
    const el = box.current
    if (el === null) return
    const rect = el.getBoundingClientRect()
    setPlacement(
      placeMenu(
        at,
        { width: rect.width, height: rect.height },
        { width: window.innerWidth, height: window.innerHeight },
      ),
    )
  }, [at, entries])

  // The menu takes focus so the arrows are its own. Item 0 is *not* pre-selected: a menu that
  // opens with a destructive line highlighted is one stray Enter from doing it.
  useLayoutEffect(() => {
    box.current?.focus()
  }, [])

  useEffect(() => {
    if (active === null) return
    items.current.get(active)?.focus()
  }, [active])

  const run = useCallback(
    (index: number) => {
      const item = activeItem(entries, index)
      if (item === null) return
      // Close first. The handler may open a dialog or move focus, and a menu still mounted
      // over it would then eat the next outside click to dismiss itself instead.
      onClose()
      item.run?.()
    },
    [entries, onClose],
  )

  /*
   * Dismissal, in one effect because the four triggers are the same answer to the same
   * question — "is the thing this menu was about still under it?".
   *
   * `pointerdown` in capture, not `click`: a click that lands on a button behind the menu
   * would otherwise both dismiss the menu and press the button. Scroll and resize are
   * included because the placement is computed once from window coordinates and is stale the
   * instant anything moves; re-placing on scroll would leave the menu glued to a pane that
   * has scrolled out from under the row it belongs to. Blur covers alt-tab and the app's own
   * detached windows.
   */
  useEffect(() => {
    const onPointerDown = (ev: PointerEvent) => {
      const el = box.current
      if (el !== null && ev.target instanceof Node && el.contains(ev.target)) return
      onClose()
    }
    // `true`: a pane that stops propagation on its own pointerdown must not be able to trap
    // a menu open, and scroll does not bubble from an inner container at all.
    window.addEventListener('pointerdown', onPointerDown, true)
    window.addEventListener('scroll', onClose, true)
    window.addEventListener('resize', onClose)
    window.addEventListener('blur', onClose)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown, true)
      window.removeEventListener('scroll', onClose, true)
      window.removeEventListener('resize', onClose)
      window.removeEventListener('blur', onClose)
    }
  }, [onClose])

  const onKeyDown = (ev: React.KeyboardEvent<HTMLDivElement>) => {
    const move = (motion: 'next' | 'prev' | 'first' | 'last') => {
      ev.preventDefault()
      ev.stopPropagation()
      setActive((current) => moveFocus(entries, current, motion))
    }
    switch (ev.key) {
      case 'Escape':
        ev.preventDefault()
        // Stopped, so the same Escape does not also close the pane behind the menu.
        ev.stopPropagation()
        return onClose()
      case 'ArrowDown':
        return move('next')
      case 'ArrowUp':
        return move('prev')
      case 'Home':
        return move('first')
      case 'End':
        return move('last')
      // A menu is a modal loop: Tab moves within it rather than out of it and into whatever
      // is behind, which the user cannot see.
      case 'Tab':
        return move(ev.shiftKey ? 'prev' : 'next')
      case 'Enter':
      case ' ':
        ev.preventDefault()
        ev.stopPropagation()
        if (active !== null) run(active)
        return
      // Left/right are submenu keys elsewhere; there are no submenus here, and closing on
      // Left is what every menu that lacks them does.
      case 'ArrowLeft':
        ev.preventDefault()
        ev.stopPropagation()
        return onClose()
      default:
        return
    }
  }

  const hasToggle = entries.some((e) => e.kind === 'item' && e.checked !== null)

  return createPortal(
    <div
      ref={box}
      className={placement === null ? `${styles.menu} ${styles.measuring}` : styles.menu}
      style={
        placement === null
          ? undefined
          : { left: placement.x, top: placement.y, maxHeight: placement.maxHeight }
      }
      role="menu"
      aria-label={label}
      aria-orientation="vertical"
      tabIndex={-1}
      data-audit="contextMenu"
      data-flipped-x={placement?.flippedX === true ? 'true' : 'false'}
      data-flipped-y={placement?.flippedY === true ? 'true' : 'false'}
      onKeyDown={onKeyDown}
      // The menu is our menu; a right-click *on* it must not summon a second one, and must
      // not reach the surface underneath either.
      onContextMenu={(ev) => {
        ev.preventDefault()
        ev.stopPropagation()
      }}
    >
      {entries.map((entry, index) =>
        entry.kind === 'separator' ? (
          <div key={entry.id} className={styles.separator} role="separator" />
        ) : (
          <button
            key={entry.id}
            type="button"
            ref={(el) => {
              if (el === null) items.current.delete(index)
              else items.current.set(index, el)
            }}
            className={[
              styles.item,
              entry.enabled ? '' : styles.disabled,
              entry.danger ? styles.danger : '',
              active === index ? styles.active : '',
            ]
              .filter((c) => c !== '')
              .join(' ')}
            role={entry.checked === null ? 'menuitem' : 'menuitemcheckbox'}
            aria-checked={entry.checked === null ? undefined : entry.checked}
            tabIndex={-1}
            // Not the `disabled` attribute: a disabled <button> takes no pointer events, so
            // its `title` never appears and the reason becomes unreadable on hover.
            aria-disabled={entry.enabled ? undefined : true}
            data-item={entry.id}
            data-enabled={entry.enabled ? 'true' : 'false'}
            title={entry.reason ?? undefined}
            onClick={() => run(index)}
            // Hover moves the keyboard cursor too, so arrowing after reaching for the mouse
            // continues from where the eye is rather than from where the keyboard was.
            onPointerEnter={() => entry.enabled && setActive(index)}
          >
            {hasToggle && (
              <span className={styles.check} aria-hidden="true">
                {entry.checked === true ? '✓' : ''}
              </span>
            )}
            <span className={styles.label}>{entry.label}</span>
            {entry.reason !== null && <span className={styles.reason}>{entry.reason}</span>}
            {entry.hint !== null && <span className={styles.hint}>{entry.hint}</span>}
          </button>
        ),
      )}
    </div>,
    menuRoot(),
  )
}
