/**
 * The box itself: measure, place, paint, and hand focus back when it goes.
 *
 * Everything it *decides* lives in `model.ts` and is checked by `ui/scripts/check-menus.mjs`.
 * What is left here is the part that genuinely needs a DOM — reading the rendered size,
 * portalling, wiring listeners — and it is kept deliberately thin so that the untested half
 * is the half that cannot be tested.
 *
 * Callers do not normally render this. `useContextMenu` does, and hands back the node.
 *
 * # Submenus, and why there is exactly one level of them
 *
 * A row carrying `MenuItem.submenu` opens a second box beside itself. That box is rendered by
 * *this* component, into the same portal, and a `submenu` on one of its own rows is ignored.
 *
 * The alternative — `ContextMenu` rendering a nested `ContextMenu` — reads better and is
 * wrong here for a concrete reason: the dismissal listeners are `window`-level and ask "is the
 * pointer inside my box". A nested instance would answer that question for its own box only,
 * so a click in the submenu would land outside the parent's, close the parent, and unmount the
 * submenu underneath the click. Owning both boxes is what lets one `inside()` cover both — see
 * the effect below, which is the whole reason this file grew rather than gained a sibling.
 *
 * One level is a decision, not a limit reached by accident. Nothing in this app has wanted two,
 * and each further level is another placement pass, another focus chain and another dismissal
 * rule that no check script in this repo can drive, because there is no DOM in the harness.
 *
 * ## What the submenu deliberately does not have: a safe triangle
 *
 * Moving the pointer onto another row closes the open submenu immediately. Desktop toolkits
 * soften that with a "safe triangle" — a few hundred milliseconds during which a pointer
 * heading towards the submenu may cross other rows without dismissing it — and this has none.
 *
 * That is a real cost and it is bounded: the submenu opens flush against the parent menu's
 * right edge and level with its row, so the direct path is horizontal and crosses nothing. A
 * diagonal path across the row below does close it, and the fix is to move sideways first.
 * The alternative was a timer, a hover intent and a geometry test in a component whose whole
 * design principle is that anything with a rule in it lives in `model.ts` where a check can run
 * it — and none of that could be checked here, because there is no pointer in the harness
 * either. If it becomes annoying in use, the triangle test is a pure function of two rects and
 * a point and belongs beside `placeSubmenu`.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Icon } from '@/icons/Icon'

import styles from './ContextMenu.module.css'
import {
  activeItem,
  moveFocus,
  placeMenu,
  placeSubmenu,
  resolveMenu,
  type MenuEntry,
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
  /**
   * The keychip source, for rows built *after* the menu opened — that is, submenu rows.
   *
   * `useContextMenu` resolves the top level itself and hands the result down already
   * resolved; a submenu's entries do not exist until it is hovered, so this component has to
   * resolve those, and a resolution without `chipFor` silently drops every shortcut hint in
   * the submenu. Optional because a fixture with no keymap should look like one.
   */
  chipFor?: ((command: string) => string | null) | undefined
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

/** An open submenu: whose row it belongs to, what is in it, and where that row is. */
interface OpenSubmenu {
  /** The parent row's index into `entries`. Identity for the placement, and the focus return. */
  readonly index: number
  readonly entries: readonly ResolvedEntry[]
  /** The parent row's box at the moment it opened, in window coordinates. */
  readonly anchor: { left: number; right: number; top: number; bottom: number }
}

