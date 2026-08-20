/**
 * Every decision a context menu makes, with none of the DOM it makes them about.
 *
 * There is no browser and no jsdom in this repo's harness, so anything that can only be
 * observed by rendering is effectively unverified. The three things a reviewer will actually
 * want proved about a menu — *which items can be clicked*, *where the box lands when the
 * pointer is near an edge*, and *what the arrow keys do* — are all pure functions of numbers
 * and records, so they live here and `ui/scripts/check-menus.mjs` compiles this file on its
 * own and exercises them. `ContextMenu.tsx` is then the thin part: measure, call, paint.
 *
 * DOM-free and import-free on purpose, in the same way and for the same reason as
 * `src/keys/when.ts` and `src/settings/theme.ts`. Do not import React, `@/…`, or a DOM type
 * into this file — `check-menus.mjs` compiles it with a bare `tsc` invocation and adding an
 * import is how that stops working.
 */

// ---------------------------------------------------------------------------------------
// What a caller writes
// ---------------------------------------------------------------------------------------

/** A rule in the menu. Renders as a hairline; never focusable. */
export interface MenuSeparator {
  readonly kind: 'separator'
}

/**
 * One clickable line.
 *
 * There is deliberately no `disabled: boolean` and no `hint: string`.
 *
 * `disabled` lost to [`disabledReason`] because a greyed-out control with no explanation is
 * the single most common way this app has wasted a user's time — they cannot tell "not
 * applicable here" from "broken". Requiring the sentence to disable the item makes the
 * explanation unskippable rather than optional.
 *
 * A literal `hint` lost to [`command`] because a hardcoded `Ctrl+C` is wrong the moment
 * anyone edits their keymap, and it is wrong *silently*. The shortcut shown always comes
 * from the same resolved keymap the key gate dispatches from, or is not shown at all.
 */
export interface MenuItem {
  readonly kind?: 'item' | undefined
  /** Unique within one menu; it is the React key and the handle a check script asserts on. */
  readonly id: string
  readonly label: string
  /**
   * The command id whose binding is shown on the right, e.g. `file.save`. Purely cosmetic —
   * clicking runs [`run`], not the command — so an item may name a command it does not
   * dispatch, and an item that dispatches through the command layer should still name it.
   */
  readonly command?: string | undefined
  /** Present ⇒ the item is disabled, and this is the sentence shown in place of the hint. */
  readonly disabledReason?: string | undefined
  /** Destructive. Painted in `--red`; changes nothing about behaviour. */
  readonly danger?: boolean | undefined
  /** Set (either way) ⇒ the item is a toggle and gets a check gutter. */
  readonly checked?: boolean | undefined
  /** Absent ⇒ disabled, *unless* [`submenu`] is present. See [`NO_ACTION_REASON`]. */
  readonly run?: (() => void) | undefined
  /**
   * The item opens a submenu instead of doing something. One level deep, and no deeper.
   *
   * # Why a thunk and not an array
   *
   * A submenu is opened by a hover, which is at minimum a couple of hundred milliseconds after
   * the parent menu opened, and the one caller that needs this — the code pane's *Send … to
   * Claude ▸* — is listing **live Claude sessions with the names their user gave them**. Those
   * names are read off disk by Rust and arrive asynchronously; a snapshot taken when the parent
   * menu was built would show a list one round trip stale, which for a menu of conversations to
   * type into is the difference between naming the right one and naming the one before it.
   *
   * So the array is built when the submenu opens, not when its parent does. That is also the
   * rule `useContextMenu`'s `items` already follows one level up, for the same reason it states
   * there: nothing is computed until the gesture happens, so nothing can be stale.
   *
   * # Why one level
   *
   * Nothing in this app has ever wanted two, and each extra level is a placement pass, a focus
   * chain and a dismissal rule that no check script can drive without a DOM. `ContextMenu`
   * renders exactly one submenu box and ignores a `submenu` inside one; if a second level is
   * ever genuinely needed, that box is where the recursion goes, not here.
   *
   * # `run` is ignored when this is set
   *
   * A row cannot both open a list and do a thing — the click that would fire it is the same
   * click that opens the submenu, so one of the two would be unreachable by mouse. The parent
   * row opens the submenu and nothing else; [`resolveMenu`] drops any `run` beside it rather
   * than leaving a caller to find out which one won.
   */
  readonly submenu?: (() => readonly MenuEntry[]) | undefined
}

