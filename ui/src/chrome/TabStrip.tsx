/**
 * The 30px workspace tab strip, below the header and to the right of the rail.
 *
 * The first tab is the pinned Claude console and never draws a close button. That is read
 * off `kind === 'claudeHome'`, never off the index: array order is a rendering detail,
 * whereas the pin is a property of the tab. Withholding the button is a courtesy to the
 * user, not the enforcement — the Rust side stays the authority that refuses the close.
 *
 * The component takes its whole *layout* from props, so a detached-tab window mirroring a
 * different workspace slice can render the same strip. It is no longer store-free, and the
 * note that used to claim so needs its exception stated: the awaiting marker below reads a
 * module-level table keyed by session id. That is not a workspace slice — what a strip shows
 * is still decided entirely by the tabs it is handed — and it cannot be a prop, for the
 * reason `PaneTitleBar` gives at length: the state is a *history* of `cide://session-state`
 * that no caller holds, and threading it through would need a line in `App.tsx`, which is how
 * three features in this app have shipped wired to nothing.
 *
 * # Why a background tab needs its own marker
 *
 * Not because its panes are gone — they are emphatically not. `TabContent` mounts *every*
 * tab's tree and hides the inactive ones with `visibility: hidden`, and `paneHosts` forbids
 * anything weaker or stronger (rule 2: never conditionally render a pane; `display: none`
 * zeroes xterm's measurements, unmounting destroys its scrollback). So a background pane's
 * title bar, and the awaiting marker in it, exist in the DOM the whole time.
 *
 * They are simply *unreachable*: `visibility: hidden` takes an element out of painting and
 * out of hit-testing, so the OS title says one session wants you and the only surface that
 * could say which is behind a tab you have to guess at. The tab strip is the one part of a
 * background tab that stays visible, which is why the count goes here.
 *
 * # The `▾` at the right end, and why it is a sibling of the tabs box
 *
 * `.tabs` is `overflow: hidden` and `.tab` is `flex: none`, so an overfull strip *clips*: the
 * tabs past the edge are not painted, not hit-testable and — as `useTabDrag` records — not even
 * droppable. The chevron opens a list of exactly those, and activating one switches to it. The
 * rule for which those are is `tabOverflow.clippedTabs`, measured off the DOM below.
 *
 * Two decisions about where it lives, both of which have already been made once in this app:
 *
 * 1. **Outside the tablist.** `.tabs` is `role="tablist"`, and a generic element inside it
 *    breaks the ownership the tabs pattern requires — the same reason `TabItem`'s row is
 *    `role="presentation"`. It is also the bug `AppHeader` fixed by moving `<ProjectMenu>` out
 *    of *its* `.tabs` box: a control that opens the list of what is clipped, placed inside the
 *    box that does the clipping, eventually clips itself. Outside, it is pinned.
 * 2. **The slot is always rendered; only the button is conditional.** So the tabs box has the
 *    same width whether or not the chevron is showing, and `clippedTabs` stays a function of
 *    geometry alone. A slot that appeared with the button would narrow the tabs box, which can
 *    clip one more tab, which keeps the button on — two self-consistent states with the outcome
 *    depending on which way the window was last resized, plus a width literal in TypeScript that
 *    has to agree with the stylesheet for ever. The reserve is 24px of the strip's right-hand
 *    third, which is empty spacer in every state except an overfull one.
 *
 * The tabs themselves are *not* shrunk to make room, and that is a requirement rather than a
 * preference: `.tab { flex: none }` is what keeps the 12/13px paddings the chrome audit measures,
 * and a horizontal reserve taken out of the thing being reserved against has been rejected in
 * this app once already (the pane cluster over the find bar, README's M16).
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { useContextMenu } from '@/menus'
import type { Tab, TabId, TabKind } from '@/ipc/client'
import { useAwaitingInTab } from '@/panes/awaiting'
import { cancelResizeSettle, whenResizeSettles } from '@/layout/resizeGesture'
import { awaitingBadge, awaitingHint } from '@/panes/awaitingRule'
import { overflowEntries, tabMenuEntries } from './menuModel'
import { clippedTabs, overflowHint } from './tabOverflow'
import { useTabDrag } from './useTabDrag'
import styles from './TabStrip.module.css'

export interface TabStripProps {
  tabs: Tab[]
  activeTab: TabId
  onActivate?: ((id: TabId) => void) | undefined
  onClose?: ((id: TabId) => void) | undefined
  /**
   * *Show history for this file* — opens a tab in the git tool window. (M18)
   *
   * Takes an **absolute** path; `menuModel::absolutePathOf` decides whether the item is offered
   * at all, because a git diff tab's path is repo-relative and would name nothing the backend
   * can resolve. Absent in a window with no tool window, which disables the item with a reason
   * rather than hiding it.
   */
  onShowHistory?: ((path: string) => void) | undefined
  /**
   * Close several tabs as one gesture — the menu's *Close others* / *to the left* / *to the
   * right*.
   *
   * Separate from `onClose` because it must ask about unsaved work **once**, naming every file
   * at stake, rather than once per tab. See `TabActions.closeMany` and the store's `closeTabs`.
   */
  onCloseMany?: ((ids: TabId[]) => void) | undefined
  /**
   * Split the **active** tab's focused pane sideways.
   *
   * The ⊞ button this used to sit behind is gone. It was not a second capability alongside the
   * header's ⊞ — the two were the same call, argument for argument: `App.tsx` passed
   * `splitPane(project, activeTab, activeTab.tree.focused, 'row', 'after')` here and
   * `splitPane(project, focused.tab.id, focused.pane.id, 'row', 'after')` there, and `focused`
   * is derived from the active tab, so they resolve identically. Two buttons for one gesture is
   * one button too many, and the header's is the one the mock draws.
   *
   * The handler survives as a **context-menu item**, because the menu is where a per-tab action
   * belongs. It is offered only on the active tab: this signature has no way to say *which* tab
   * to split, and silently splitting a different tab than the one under the pointer is worse
   * than an item that says why it is unavailable.
   */
  onSplit?: (() => void) | undefined
  /**
   * Detach the active tab's focused pane into its own window.
   *
   * The ⧉ button is gone too, and this one for a different reason worth recording: it was a
   * **dead control naming a capability that does not exist**. Its tooltip said "Detach tab",
   * `App.tsx` never passed a handler, and there is no `window_detach_tab` command anywhere —
   * `WindowRole::DetachedTab` is a domain variant nothing can produce. So there was nothing to
   * preserve by renaming it, and hiding a live-looking button that does nothing on click is a
   * strict improvement.
   *
   * What *does* exist is detaching a pane, which the header's ⧉ already does. This prop is the
   * same gesture offered from the tab menu; absent, the item says it is unavailable rather than
   * pretending.
   */
  onDetach?: (() => void) | undefined
  /**
   * Move a tab along the strip. Absent makes the strip inert rather than pretending.
   *
   * `before` is the tab the dragged one lands **in front of**, `null` for the end — the shape
   * `tab.reorder` takes, and ids rather than indices for the reason that command documents: the
   * boundary is computed here against a snapshot, and the strip can change before the drop.
   *
   * Optional because `App.tsx` renders a second, handler-free `<TabStrip>` for the chrome audit,
   * and a fixture whose tabs could be dragged into a workspace that does not exist would be a
   * crash rather than a feature. `useTabDrag` refuses to start a gesture without it, so the
   * audit strip is inert by construction rather than by remembering not to touch it.
   */
  onReorder?: ((tab: TabId, before: TabId | null) => void) | undefined
}