export function ContextMenu({
  label,
  entries,
  at,
  onClose,
  chipFor,
}: ContextMenuProps): React.ReactNode {
  const box = useRef<HTMLDivElement>(null)
  const items = useRef(new Map<number, HTMLButtonElement>())
  const [placement, setPlacement] = useState<Placement | null>(null)
  const [active, setActive] = useState<number | null>(null)

  const subBox = useRef<HTMLDivElement>(null)
  const subItems = useRef(new Map<number, HTMLButtonElement>())
  const [submenu, setSubmenu] = useState<OpenSubmenu | null>(null)
  /*
   * The submenu's placement, tagged with the row it was computed for.
   *
   * A bare `Placement | null` would be applied to the *next* submenu for the one frame between
   * that submenu rendering and the layout effect re-measuring it — the box would paint beside
   * the previous row and jump. Tagging it means a placement that does not belong to the open
   * submenu reads as "not measured yet", which is already the state that hides the box.
   */
  const [subPlacement, setSubPlacement] = useState<{ index: number; at: Placement } | null>(null)
  /** The focused submenu row, or `null` when the submenu was opened by hover and not entered. */
  const [subActive, setSubActive] = useState<number | null>(null)

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

  /*
   * The same, for the submenu, against its parent *row* rather than the pointer.
   *
   * `placeSubmenu` and not `placeMenu`: a submenu that runs out of room flips round its row to
   * `left - width`, where a pointer menu flips through the pointer to `x - width`. The second
   * sum would lay the submenu over the parent menu and hide the row the user is hovering.
   */
  useLayoutEffect(() => {
    if (submenu === null) return
    const el = subBox.current
    if (el === null) return
    const rect = el.getBoundingClientRect()
    setSubPlacement({
      index: submenu.index,
      at: placeSubmenu(
        submenu.anchor,
        { width: rect.width, height: rect.height },
        { width: window.innerWidth, height: window.innerHeight },
      ),
    })
  }, [submenu])

  /*
   * The menu takes focus so the arrows are its own. Item 0 is *not* pre-selected: a menu that
   * opens with a destructive line highlighted is one stray Enter from doing it.
   *
   * # Why this is keyed on the placement and was dead before
   *
   * It used to be `useLayoutEffect(() => { box.current?.focus() }, [])`, which is a **no-op**,
   * and the whole keyboard model went with it. Both layout effects run in the same commit, so
   * this one fired before React had flushed the `setPlacement` above — the box still carried
   * `styles.measuring`, which is `visibility: hidden`, and WebKit refuses focus on it
   * (`Element::focus` → `isProgramaticallyFocusable` → `hasFocusableStyle`, which requires
   * `Visibility::Visible`). Measured against this app's own React and CSS, not reasoned about:
   * `activeElement` stayed `BODY`, and the only thing that ever moved focus into the menu was
   * hovering an item. So `onKeyDown` below — Escape, the arrows, Home/End, Tab, Enter — was
   * reachable from nothing until the pointer touched a line, and a menu opened from the
   * keyboard (Shift+F10, the Menu key) could not be driven at all.
   *
   * `placement === null` as the dependency, not `placement`: re-measuring on a moved anchor
   * must not yank focus back off an item the user has already arrowed to.
   *
   * `preventScroll` for the same reason `useContextMenu`'s `close` passes it — this focus
   * change is what *causes* WebKit to clear the document selection, and a portalled box at the
   * far corner of the window is exactly the element the browser would otherwise scroll an
   * ancestor to bring into view.
   */
  useLayoutEffect(() => {
    if (placement === null) return
    box.current?.focus({ preventScroll: true })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [placement === null])

  useEffect(() => {
    // While the keyboard is inside the submenu the parent's highlighted row must keep its
    // ring but must **not** take focus back, or every ArrowDown in the submenu would be read
    // by the parent list instead.
    if (active === null || subActive !== null) return
    // `preventScroll`, and the menu is the one surface where that is not merely hygiene: the
    // box is `overflow-y: auto` under a computed `maxHeight`, so arrowing to an item below the
    // fold scrolls it — and the `scroll` capture listener below would then dismiss the menu
    // under the user's own ArrowDown. The scroller is moved deliberately instead, by
    // `scrollIntoView` with the block alignment a menu wants.
    const item = items.current.get(active)
    if (item === undefined) return
    item.focus({ preventScroll: true })
    item.scrollIntoView({ block: 'nearest' })
  }, [active, subActive])

  useEffect(() => {
    if (subActive === null) return
    const item = subItems.current.get(subActive)
    if (item === undefined) return
    item.focus({ preventScroll: true })
    item.scrollIntoView({ block: 'nearest' })
  }, [subActive])

  /** Close the submenu and put the keyboard back on the row it came out of. */
  const closeSubmenu = useCallback(() => {
    setSubmenu(null)
    setSubActive(null)
  }, [])

  /**
   * Open the list hanging off row `index`, building it now.
   *
   * Built at open time rather than at the parent's — see `MenuItem.submenu`. The one caller
   * lists live Claude sessions by the names their user gave them, and those names arrive from
   * Rust asynchronously; a list snapshotted when the parent menu opened would be one round
   * trip stale, which for a menu of conversations to type into is the wrong conversation.
   *
   * An empty result opens nothing. A submenu box with no rows in it says the feature is broken
   * rather than that it has nothing to offer, which is the same argument `useContextMenu`'s
   * `isEmptyMenu` guard makes about the top level.
   */
  const openSubmenu = useCallback(
    (index: number, row: HTMLElement, build: () => readonly MenuEntry[], enter = false) => {
      const resolved = resolveMenu(build(), chipFor === undefined ? {} : { chipFor })
      if (!resolved.some((e) => e.kind === 'item')) {
        closeSubmenu()
        return
      }
      const rect = row.getBoundingClientRect()
      setSubmenu({
        index,
        entries: resolved,
        anchor: { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom },
      })
      /*
       * `enter` is the difference between the mouse route and the keyboard one, and it is not
       * cosmetic. A hover must leave the keyboard on the parent row: the pointer opened the
       * box, and stealing focus into it would mean the user's next ArrowDown walks a list they
       * were not looking at. A key that opens the box is a statement that the keyboard is
       * going there, so it lands on the first row and the next ArrowDown continues inside.
       *
       * Computed from `resolved` rather than from `submenu.entries`, because the state this
       * call sets is not readable until the next render — and a `setSubActive` that read it
       * would land on whatever the *previous* submenu had.
       */
      setSubActive(enter ? moveFocus(resolved, null, 'first') : null)
    },
    [chipFor, closeSubmenu],
  )

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

  const runSub = useCallback(
    (index: number) => {
      if (submenu === null) return
      const item = activeItem(submenu.entries, index)
      if (item === null) return
      // The whole menu goes, not just the submenu: the user has chosen, and leaving the parent
      // standing over whatever the choice opened is the same trap `run` closes first for.
      onClose()
      item.run?.()
    },
    [submenu, onClose],
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
    const inside = (target: EventTarget | null): boolean => {
      if (!(target instanceof Node)) return false
      // Both boxes, because the submenu is a portal *sibling* of the main one rather than a
      // descendant of it. Asking only the main box would make every click on a submenu row an
      // outside click: the menu would close before the row's own handler ran.
      return box.current?.contains(target) === true || subBox.current?.contains(target) === true
    }
    const onPointerDown = (ev: PointerEvent) => {
      if (inside(ev.target)) return
      onClose()
    }
    /*
     * The menu's *own* scrolling is not "the thing this menu was about has moved".
     *
     * `.menu` is `overflow-y: auto` under a computed `maxHeight`, so a long menu — the git
     * changes one is the long one — scrolls internally, and a `scroll` listener in capture on
     * `window` sees that too (scroll does not bubble, which is why the listener is in capture
     * in the first place). Without this, arrowing or wheeling inside a clipped menu dismisses
     * it: the box scrolls, the listener fires, and the menu vanishes under the pointer.
     */
    const onScroll = (ev: Event) => {
      if (inside(ev.target)) return
      onClose()
    }
    // `true`: a pane that stops propagation on its own pointerdown must not be able to trap
    // a menu open, and scroll does not bubble from an inner container at all.
    window.addEventListener('pointerdown', onPointerDown, true)
    window.addEventListener('scroll', onScroll, true)
    window.addEventListener('resize', onClose)
    window.addEventListener('blur', onClose)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown, true)
      window.removeEventListener('scroll', onScroll, true)
      window.removeEventListener('resize', onClose)
      window.removeEventListener('blur', onClose)
    }
  }, [onClose])

  /**
   * Open row `index`'s submenu and put the keyboard in it, or report that it has none.
   *
   * Shared by Enter and ArrowRight so the two cannot drift: both mean "go into this list",
   * and the `false` is what lets Enter fall through to running an ordinary row while
   * ArrowRight leaves the event alone for anything else that wants it.
   */
  const enterSubmenu = (index: number): boolean => {
    const entry = activeItem(entries, index)
    const row = items.current.get(index)
    if (entry === null || entry.submenu === null || row === undefined) return false
    openSubmenu(index, row, entry.submenu, true)
    return true
  }

  const onKeyDown = (ev: React.KeyboardEvent<HTMLDivElement>) => {
    const move = (motion: 'next' | 'prev' | 'first' | 'last') => {
      ev.preventDefault()
      ev.stopPropagation()
      // Arrowing off the parent row abandons the submenu it opened; leaving it standing beside
      // a row three lines up is a box pointing at nothing.
      closeSubmenu()
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
      case ' ': {
        ev.preventDefault()
        ev.stopPropagation()
        if (active === null) return
        // Enter on a parent row opens its list rather than doing nothing — `run` is null for
        // those by construction, so without this the key would be dead on exactly the rows a
        // keyboard user most needs it on.
        const opened = enterSubmenu(active)
        if (!opened) run(active)
        return
      }
      /*
       * The submenu keys. Right opens and enters, Left closes — which is the convention every
       * desktop menu follows, and it is why this file's old comment said "there are no
       * submenus here, and closing on Left is what every menu that lacks them does".
       *
       * Left still closes the whole menu on a row that has no submenu open, because that is
       * what it did before and there is nothing else for it to mean at the top level.
       */
      case 'ArrowRight': {
        if (active === null) return
        if (!enterSubmenu(active)) return
        ev.preventDefault()
        ev.stopPropagation()
        return
      }
      case 'ArrowLeft':
        ev.preventDefault()
        ev.stopPropagation()
        if (submenu !== null) return closeSubmenu()
        return onClose()
      default:
        return
    }
  }

  /**
   * The submenu's own key handling.
   *
   * A separate handler rather than a branch inside `onKeyDown`, because the submenu box is a
   * portal *sibling* of the main one: a keypress on a submenu row never bubbles to the main
   * box's `onKeyDown` at all, so a branch there would be unreachable.
   */
  const onSubKeyDown = (ev: React.KeyboardEvent<HTMLDivElement>) => {
    if (submenu === null) return
    const move = (motion: 'next' | 'prev' | 'first' | 'last') => {
      ev.preventDefault()
      ev.stopPropagation()
      setSubActive((current) => moveFocus(submenu.entries, current, motion))
    }
    switch (ev.key) {
      case 'Escape':
        ev.preventDefault()
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
      case 'Tab':
        return move(ev.shiftKey ? 'prev' : 'next')
      case 'ArrowLeft':
        ev.preventDefault()
        ev.stopPropagation()
        return closeSubmenu()
      case 'Enter':
      case ' ':
        ev.preventDefault()
        ev.stopPropagation()
        if (subActive !== null) runSub(subActive)
        return
      default:
        return
    }
  }

  const hasToggle = entries.some((e) => e.kind === 'item' && e.checked !== null)
  const subHasToggle =
    submenu !== null && submenu.entries.some((e) => e.kind === 'item' && e.checked !== null)
  const subAt =
    submenu !== null && subPlacement?.index === submenu.index ? subPlacement.at : null

  return createPortal(
    <>
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
              aria-haspopup={entry.submenu === null ? undefined : 'menu'}
              aria-expanded={entry.submenu === null ? undefined : submenu?.index === index}
              tabIndex={-1}
              // Not the `disabled` attribute: a disabled <button> takes no pointer events, so
              // its `title` never appears and the reason becomes unreadable on hover.
              aria-disabled={entry.enabled ? undefined : true}
              data-item={entry.id}
              data-enabled={entry.enabled ? 'true' : 'false'}
              data-submenu={entry.submenu === null ? undefined : 'true'}
              title={entry.reason ?? undefined}
              onClick={(ev) => {
                // A parent row opens its list and does nothing else. Clicking it must not also
                // dismiss the menu, which is what `run`'s `onClose` would do.
                if (entry.submenu !== null) {
                  return openSubmenu(index, ev.currentTarget, entry.submenu)
                }
                run(index)
              }}
              // Hover moves the keyboard cursor too, so arrowing after reaching for the mouse
              // continues from where the eye is rather than from where the keyboard was.
              //
              // And hover is the gesture the user asked for here: moving onto *Send … to
              // Claude* shows the sessions without a click. Moving onto any other row closes
              // whatever was open, so a submenu never outlives the row it belongs to.
              onPointerEnter={(ev) => {
                if (entry.enabled) setActive(index)
                if (entry.enabled && entry.submenu !== null) {
                  openSubmenu(index, ev.currentTarget, entry.submenu)
                } else if (submenu !== null) {
                  // Including for a *disabled* row, which is why this is not inside the
                  // `enabled` guard: a submenu left standing beside a row the pointer has
                  // moved off is a box pointing at nothing, and the greyed rows are exactly
                  // the ones a pointer crosses on the way past.
                  closeSubmenu()
                }
              }}
            >
              {hasToggle && (
                <span className={styles.check} aria-hidden="true">
                  {entry.checked === true ? <Icon name="check" size={0} /> : null}
                </span>
              )}
              <span className={styles.label}>{entry.label}</span>
              {entry.reason !== null && <span className={styles.reason}>{entry.reason}</span>}
              {entry.hint !== null && <span className={styles.hint}>{entry.hint}</span>}
              {entry.submenu !== null && (
                <span className={styles.chevron} aria-hidden="true">
                  <Icon name="chevron-right" size={1} />
                </span>
              )}
            </button>
          ),
        )}
      </div>

      {submenu !== null && (
        <div
          ref={subBox}
          className={subAt === null ? `${styles.menu} ${styles.measuring}` : styles.menu}
          style={
            subAt === null
              ? undefined
              : { left: subAt.x, top: subAt.y, maxHeight: subAt.maxHeight }
          }
          role="menu"
          aria-label={`${label} submenu`}
          aria-orientation="vertical"
          tabIndex={-1}
          data-audit="contextSubmenu"
          data-flipped-x={subAt?.flippedX === true ? 'true' : 'false'}
          onKeyDown={onSubKeyDown}
          onContextMenu={(ev) => {
            ev.preventDefault()
            ev.stopPropagation()
          }}
        >
          {submenu.entries.map((entry, index) =>
            entry.kind === 'separator' ? (
              <div key={entry.id} className={styles.separator} role="separator" />
            ) : (
              <button
                key={entry.id}
                type="button"
                ref={(el) => {
                  if (el === null) subItems.current.delete(index)
                  else subItems.current.set(index, el)
                }}
                className={[
                  styles.item,
                  entry.enabled ? '' : styles.disabled,
                  entry.danger ? styles.danger : '',
                  subActive === index ? styles.active : '',
                ]
                  .filter((c) => c !== '')
                  .join(' ')}
                role={entry.checked === null ? 'menuitem' : 'menuitemcheckbox'}
                aria-checked={entry.checked === null ? undefined : entry.checked}
                tabIndex={-1}
                aria-disabled={entry.enabled ? undefined : true}
                data-item={entry.id}
                data-enabled={entry.enabled ? 'true' : 'false'}
                title={entry.reason ?? undefined}
                onClick={() => runSub(index)}
                onPointerEnter={() => entry.enabled && setSubActive(index)}
              >
                {subHasToggle && (
                  <span className={styles.check} aria-hidden="true">
                    {entry.checked === true ? <Icon name="check" size={0} /> : null}
                  </span>
                )}
                <span className={styles.label}>{entry.label}</span>
                {entry.reason !== null && <span className={styles.reason}>{entry.reason}</span>}
                {entry.hint !== null && <span className={styles.hint}>{entry.hint}</span>}
              </button>
            ),
          )}
        </div>
      )}
    </>,
    menuRoot(),
  )
}