export type MenuEntry = MenuItem | MenuSeparator

// ---------------------------------------------------------------------------------------
// What the component renders
// ---------------------------------------------------------------------------------------

export interface ResolvedItem {
  readonly kind: 'item'
  readonly id: string
  readonly label: string
  readonly enabled: boolean
  /** `null` iff enabled. */
  readonly reason: string | null
  /** The keychip, e.g. `⌘S` — or `null` when the command is unbound or unnamed. */
  readonly hint: string | null
  readonly danger: boolean
  /** `null` when the item is not a toggle, so "unchecked" and "not a toggle" stay distinct. */
  readonly checked: boolean | null
  readonly run: (() => void) | null
  /**
   * Builds the submenu's entries when it opens, or `null` for an ordinary row.
   *
   * Never both this and [`run`]: a row is one or the other, and `resolveMenu` is what makes
   * that true rather than the caller. A disabled parent carries `null` here for the same
   * reason a disabled item carries a `null` `run` — an enablement a caller ignores must not
   * leave a live handle behind.
   */
  readonly submenu: (() => readonly MenuEntry[]) | null
}

export interface ResolvedSeparator {
  readonly kind: 'separator'
  readonly id: string
}

export type ResolvedEntry = ResolvedItem | ResolvedSeparator

/**
 * Why an item with no `run` is disabled.
 *
 * A menu line that does nothing when clicked is the same failure as the `+ row` buttons and
 * the search button before it: from the outside, unwired and broken are indistinguishable.
 * Rather than trust every caller to remember, an item without a handler is disabled by
 * construction and says so. Callers that mean "not here" supply their own better sentence.
 */
export const NO_ACTION_REASON = 'Not available here'

export interface ResolveOptions {
  /**
   * `Keymap.chipFor` — command id to keychip, `null` when unbound. Omit and no item gets a
   * hint, which is what a fixture with no keymap should look like.
   */
  readonly chipFor?: ((command: string) => string | null) | undefined
}

/**
 * Normalise a caller's entries into what gets painted.
 *
 * Separator hygiene is done here rather than in CSS (`:first-child`/`:last-child` selectors)
 * because callers build menus conditionally — `...(canDelete ? [item] : [])` — and the
 * common result is a leading rule, a trailing rule, or two rules with nothing between them.
 * A collapsed run is one rule; the ends are dropped. Doing it in the model also means the
 * emptiness test below is honest: a menu of nothing but separators is an empty menu.
 */
export function resolveMenu(
  entries: readonly MenuEntry[],
  options: ResolveOptions = {},
): ResolvedEntry[] {
  const { chipFor } = options
  const out: ResolvedEntry[] = []
  let pendingSeparator = false
  let separators = 0

  for (const entry of entries) {
    if (entry.kind === 'separator') {
      // Only remembered. It is emitted when — and if — a following item earns it, which
      // collapses runs and drops the trailing one in the same move.
      if (out.length > 0) pendingSeparator = true
      continue
    }

    if (pendingSeparator) {
      out.push({ kind: 'separator', id: `sep-${separators++}` })
      pendingSeparator = false
    }

    // A submenu is an action for this purpose: the row does something when it is clicked —
    // it opens a list — so it is not the wired-to-nothing shape `NO_ACTION_REASON` names.
    const inert = entry.run === undefined && entry.submenu === undefined
    const reason = entry.disabledReason ?? (inert ? NO_ACTION_REASON : null)
    const enabled = reason === null
    out.push({
      kind: 'item',
      id: entry.id,
      label: entry.label,
      enabled,
      reason,
      // Disabled items get no chip: the slot is showing the reason instead, and a shortcut
      // beside a line that refuses to run reads as a promise the menu will not keep.
      hint: enabled && entry.command !== undefined && chipFor ? chipFor(entry.command) : null,
      danger: entry.danger === true,
      checked: entry.checked === undefined ? null : entry.checked,
      // A parent never runs. See `MenuItem.submenu`: the click that would fire it is the click
      // that opens the list, so shipping both would make one of the two unreachable by mouse.
      run: enabled && entry.submenu === undefined ? (entry.run ?? null) : null,
      submenu: enabled ? (entry.submenu ?? null) : null,
    })
  }

  return out
}