export function TabStrip({
  tabs,
  activeTab,
  onActivate,
  onClose,
  onCloseMany,
  onSplit,
  onDetach,
  onReorder,
  onShowHistory,
}: TabStripProps) {
  const tablist = useRef<HTMLDivElement | null>(null)
  const drag = useTabDrag({
    tabs,
    container: tablist,
    // Narrowed back to the branded ids at the seam. `tabDrag.ts` works in plain strings so a bare
    // `tsc` can compile it standalone — the constraint its header states — and this is the one
    // place that knows those strings are `TabId`s.
    ...(onReorder
      ? { onReorder: (tab: string, before: string | null) => onReorder(tab as TabId, before as TabId | null) }
      : {}),
  })

  /*
   * The ids the strip is currently hiding, and the three things that can change them.
   *
   * The measurement is deliberately dumb — read every `[data-tab-id]` box, hand the numbers to
   * `clippedTabs` — because everything that could be wrong about it is arithmetic, and the
   * arithmetic is the half `check:tab-overflow` can actually run. `offsetLeft` is relative to
   * `.tabs` (it is `position: relative`, the line the drag caret also depends on), and
   * `clientWidth` is that box's *visible* width, which already tracks the window: `.tabs` is
   * `flex: 0 1 auto` next to a `flex: 1` spacer whose basis is 0, so an overfull strip shrinks
   * the tabs box and nothing else.
   *
   * `[data-tab-id]` rather than a ref per tab, exactly as `useTabDrag.aim` does: the attribute is
   * already there for the context menu, and a ref array would need its own invalidation.
   */
  const overflowAnchor = useRef<HTMLButtonElement | null>(null)
  /**
   * This strip's identity in `resizeGesture`'s queue.
   *
   * An object rather than a name: the key only has to be unique, and an identity is unique by
   * construction where a string has to be chosen and could collide with another component's.
   */
  const settleKey = useRef({})
  const [hidden, setHidden] = useState<readonly string[]>([])
  const measure = useCallback(() => {
    const box = tablist.current
    if (box === null) return
    const boxes = Array.from(box.querySelectorAll<HTMLElement>('[data-tab-id]')).map((el) => ({
      id: el.dataset.tabId ?? '',
      left: el.offsetLeft,
      width: el.offsetWidth,
    }))
    const next = clippedTabs(boxes, { viewport: box.clientWidth, scroll: box.scrollLeft })
    // Identity-stable when nothing changed, and that is not a micro-optimisation: the layout
    // effect below runs on *every* commit with no dependency array, so a fresh array every time
    // would be a state write every time — a render loop that only shows up as the app pinning a
    // core.
    setHidden((prev) =>
      prev.length === next.length && prev.every((id, i) => id === next[i]) ? prev : next,
    )
  }, [])

  /*
   * Trigger 1: every commit, before paint.
   *
   * No dependency array on purpose. Anything that changes a tab's *width* changes the answer,
   * and the list of those is not enumerable: a file going dirty swaps a 12px `×` for a 14px `•`,
   * a `claudeFull` tab's title is renamed by the agent, a badge goes from `·` to `TOML`, a tab is
   * opened or closed or reordered. A dependency array here is how the chevron goes stale, and
   * stale is worse than absent — it lists a tab that is on screen, or hides while three are not.
   *
   * `useLayoutEffect` rather than `useEffect` so the control is never one painted frame late; the
   * reads are ~12 `offsetLeft`/`offsetWidth` on elements the engine has just laid out.
   */
  useLayoutEffect(measure)

  /*
   * Trigger 2: the box changing size — or scrolling — without the tabs changing.
   *
   * Resizing the window, dragging the sidebar splitter, a detached window being tiled. None of
   * those re-render this component with different props, so no commit happens and trigger 1 never
   * fires. The `typeof` guard is the same one `PaneSlot` and `GitDiffPane` carry.
   */
  useEffect(() => {
    const box = tablist.current
    if (box === null) return
    /*
     * Deferred while a splitter is being dragged.
     *
     * `measure` reads `offsetLeft` and `offsetWidth` for every tab plus the box's own
     * `clientWidth`/`scrollLeft` — a forced layout of the strip, immediately after the frame that
     * changed its width. The answer cannot be *used* mid-drag anyway: the chevron is about where
     * the tabs ended up, and a drag is by definition still moving them. See
     * `@/layout/resizeGesture`.
     */
    const ro =
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(() => whenResizeSettles(settleKey.current, measure))
    ro?.observe(box)
    /*
     * ...and the box *scrolling* without either changing, which is the same class of miss and
     * was very nearly shipped unwired: `clippedTabs` takes `scroll` as a real input — the whole
     * reason it compares against both edges rather than only the right one — but a scroll
     * changes no box's size and re-renders nothing, so without this listener the only trigger
     * that could ever deliver a non-zero `scrollLeft` to the rule was an unrelated commit that
     * happened to follow it. `overflow: hidden` is still a scroll container and WebKit scrolls
     * one programmatically to reveal a focused descendant, so Tab-ing into a clipped tab's
     * button leaves the strip scrolled with the *leading* tabs gone: the chevron would then be
     * offering a tab that is on screen and omitting the three that are not.
     *
     * `passive` because this listener never calls `preventDefault`, and in capture for the
     * reason the menus' dismissal listener states — a `scroll` event does not bubble.
     */
    const onScroll = () => measure()
    box.addEventListener('scroll', onScroll, { passive: true })
    return () => {
      ro?.disconnect()
      cancelResizeSettle(settleKey.current)
      box.removeEventListener('scroll', onScroll)
    }
  }, [measure])

  /*
   * Trigger 3: the webfonts resolving.
   *
   * The opposite case to trigger 2, and it is the one that would have been missed. An unloaded
   * face is measured at the fallback stack's metrics — `App.tsx` already waits on `fonts.ready`
   * for exactly this — and when the swap lands on an *already overfull* strip, every tab changes
   * width while `.tabs` does not, so the ResizeObserver never fires and no re-render happens
   * either. The mounted flag is because `fonts.ready` can settle after this strip is gone: a
   * detached-tab window's strip outlives nothing, but the audit fixture's does.
   */
  useEffect(() => {
    if (typeof document === 'undefined' || document.fonts === undefined) return
    let alive = true
    void document.fonts.ready.then(() => {
      if (alive) measure()
    })
    return () => {
      alive = false
    }
  }, [measure])

  const menu = useContextMenu({
    label: 'Tab',
    items: ({ target }) => {
      const id = target?.closest<HTMLElement>('[data-tab-id]')?.dataset.tabId
      const tab = tabs.find((t) => t.id === id)
      // A right-click on the strip's empty space, not on a tab. Declining to open beats a menu
      // whose every item is about a tab the user did not aim at.
      if (!tab) return []
      // The clipboard is resolved here rather than in the model, so the model stays DOM-free
      // and `check:menu-model` can compile it on its own. Absent ⇒ Copy path is disabled with
      // a reason, never drawn and dead — WebKitGTK does not always expose one.
      const clipboard = typeof navigator === 'undefined' ? undefined : navigator.clipboard
      return tabMenuEntries(tabs, tab, activeTab, {
        close: onClose,
        closeMany: onCloseMany,
        split: onSplit,
        detach: onDetach,
        ...(clipboard ? { copy: (text: string) => void clipboard.writeText(text) } : {}),
        ...(onShowHistory ? { history: onShowHistory } : {}),
      })
    },
  })

  /*
   * The `▾` list, on the same hook the strip's context menu uses.
   *
   * Reusing `useContextMenu` rather than writing a popup is the whole of the keyboard and
   * dismissal story: Escape, arrows, Home/End, Tab cycling within the menu, pointerdown outside,
   * scroll, resize and blur all come with it, it portals to the app root so no `overflow: hidden`
   * ancestor can clip it, and `placeMenu` flips it near the window edge — which matters here more
   * than anywhere, because this control lives *at* the right edge by construction.
   *
   * The one inherited wart: pressing `▾` while its menu is open re-opens rather than toggling
   * shut, because the hook's capture-phase `pointerdown` dismissal fires before the `click`.
   * `ProjectMenu`'s `▾` behaves identically today, and matching it beats inventing a fourth
   * dismissal path for one button.
   *
   * `items` is called at open time, so the list is built from the measurement as it stands when
   * the user presses — not from whatever was true at the last render.
   */
  const overflow = useContextMenu({
    label: 'Tabs not in view',
    items: () => overflowEntries(tabs, hidden, { ...(onActivate ? { activate: onActivate } : {}) }),
  })

  // The `data-audit` attributes below are how chrome/layoutAudit.ts locates this surface:
  // CSS Module class names are hashed at build time, so nothing outside can select on them.
  return (
    <div
      className={styles.strip}
      data-audit="tabStrip"
      // A right-click mid-drag would open a menu about a tab that is in flight, over a caret the
      // user is still aiming — and the menu's own pointer handling would fight the capture.
      onContextMenu={(e) => {
        if (drag.state !== null) return
        menu.onContextMenu(e)
      }}
      // Kills the hover wash while a drag is in flight: the pointer is captured, so the tab under
      // it lights up as though it were the target when the caret is the only thing that says so.
      // The same channel `ChangesTree` and `FileTree` use, on the same attribute name.
      {...(drag.state !== null ? { 'data-dragging': '' } : {})}
    >
      {/* The tablist holds tabs and nothing else. It used to share the strip with a split and
          a detach button; both are gone — see `TabStripProps`. It is `position: relative` so the
          insertion caret can be absolutely positioned against it, which is also what makes
          `offsetLeft` in `useTabDrag` measure from its left edge. */}
      <div className={styles.tabs} role="tablist" aria-label="Open tabs" ref={tablist}>
        {tabs.map((tab) => (
          <TabItem
            key={tab.id}
            tab={tab}
            active={tab.id === activeTab}
            onActivate={onActivate}
            onClose={onClose}
            onPick={drag.onPointerDown}
            dragged={drag.dragged}
            inFlight={drag.state?.drag.tab === tab.id}
          />
        ))}
        {/*
         * The insertion caret: a 2px rule in the accent, drawn in the gap the drop would land in.
         *
         * A caret rather than opening a gap between two tabs, and that is a rule this stylesheet
         * already states at length for the awaiting marker: the strip is 30px tall with 12-13px
         * paddings and it reflows *under the pointer* — a tab that moves as it is being aimed at
         * is a tab that gets clicked wrong. A gap-based preview violates that on every pointer
         * move; a zero-width overlay cannot. It is also why the caret is `position: absolute`
         * rather than a flex child: a flex child would shift every tab after it by 2px and the
         * layout audit measures those positions.
         */}
        {drag.state !== null && (
          <div
            className={styles.caret}
            style={{ left: `${drag.state.caretX}px` }}
            aria-hidden="true"
          />
        )}
      </div>
      <div className={styles.spacer} />
      {/*
       * The reserved slot. Always here, empty when everything fits — see the module comment:
       * this is what keeps `.tabs`'s width independent of whether the control is showing, and
       * therefore what stops the detector oscillating at the boundary.
       */}
      <div className={styles.overflowSlot}>
        {hidden.length > 0 && (
          <button
            type="button"
            ref={overflowAnchor}
            className={styles.overflow}
            // The count lives in the tooltip and the accessible name, never on the glyph: a
            // label that grew a digit would change this slot's width, which changes what is
            // clipped, which changes the digit. See `overflowHint`.
            title={overflowHint(hidden.length)}
            aria-label={overflowHint(hidden.length)}
            aria-haspopup="menu"
            aria-expanded={overflow.isOpen}
            onClick={() => {
              const el = overflowAnchor.current
              if (el !== null) overflow.openFor(el)
            }}
          >
            {/* The glyph `ProjectMenu` already uses for "this opens a list", which is also the
                control the report asked for: "a dropdown icon (like > but down)". */}
            ▾
          </button>
        )}
      </div>
      {menu.menu}
      {overflow.menu}
    </div>
  )
}

interface TabItemProps {
  tab: Tab
  active: boolean
  onActivate?: ((id: TabId) => void) | undefined
  onClose?: ((id: TabId) => void) | undefined
  /** The press that may become a reorder. Decides for itself whether this tab can start one. */
  onPick: (e: React.PointerEvent, tab: Tab) => void
  /** Whether the gesture just ending was a drag — see the guard on `onClick` below. */
  dragged: () => boolean
  /** This is the tab in flight, so it is dimmed to show what is being moved. */
  inFlight: boolean
}

function TabItem({ tab, active, onActivate, onClose, onPick, dragged, inFlight }: TabItemProps) {
  const view = viewFor(tab.kind)
  const shell = active ? `${styles.tab} ${styles.tabActive}` : styles.tab
  const label = view.closable ? styles.label : `${styles.label} ${styles.labelPinned}`

  // The whole reason this exists: a background tab's panes are mounted but `visibility:
  // hidden`, so the marker in each title bar is painted nowhere and the `Awaiting: 1` in the
  // OS title names nobody. See the module comment.
  const waiting = useAwaitingInTab(tab)
  const badge = awaitingBadge(waiting)
  const hint = awaitingHint(waiting, 'tab')

  return (
    // `presentation` so the row does not sit between the tablist and its tabs: an
    // intervening generic element breaks the ownership the tabs pattern requires.
    <div
      className={`${shell} ${view.shape}`}
      role="presentation"
      data-audit={view.audit}
      // How the strip's one context menu finds which tab was hit; `items` runs at open time
      // and reads it back off the DOM rather than one hook per tab.
      data-tab-id={tab.id}
      data-active={active ? 'true' : 'false'}
      data-awaiting={waiting > 0 ? String(waiting) : 'false'}
      // The press that may become a reorder. On the row rather than on the label button, so a
      // grab anywhere in the tab's box starts it — the same "one box the pointer can hit
      // anywhere" contract the label's negative margins exist to preserve.
      onPointerDown={(e) => onPick(e, tab)}
      {...(inFlight ? { 'data-drag': '' } : {})}
    >
      <button
        type="button"
        className={label}
        role="tab"
        aria-selected={active}
        // The marker cannot carry its own tooltip — it is `pointer-events: none` so the tab
        // stays one box the pointer can hit anywhere — so the tab's own tooltip carries the
        // sentence, and the exact count that `9+` stops spelling out.
        title={hint ? `${view.hint} — ${hint}` : view.hint}
        // Guarded by the drag, because `click` fires after `pointerup` and a drop would
        // otherwise *also* activate the tab it just moved. Harmless-looking and not: dropping a
        // background tab into a new position would yank the user out of whatever they were
        // reading, which is a second, unasked-for consequence of a gesture about order alone.
        onClick={() => {
          if (dragged()) return
          onActivate?.(tab.id)
        }}
      >
        {view.body}
      </button>
      {/*
       * Always rendered, filled only when it means something — the same contract, and for the
       * same reason, as the marker in the pane title bar. A box that appeared and cleared
       * would change this tab's width, shove every tab after it sideways and reflow the strip
       * *under the pointer*; the strip is 30px with 12-13px paddings, so a tab that moves
       * while being aimed at is a tab that gets missed. Reserving 13px permanently is the
       * price, and the CSS states what that price actually costs.
       */}
      <span
        className={badge ? `${styles.awaiting} ${styles.awaitingOn}` : styles.awaiting}
        // `role` is on the empty box too, unlike the pane bar's marker, and the difference is
        // deliberate: a live region that comes into existence at the same moment as its
        // content is not reliably announced, because there was nothing there to be watching.
        // The region exists from mount; `aria-hidden` is what flips, which is the ordinary
        // hide-then-reveal an announcement does follow.
        role="status"
        aria-label={hint}
        aria-hidden={badge ? undefined : true}
      >
        {badge}
      </span>
      {view.closable && (
        <button
          type="button"
          className={view.dirty ? `${styles.close} ${styles.closeDirty}` : styles.close}
          title={view.dirty ? 'Close (unsaved changes)' : 'Close tab'}
          // Guarded by the drag for the same reason the label is, and the stakes here are
          // higher: a press that starts on `×` and turns into a reorder must not *also* close
          // the tab it just moved. Pointer capture normally retargets the `click` to the row and
          // this button never hears it — but capture can fail (a pointer released before the
          // threshold, an engine that refuses the call), and the fallback path of a guard that
          // discards work is not a fallback worth having.
          onClick={() => {
            if (dragged()) return
            onClose?.(tab.id)
          }}
        >
          {view.dirty ? '•' : '×'}
        </button>
      )}
    </div>
  )
}