/** Nothing to show. The hook refuses to open one — an empty box at the pointer is a bug. */
export function isEmptyMenu(entries: readonly ResolvedEntry[]): boolean {
  return !entries.some((e) => e.kind === 'item')
}

/**
 * Every item is disabled.
 *
 * Still worth opening: "Rename — the file is outside the project" is an answer, and closing
 * over silence is not. Exposed so a caller can decide otherwise for its own surface.
 */
export function isInertMenu(entries: readonly ResolvedEntry[]): boolean {
  return !entries.some((e) => e.kind === 'item' && e.enabled)
}

// ---------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------

export interface Point {
  readonly x: number
  readonly y: number
}

export interface Size {
  readonly width: number
  readonly height: number
}

export interface Placement {
  readonly x: number
  readonly y: number
  /** The menu opens leftwards from the pointer. */
  readonly flippedX: boolean
  /** The menu opens upwards from the pointer — its *bottom* edge is at the pointer. */
  readonly flippedY: boolean
  /** What the box may grow to before it has to scroll. Always ≥ 0. */
  readonly maxHeight: number
}

/** Gutter kept between the menu and the window edge, in px. */
export const MENU_MARGIN = 8

/**
 * Where the box goes.
 *
 * The viewport here is the **window**, not the screen. This app is undecorated and is
 * routinely a fraction of the display, and a right-click near the bottom of a 500px-tall
 * detached pane window must flip inside that window — `screen.availHeight` would happily
 * report room that this webview cannot paint into. Callers pass `innerWidth`/`innerHeight`.
 *
 * Flip, then clamp, and only flip when the other side is genuinely roomier. Flipping merely
 * because the preferred side is short would send a menu upwards next to a pointer that has
 * plenty of space below it and merely sits under a tall menu; comparing the two spaces keeps
 * the common case — open down and right — and reserves the flip for the edge that needs it.
 * The clamp afterwards is what handles the case neither side can hold: the box is pinned to
 * the margin and `maxHeight` makes it scroll rather than run off.
 */
export function placeMenu(
  at: Point,
  size: Size,
  viewport: Size,
  margin: number = MENU_MARGIN,
): Placement {
  const spaceRight = viewport.width - margin - at.x
  const spaceLeft = at.x - margin
  const flippedX = size.width > spaceRight && spaceLeft > spaceRight
  const x = clamp(
    flippedX ? at.x - size.width : at.x,
    margin,
    viewport.width - margin - size.width,
  )

  const spaceBelow = viewport.height - margin - at.y
  const spaceAbove = at.y - margin
  const flippedY = size.height > spaceBelow && spaceAbove > spaceBelow
  const y = clamp(
    flippedY ? at.y - size.height : at.y,
    margin,
    viewport.height - margin - size.height,
  )

  /*
   * The budget is measured from `y` — the position after the clamp — not from the pointer.
   *
   * `spaceBelow` is the room below the *pointer*, and when the box did not fit there the
   * clamp has already moved it up; billing it for the room below the pointer would then cap a
   * box that had just been given more. A 900px menu at y=150 in a 300px window lands at the
   * top margin with 284px of room and would have been told it had 142 — half the window blank
   * underneath and a scrollbar with nothing forcing it. `viewport.height - margin - y` is the
   * same number as `spaceBelow` whenever the clamp did not fire, so the ordinary case is
   * unchanged.
   *
   * The flipped branch stays on `spaceAbove` deliberately: there the point of the placement is
   * that the box's *bottom* edge sits on the pointer, so its budget is the room above the
   * pointer and growing past it would undo the flip.
   */
  const maxHeight = Math.max(
    0,
    flippedY
      ? Math.min(spaceAbove, viewport.height - margin * 2)
      : viewport.height - margin - y,
  )

  return { x, y, flippedX, flippedY, maxHeight }
}

/**
 * `lo` wins when the range is inverted, which is the case that matters: a box wider than the
 * window makes `hi < lo`, and pinning to the left/top margin is the readable half to keep.
 */
function clamp(value: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(value, hi))
}

/** The parent row a submenu hangs off, in window coordinates. A `DOMRect` is assignable. */
export interface Rect {
  readonly left: number
  readonly right: number
  readonly top: number
  readonly bottom: number
}

/**
 * Where a submenu goes: beside its parent row, not under the pointer.
 *
 * # Why this is not `placeMenu` with a cleverer anchor
 *
 * `placeMenu` flips a box **through** its anchor — the anchor is a pointer, and a pointer has
 * no width, so `x - width` is the correct leftward placement. A submenu's anchor is a *row*,
 * and flipping through its right edge would lay the submenu straight over the parent menu it
 * came out of: the user would lose the row they are hovering, which is the one thing that has
 * to stay visible while a submenu is open. The flip has to go round the row, from `rect.right`
 * to `rect.left - width`, and that is a different sum from the one `placeMenu` does.
 *
 * The rest follows the same rules as `placeMenu` and for the same reasons: flip only when the
 * other side is genuinely roomier (so the ordinary case stays "open rightwards"), clamp
 * afterwards for the window that can hold neither, and bill `maxHeight` from the placed `y`
 * rather than from the anchor, so a box the clamp has already moved up is not told it has less
 * room than it was given.
 *
 * Vertically the box's top starts level with the row's top rather than its bottom, so the
 * first submenu item sits beside the row that opened it. That is what makes a diagonal mouse
 * path from row to submenu work at all, and it is what every desktop menu does.
 */
export function placeSubmenu(
  anchor: Rect,
  size: Size,
  viewport: Size,
  margin: number = MENU_MARGIN,
): Placement {
  const spaceRight = viewport.width - margin - anchor.right
  const spaceLeft = anchor.left - margin
  const flippedX = size.width > spaceRight && spaceLeft > spaceRight
  const x = clamp(
    flippedX ? anchor.left - size.width : anchor.right,
    margin,
    viewport.width - margin - size.width,
  )

  const y = clamp(anchor.top, margin, viewport.height - margin - size.height)
  return {
    x,
    y,
    flippedX,
    // A submenu is never "flipped up": its top edge is placed, then clamped, so the box grows
    // downwards from wherever it landed. The field is on `Placement` for `placeMenu`'s sake
    // and is reported honestly rather than left to a caller to interpret.
    flippedY: false,
    maxHeight: Math.max(0, viewport.height - margin - y),
  }
}

/**
 * A keyboard-invoked menu (Shift+F10, the Menu key) has no pointer to sit at, so it hangs off
 * the element instead — bottom-left corner, the convention every desktop menu follows.
 */
export function anchorToRect(rect: {
  readonly left: number
  readonly bottom: number
}): Point {
  return { x: rect.left, y: rect.bottom }
}

// ---------------------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------------------

export type MenuMotion = 'next' | 'prev' | 'first' | 'last'

/**
 * The next index the arrow keys should land on, or `null` when nothing in the menu can take
 * focus.
 *
 * Indices are into the **resolved** array, separators included, so the value is directly the
 * component's `ResolvedEntry[]` index and nothing has to hold a second parallel list.
 *
 * Wraps, because a menu is short and a user holding Down expects to come back around rather
 * than to stick. Skips separators and disabled items: focusing a line that cannot run is a
 * dead keypress, and the reason text is already visible without focusing it.
 *
 * An `from` that is out of range, a separator, or a disabled item is treated as "nowhere" —
 * which is exactly what happens when the menu re-resolves under a moving selection and the
 * previously focused item goes away.
 */