interface TabView {
  /** Metrics class: font, padding and the gap shared with the label button. */
  shape: string | undefined
  body: ReactNode
  closable: boolean
  dirty: boolean
  /** Tooltip text, and the only place the full path is legible once the name is clipped. */
  hint: string
  /** Layout-audit hook, on the kinds the mock states dimensions for. */
  audit: string | undefined
}

function viewFor(kind: TabKind): TabView {
  switch (kind.kind) {
    case 'claudeHome':
      return {
        shape: styles.shapeClaude,
        body: (
          <>
            <span className={styles.swatch} data-audit="consoleSwatch" />
            <span>Claude</span>
          </>
        ),
        closable: false,
        dirty: false,
        hint: 'Claude console (pinned)',
        audit: 'consoleTab',
      }

    case 'claudeFull':
      return {
        shape: styles.shapeClaude,
        body: (
          <>
            <span className={styles.swatch} />
            <span>{kind.title}</span>
          </>
        ),
        closable: true,
        dirty: false,
        hint: kind.title,
        audit: undefined,
      }

    case 'file': {
      const badge = badgeFor(kind.path)
      return {
        shape: styles.shapeFile,
        body: (
          <>
            <span className={`${styles.badge} ${badge.tone}`} data-audit="fileTabBadge">
              {badge.label}
            </span>
            <span>{basename(kind.path)}</span>
          </>
        ),
        closable: true,
        dirty: kind.dirty,
        hint: kind.path,
        audit: 'fileTab',
      }
    }

    case 'diff': {
      // The mock has no diff tab, so it borrows the file shape and takes its badge from the
      // new side — a diff of a .rs file should sit next to that file under the same RS mark.
      const badge = badgeFor(kind.spec.newPath)
      return {
        shape: styles.shapeFile,
        body: (
          <>
            <span className={`${styles.badge} ${badge.tone}`} data-audit="fileTabBadge">
              {badge.label}
            </span>
            <span>{kind.spec.title}</span>
          </>
        ),
        closable: true,
        dirty: false,
        hint: `${kind.spec.oldPath} → ${kind.spec.newPath}`,
        audit: 'fileTab',
      }
    }

    case 'revision': {
      // A file as one commit left it. Borrows the file shape and its badge from the path, for the
      // same reason the diff tab does: `log.rs @ a1b2c3d` should sit next to `log.rs` under the
      // same RS mark rather than inventing a fifth tab silhouette for a read-only buffer.
      //
      // Never dirty, and that is a fact about the tab rather than a default: the bytes come from
      // the object database, a commit is immutable, and there is nothing to save. `title` is
      // built in Rust (`basename @ short oid`) so the strip and the reopen stack cannot disagree
      // about what the tab is called.
      const badge = badgeFor(kind.path)
      return {
        shape: styles.shapeFile,
        body: (
          <>
            <span className={`${styles.badge} ${badge.tone}`} data-audit="fileTabBadge">
              {badge.label}
            </span>
            <span>{kind.title}</span>
          </>
        ),
        closable: true,
        dirty: false,
        hint: `${kind.path} at ${kind.rev}`,
        audit: 'fileTab',
      }
    }

    case 'settings':
      return {
        shape: styles.shapeSettings,
        body: <span>Settings</span>,
        closable: true,
        dirty: false,
        hint: 'Settings',
        audit: undefined,
      }

    default:
      return unhandled(kind)
  }
}