export function moveFocus(
  entries: readonly ResolvedEntry[],
  from: number | null,
  motion: MenuMotion,
): number | null {
  const focusable: number[] = []
  for (let i = 0; i < entries.length; i++) {
    const entry = entries[i] as ResolvedEntry
    if (entry.kind === 'item' && entry.enabled) focusable.push(i)
  }
  if (focusable.length === 0) return null

  const first = focusable[0] as number
  const last = focusable[focusable.length - 1] as number
  if (motion === 'first') return first
  if (motion === 'last') return last

  const at = from === null ? -1 : focusable.indexOf(from)
  if (at < 0) return motion === 'next' ? first : last

  const step = motion === 'next' ? 1 : -1
  return focusable[(at + step + focusable.length) % focusable.length] as number
}

/** The item Enter/Space would run at `index`, or `null` if that index cannot run. */
export function activeItem(
  entries: readonly ResolvedEntry[],
  index: number | null,
): ResolvedItem | null {
  if (index === null) return null
  const entry = entries[index]
  if (entry === undefined || entry.kind !== 'item' || !entry.enabled) return null
  return entry
}

// ---------------------------------------------------------------------------------------
// Handing focus back
// ---------------------------------------------------------------------------------------

/**
 * One step of the focus handoff a closing menu performs.
 *
 * * `custom` — the surface's own `restoreFocus`. It may decline (return `false`), which is
 *   why this is a *chain* and not a single verdict.
 * * `default` — `previous.focus({ preventScroll: true })` on the element that had focus when
 *   the menu opened.
 */
export type FocusReturnStep = 'custom' | 'default'

/**
 * What a closing menu should try, in order, to give the keyboard back.
 *
 * # Why this is a list and not a verdict
 *
 * A surface's `restoreFocus` is allowed to say "not now" — `useCodeMenu`'s returns `false`
 * when the `EditorView` has already been destroyed, which happens whenever the menu's action
 * closed the tab it hung off. Collapsing this to a single answer would mean either dropping
 * the fallback (a window left with focus on `<body>`) or running both unconditionally (the
 * editor focused, then a stale `.cm-content` focused on top of it). So the caller walks the
 * chain and stops at the first step that reports success.
 *
 * # Why `custom` outranks `default`, and why `default` exists at all
 *
 * `default` is a raw DOM `focus()`, and on a CodeMirror surface that is not merely coarse —
 * it is *wrong*, and the reason is the bug this function was written for. WebKit clears the
 * document selection on every focus change (`FocusController::setFocusedElement` →
 * `clearSelectionIfNeeded`), so by the time the menu closes the editor's DOM selection is
 * gone; `Element::updateFocusAppearance` then sees a root editable element whose frame has no
 * selection, sets one at `firstPositionInOrBeforeNode(this)` and reveals it. The buffer jumps
 * to line 1. `preventScroll` suppresses the reveal and *not* the `setSelection` that precedes
 * it, so the caret is still collapsed to the top and CodeMirror's `DOMObserver` reads it back
 * into state. Only `EditorView.focus()` — which is `focusPreventScroll` **plus**
 * `docView.updateSelection()` — restores both, and only the surface can call it.
 *
 * # Why a disconnected `previous` is skipped rather than focused
 *
 * A menu whose action deleted the row it hung off would otherwise focus a detached node and
 * leave the window with no focus at all. Doing nothing is better: focus falls to `<body>` and
 * the next Tab starts from the top rather than from nowhere. `custom` is still offered in that
 * case, because a surface that rebuilt its DOM under the menu is exactly the surface that knows
 * where focus should go.
 */
export function focusReturnPlan(options: {
  /** The surface supplied a `restoreFocus`. */
  readonly hasCustom: boolean
  /** The element that had focus at open time is still in the document. */
  readonly previousConnected: boolean
}): readonly FocusReturnStep[] {
  const steps: FocusReturnStep[] = []
  if (options.hasCustom) steps.push('custom')
  if (options.previousConnected) steps.push('default')
  return steps
}