/** Turns a future `TabKind` variant into a compile error here rather than a blank tab. */
function unhandled(kind: never): never {
  throw new Error(`unhandled tab kind: ${JSON.stringify(kind)}`)
}

function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1)
}

/** Label and colour for a path's language, from the mock's own badge table. */
function badgeFor(path: string) {
  const name = basename(path)
  const dot = name.lastIndexOf('.')
  // `> 0` rather than `>= 0`: a leading dot makes a hidden file, not an extension, so
  // `.gitignore` falls through to the neutral marker instead of claiming a GITIGNORE badge.
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : ''

  switch (ext) {
    case 'rs':
      return { label: 'RS', tone: styles.toneAccent }
    case 'ts':
      return { label: 'TS', tone: styles.toneBlue }
    case 'tsx':
      return { label: 'TSX', tone: styles.toneCyan }
    case 'js':
      return { label: 'JS', tone: styles.toneYellow }
    case 'toml':
      return { label: 'TOML', tone: styles.toneGreen }
    case 'lock':
      return { label: 'LOCK', tone: styles.toneFaint }
    case 'md':
      return { label: 'MD', tone: styles.toneDim }
    case 'yml':
    case 'yaml':
      return { label: 'YML', tone: styles.tonePurple }
    case 'json':
      return { label: 'JSON', tone: styles.toneYellow }
    case 'sh':
      return { label: 'SH', tone: styles.toneGreen }
    default:
      return { label: '·', tone: styles.toneFaint }
  }
}