// ---------------------------------------------------------------------------------------
// Whose menu is this — ours or the webview's
// ---------------------------------------------------------------------------------------

/** The attribute a surface writes to opt back into the webview's own menu, or out of it. */
export const NATIVE_MENU_ATTR = 'data-native-menu'

/** One element of the ancestor chain, as much of it as the decision below can read. */
export interface ElementFacts {
  /** Lowercased tag name. */
  readonly tag: string
  /** `<input type>`, lowercased. Absent is `text`, which is what HTML says. */
  readonly type?: string | undefined
  /** The `data-native-menu` attribute value; `null`/absent when the element has none. */
  readonly nativeMenu?: string | null | undefined
  /** True when `isContentEditable` is. */
  readonly contentEditable?: boolean | undefined
  readonly disabled?: boolean | undefined
}

/**
 * `<input type>` values that hold text a user edits by hand.
 *
 * `checkbox`, `radio`, `range`, `color`, `file`, `button` and the date family are excluded:
 * none of them has a selection, so the native menu offers cut/copy/paste that do nothing.
 */
const TEXT_INPUT_TYPES = new Set([
  '',
  'text',
  'search',
  'url',
  'tel',
  'email',
  'password',
  'number',
])

function isTextEntry(el: ElementFacts): boolean {
  if (el.disabled === true) return false
  if (el.contentEditable === true) return true
  if (el.tag === 'textarea') return true
  // A read-only input is still worth a native menu: select-all and copy work on it, and it
  // is the shape a "path of this file" field takes.
  return el.tag === 'input' && TEXT_INPUT_TYPES.has((el.type ?? '').toLowerCase())
}

/**
 * Should the webview be allowed to draw its own menu here?
 *
 * `chain` is the target and its ancestors, **innermost first**.
 *
 * The rule is two-tier on purpose. An explicit `data-native-menu` anywhere in the chain is a
 * statement someone wrote, so the innermost one wins outright — that is how a pane opts its
 * whole subtree out (`data-native-menu="false"`) and how one field inside it opts back in.
 * Only when nobody stated anything does the text-entry default apply.
 *
 * `nativeInTextInputs` is the answer to the one real conflict in this feature. The user asked
 * for no "Inspect element"; devtools are enabled, so WebKitGTK puts that line in the menu it
 * draws — and that menu is also the only thing in this app offering cut/copy/paste on a plain
 * `<input>`. The default is `true`: text fields keep the native menu, because silently
 * removing paste is a regression nobody asked for and *is* the whole reason the escape hatch
 * exists. The alternative that lost was shipping our own Cut/Copy/Paste items — it loses on
 * paste specifically: WebKit refuses `document.execCommand('paste')` from page script, so
 * that item would be drawn and then quietly do nothing, which is worse than a menu with an
 * extra line in it. (`tauri-plugin-clipboard-manager` is registered in `cide-app`, but its JS
 * package is not a dependency of `ui`; adding it is the route to flipping this default, and
 * `installNativeMenuSuppression({ nativeInTextInputs: false })` is the switch once someone
 * does.) Everywhere that is not a text field — trees, tabs, terminals, the editor's own
 * surface, code panes — is suppressed, which is all of the app the user was right-clicking.
 */
export function wantsNativeMenu(
  chain: readonly ElementFacts[],
  nativeInTextInputs: boolean,
): boolean {
  for (const el of chain) {
    const stated = el.nativeMenu
    if (stated === undefined || stated === null) continue
    const value = stated.toLowerCase()
    // A bare `data-native-menu` reads as an empty string and means "yes"; that is how a bare
    // boolean attribute behaves everywhere else in HTML.
    return value !== 'false' && value !== '0' && value !== 'off'
  }

  if (!nativeInTextInputs) return false
  // Any element in the chain, not just the target: a right-click inside a contenteditable
  // lands on whatever `<span>` the text happens to be wrapped in.
  return chain.some(isTextEntry)
}
