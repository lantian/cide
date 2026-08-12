/**
 * The pane frame, and the controls that float in its top-right corner.
 *
 * # There is no title bar any more
 *
 * The user's report: *"doesn't like currently header of panels and opened files - it takes a
 * lot of space. And the main idea of it to have buttons to work with split, maximize, etc.
 * Let's instead of a header row move this button to right top corner as floating button
 * without background, they will be inside the context of pane and will be just a buttons
 * without background."*
 *
 * The bar was 26px plus a 1px rule per pane; a 2x3 grid spent 162 vertical pixels on six
 * copies of the same project name. What was in it and where it went:
 *
 * | was                | now                                                                |
 * | ------------------ | ------------------------------------------------------------------ |
 * | index `3`          | in the cluster, revealed with it; always in the frame's `aria-label` |
 * | title `cide : bash`| the same, plus the frame's `aria-label` and the menu's              |
 * | awaiting dot       | in the corner, and the one thing that never hides                   |
 * | ⊞ ⛶ ⧉ ×            | in the cluster, 22px squares, revealed on hover or focus            |
 * | right-click menu   | the whole frame now, not a 26px strip                               |
 *
 * The two components this file used to export are one. `PaneTitleBar` was separate so that a
 * fixture could draw a bar with no pane behind it; nothing ever did, and the split now costs
 * something real — the context menu has to hang off the *frame* (see below) while its items
 * are built from the pane, and threading a hook's return value between two components to
 * achieve that is worse than the duplication it would save.
 *
 * # Why the menu moved to the frame
 *
 * With no bar, a right-click on a 26px strip would have become a right-click on an invisible
 * 126px corner — and that menu is now the *primary* route to split/detach/close for anyone
 * not using the buttons. So the handler is on the frame. It does not fight the pane's
 * contents for the gesture: `useContextMenu`'s handler stops propagation, so any inner
 * surface with a menu of its own (the editor, a tree) shields this one, and `TerminalPane`
 * binds `contextmenu` natively and stops it too — a right-click inside a terminal still gets
 * the terminal's Copy/Paste menu and nothing else. What changes is that a right-click on the
 * parts of a pane that claimed nothing now answers.
 *
 * # What the component reads for itself, and why
 *
 * This used to take every action as a callback so that a detached-pane window could render
 * the same frame from a different workspace slice. Two things cannot be done that way:
 *
 * * **The awaiting marker** is driven by a session's *history* of `cide://session-state`
 *   events (see `panes/awaitingRule.ts`), which no caller holds and which would have to be
 *   threaded through `App.tsx` and `DetachedPaneWindow.tsx` to every pane. That is the exact
 *   shape that has already left three controls in this app wired to nothing, so it is read
 *   from the store instead and installs itself on first render.
 * * **The window controls** exist only in a detached-pane window, and `DetachedPaneWindow`
 *   passes this component no props at all. Rather than require an edit there, the frame asks
 *   the URL what kind of window it is in — the same `?window=` parameter every other part of
 *   the app boots from.
 *
 * The pane actions are still callbacks, and still absent-means-hidden, because those *are*
 * the caller's: only `App.tsx` knows the project and tab a pane sits in.
 */
import { useMemo, useRef, type ReactNode } from 'react'
import { useContextMenu, type MenuEntry } from '@/menus'
import { useWindowChrome } from '@/chrome/WindowFrame'
import { PANE_KINDS } from '@/chrome/RowControls'
import {
  windows as windowApi,
  windowLabel,
  windowRole,
  type Bootstrap,
  type Pane,
  type PaneId,
  type ProjectId,
  type SplitIntent,
  type TabId,
} from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { paneSessionId } from './paneHosts'
import { acknowledge, useAwaiting } from '@/panes/awaiting'
import styles from './PaneTitleBar.module.css'

/**
 * Whether this webview is the window holding one torn-out pane.
 *
 * Read from the URL rather than from the workspace store: it is true for the whole life of
 * the window, it is known before the first bootstrap round trip resolves, and it is what
 * `windows.rs` writes into `?window=` when it builds the window. A `tab:` window is
 * deliberately not included — nothing creates one, and `window_close` refuses it.
 */
function inDetachedPaneWindow(): boolean {
  return windowRole() === 'pane'
}

/**
 * Which project and tab hold a pane, or `null` when nothing in this window does.
 *
 * `pane_split` is addressed by project **and** tab **and** pane, and the frame is handed only
 * the last of the three — so the two other thirds are looked up rather than threaded down. A
 * walk of the snapshot rather than a prop for the same reason `RowControls` re-reads
 * `rowTarget` at click time instead of taking one: the alternative is two more props on this
 * component and at both call sites in `App.tsx` and `DetachedPaneWindow.tsx`, which is the
 * exact arrangement this file's own header blames for three controls being wired to nothing.
 *
 * It is a linear scan and that is fine — it runs once per click, over the tabs of the projects
 * that are open, and each tab's panes are a keyed map so the inner test is a lookup. `focused`
 * is deliberately not consulted: the pane the user right-clicked is the pane the split acts on,
 * whether or not it currently holds focus.
 */
function paneLocation(
  boot: Bootstrap | null,
  pane: PaneId,
): { project: ProjectId; tab: TabId } | null {
  if (boot === null) return null
  for (const project of Object.values(boot.workspace.projects)) {
    for (const tab of project.tabs) {
      if (Object.hasOwn(tab.tree.panes, pane)) return { project: project.id, tab: tab.id }
    }
  }
  return null
}

export interface PaneFrameProps {
  pane: Pane
  /** 1-based depth-first position of this pane in its tab's tree. */
  index: number
  focused: boolean
  maximized: boolean
  children: ReactNode
  onFocus?: (() => void) | undefined
  /** Add a tile beside this pane, in its row. Absent hides the button. */
  onAddTile?: (() => void) | undefined
  /**
   * Split this pane *downwards* — a new row under it. Menu-only; there is no button for it,
   * because the cluster has room for four glyphs before it starts covering the content it
   * floats on, and this is the less-used of the two axes.
   *
   * Absent leaves the menu item present and disabled with a reason, rather than hiding it.
   * A gesture the user has been told about in the release notes and cannot find is worse
   * than one that says why it is unavailable — and this is the prop most likely to be
   * missing, since it needs a line in `App.tsx` that nothing else needs.
   */
  onSplitDown?: (() => void) | undefined
  /** Add a full-width row to the tab. Same treatment as `onSplitDown`. */
  onAddRow?: (() => void) | undefined
  onMaximize?: (() => void) | undefined
  onDetach?: (() => void) | undefined
  onClose?: (() => void) | undefined
}

export function PaneFrame({
  pane,
  index,
  focused,
  maximized,
  children,
  onFocus,
  onAddTile,
  onSplitDown,
  onAddRow,
  onMaximize,
  onDetach,
  onClose,
}: PaneFrameProps): ReactNode {
  // Withheld once this pane is already the focused one. `onFocus` is an IPC round trip
  // that re-reads the tree and re-renders every pane in the tab, and a click inside a
  // terminal the user is already typing in must not cost that — nor must it re-run the
  // effects of a `TerminalPane` whose props are rebuilt by that render.
  const raise = focused ? undefined : onFocus

  // The live binding, not `pane.session`: the domain records the id one round trip after the
  // child starts, and a pane that spawned in this run is bound in the host registry first.
  const session = paneSessionId(pane.id) ?? pane.session

  const awaiting = useAwaiting(session)
  const detachedWindow = useMemo(inDetachedPaneWindow, [])
  const clusterRef = useRef<HTMLDivElement>(null)
  const addTileRef = useRef<HTMLButtonElement>(null)

  /*
   * False for the project console's pane. Withholding the button is a courtesy to the user,
   * not the enforcement — Rust answers `PanePrimary` whether or not we offer it — and detach
   * gets the same answer for the same reason: `layout::take_pane` refuses the primary for
   * detach exactly as it does for close, so a live detach button here would be one gesture in
   * the frame guaranteed to fail, into an error toast that reads as a bug rather than a rule.
   */
  const closable = pane.role !== 'primary'
  const detachable = pane.role !== 'primary'

  /*
   * The *window's* maximized flag, which is a different thing from the pane's.
   *
   * Subscribed unconditionally because hooks must be, and it costs nothing in a shell: the
   * store notifies only when the flag actually flips — `refresh()` compares before it emits —
   * and `WindowFrame` has already attached the one listener behind it. Without this the
   * detached window's zoom item would be labelled from `maximized`, which
   * `DetachedPaneWindow` hardcodes to `false`, so it would read "Maximize window" while the
   * window was maximized.
   */
  const { isMaximized } = useWindowChrome()

  /*
   * Clearing the awaiting marker is an *act*, not a state.
   *
   * `focused` is not enough and would be actively wrong: a detached-pane window renders its
   * one pane as focused unconditionally, so a window sitting behind another would clear its
   * own marker on mount — precisely the case the feature exists for. Pointer-down and
   * focus-in are things the user did.
   *
   * Not folded into `raise` above: that one is withheld once the pane is already focused,
   * and clicking into the pane you are already in is the commonest way of all to say "yes, I
   * have seen this".
   *
   * Two gestures land inside this frame and are emphatically *not* that, so both handlers
   * screen for them:
   *
   * * a **window control**. The cluster of a detached window draws minimize, and minimizing
   *   is the user putting the window away *to come back to it* — clearing the marker there
   *   destroys the one thing that would bring them back, and the `Awaiting: 1` in the task
   *   bar with it, which is the exact case the feature exists for. Close and maximize sit in
   *   the same cluster and read the same way. Screened in the focus handler too, not only in
   *   pointer-down: clicking a button focuses it, so guarding one and not the other guards
   *   nothing.
   * * a **secondary button**. A right-click opens the pane menu rather than reading a
   *   conversation — and that menu is where `Minimize window` is chosen from.
   *
   * `raise` stays outside both screens: focus should follow the pointer either way, and a
   * right-click has to act on the pane it landed in.
   */
  const onWindowControl = (target: EventTarget | null): boolean =>
    target instanceof Element && target.closest('[data-window-button]') !== null

  const seen = () => acknowledge(session)

  /*
   * Reach the window controls through the buttons the cluster already draws.
   *
   * Close, minimize and zoom are `WindowFrame`'s: it owns the one delegated `click` listener
   * that turns `data-window-button` into a call on `@tauri-apps/api/window`, and that
   * module is the only one besides `ipc/client.ts` allowed to import it. A menu item that
   * imported `getCurrentWindow` itself would be a third — so the item activates the button
   * instead, and the behaviour stays in exactly one place.
   *
   * The buttons are always rendered when these items are offered, because both are gated on
   * the same `detachedWindow`, and in that window the cluster is pinned visible — but a
   * `click()` does not need the element on screen either way.
   */
  function fireWindowButton(action: 'minimize' | 'zoom' | 'close'): void {
    clusterRef.current
      ?.querySelector<HTMLButtonElement>(`[data-window-button="${action}"]`)
      ?.click()
  }

  /*
   * Put the pane back in its tab, from this window, with no callback threaded in.
   *
   * `window_redock_pane` takes a label and this window knows its own — so the gesture needs
   * nothing from `DetachedPaneWindow`, which is what lets the item exist at all. The window
   * is destroyed by the command, so there is no local state to update afterwards.
   */
  function redock(): void {
    void windowApi.redockPane(windowLabel()).catch((error: unknown) => {
      console.error('[cide] could not re-dock this pane', error)
    })
  }

  /*
   * `⊞` used to add whichever pane the domain felt like.
   *
   * The report: "'Add a pane to this row' — should have a dropdown to select — claude or bash".
   * The header already asks that question with two labelled buttons (`chrome/RowControls.tsx`),
   * and the two kinds below come from that file so the two surfaces cannot drift.
   *
   * **A menu under one button, not two buttons**, and that is the whole of the interaction
   * decision. The header can afford `⊞ bash row  ⊞ claude row` because it has a whole window's
   * width; this cluster floats over live content and every glyph in it is a glyph over a
   * terminal, so a second unlabelled `⊞` beside the first would be two identical marks meaning
   * different things and 22 more pixels of transcript covered. Two items in the right-click
   * menu was the other candidate and lost on discoverability — the button is the thing the
   * user said they were looking at. The context menu's `Split right` is deliberately left as
   * the one-shot, default-kind version, because it is the line that carries the
   * `pane.split.right` key chip and a keystroke cannot pick a kind.
   */
  const addTile = (intent: SplitIntent): void => {
    const target = paneLocation(useWorkspace.getState().boot, pane.id)
    /*
     * A pane this window's snapshot does not hold: fall back to the host's intent-less
     * handler rather than doing nothing. `App.tsx` supplies one — it is a plain
     * `splitPane(…, 'row', 'after')` with no intent — so the worst case here is the behaviour
     * that shipped, not a dead button.
     */
    if (target === null) {
      onAddTile?.()
      return
    }
    void useWorkspace
      .getState()
      .splitPane(target.project, target.tab, pane.id, 'row', 'after', intent)
  }

  const tileMenu = useContextMenu({
    label: 'Add a pane to this row',
    items: () =>
      PANE_KINDS.map(({ id, word, intent }) => ({
        id,
        // "bash pane" / "claude pane", the header's "bash row" / "claude row" with the one noun
        // that differs swapped. Both words come from the same record, so they cannot drift apart
        // into "shell" here and "bash" there.
        label: `${word} pane`,
        // No `command`: neither kind has a binding of its own — `pane.split.right` splits with
        // the domain's default — and a chip naming it here would promise that this line is what
        // that keystroke does.
        run: () => addTile(intent),
      })),
  })

  const items = (): MenuEntry[] => {
    if (detachedWindow) {
      return [
        {
          // No `command`: `pane.detachToWindow` is the *other* direction, and putting its
          // chip here would show the user a shortcut that tears a pane out beside a line
          // that puts one back.
          id: 'redock',
          label: 'Re-dock into its tab',
          run: redock,
        },
        { kind: 'separator' },
        { id: 'minimize', label: 'Minimize window', run: () => fireWindowButton('minimize') },
        {
          id: 'zoom',
          label: isMaximized ? 'Restore window' : 'Maximize window',
          run: () => fireWindowButton('zoom'),
        },
        { kind: 'separator' },
        {
          // Not `danger`. Closing this window re-docks the pane; nothing is destroyed and
          // the session goes on running, so painting it red would claim a risk that is not
          // there — and this app's red means "this discards something".
          id: 'close-window',
          label: 'Close window',
          run: () => fireWindowButton('close'),
        },
      ]
    }

    return [
      {
        id: 'split-right',
        label: 'Split right',
        command: 'pane.split.right',
        run: onAddTile,
        disabledReason: onAddTile ? undefined : 'This window cannot add a tile to a row',
      },
      {
        id: 'split-down',
        label: 'Split down',
        command: 'pane.split.down',
        run: onSplitDown,
        // Named rather than hidden, and the reason is a route the user can take *today*: the
        // command exists and the palette dispatches it against the focused pane, which a
        // right-click has just made this one. Hiding the line would leave a gesture the
        // release notes promise and the menu denies.
        disabledReason: onSplitDown ? undefined : 'Run “Split pane down” from the palette',
      },
      {
        id: 'add-row',
        label: 'Add row',
        // Same treatment. The row strip at the foot of the tab is the gesture that always
        // works, because it belongs to the tree rather than to any one pane.
        run: onAddRow,
        disabledReason: onAddRow ? undefined : 'Use the row buttons in the header',
      },
      { kind: 'separator' },
      {
        id: 'maximize',
        label: maximized ? 'Restore pane' : 'Maximize pane',
        command: 'pane.maximize',
        checked: maximized,
        run: onMaximize,
      },
      { kind: 'separator' },
      {
        id: 'detach',
        label: 'Detach into a window',
        command: 'pane.detachToWindow',
        run: detachable ? onDetach : undefined,
        disabledReason: detachable ? undefined : 'The project console pane cannot be detached',
      },
      {
        id: 'close',
        label: 'Close pane',
        command: 'pane.close',
        // Red: a pane close ends what is in it. Not for the console pane, which refuses.
        danger: closable,
        run: closable ? onClose : undefined,
        disabledReason: closable ? undefined : 'The project console pane cannot be closed',
      },
    ]
  }

  /*
   * The menu's `label` is its accessible name, and it is where the index and the title are
   * *always* readable — the cluster shows them only while it is revealed, and a right-click is
   * the one gesture that works on every pixel of the pane.
   */
  const { onContextMenu, menu, isOpen: paneMenuOpen } = useContextMenu({
    label: `Pane ${index} — ${pane.title}`,
    items,
  })

  // Pinned open while either menu is up (its trigger must not vanish under it), and for the
  // whole life of a detached window, whose *window* close button cannot be hover-only.
  const revealed =
    detachedWindow || paneMenuOpen || tileMenu.isOpen
      ? `${styles.reveal} ${styles.revealPinned}`
      : styles.reveal

  return (
    // Pointer-down *capture*, so a click that lands inside a terminal focuses the pane
    // before xterm consumes the event. Focus-in capture for the same reason one step out:
    // a pane reached by Tab, or by anything that moves DOM focus without going through the
    // domain, would otherwise take the keystrokes while the ring stayed on another pane.
    // This is not `:focus-within` — the handler asks Rust to move focus and the ring is
    // still drawn only from `tree.focused`, so an overlay taking focus cannot steal it.
    <div
      className={focused ? `${styles.frame} ${styles.frameFocused}` : styles.frame}
      data-audit="pane"
      data-pane-id={pane.id}
      data-focused={focused ? 'true' : 'false'}
      /*
       * The index and the title, permanently, for the reader who gets none of the chrome.
       * `role="group"` is what makes the name reachable at all — `aria-label` on a bare div
       * is dropped — and `group` rather than `region` because six landmarks in a 2x3 grid is
       * a worse table of contents than none.
       */
      role="group"
      aria-label={`Pane ${index}: ${pane.title}`}
      onContextMenu={onContextMenu}
      onPointerDownCapture={(event) => {
        if (event.button === 0 && !onWindowControl(event.target)) seen()
        raise?.()
      }}
      onFocusCapture={(event) => {
        if (!onWindowControl(event.target)) seen()
        raise?.()
      }}
    >
      {/* Before the cluster in DOM order, so Tab reaches the pane's own content first and the
          chrome after it, and so the cluster paints on top without a second z-index. */}
      <div className={styles.body}>{children}</div>

      <div
        ref={clusterRef}
        className={styles.cluster}
        data-audit="paneTitle"
        data-awaiting={awaiting ? 'true' : 'false'}
        /*
         * A click on this cluster must not take DOM focus off whatever the user was typing
         * into.
         *
         * The old buttons were in a bar of their own above the terminal; these sit *on* it,
         * two centimetres from the caret, so mis-aimed and deliberate clicks alike are far
         * more likely to land here mid-keystroke. Preventing the mousedown default leaves
         * focus exactly where it was — the terminal keeps taking keys — while the frame's
         * pointer-down capture above still moves the *domain's* focus, which is the only
         * thing the key gate reads (`App.tsx` builds `keyContext` from `tree.focused`, never
         * from `document.activeElement`). So the ring, the `when` clauses and the typing all
         * agree, which they did not when a click parked focus on a button that answers no
         * keys.
         *
         * Only the primary button, and never `pointerdown`: preventing that one would take
         * the click, and `contextmenu` has to keep arriving.
         */
        onMouseDown={(event) => {
          if (event.button === 0) event.preventDefault()
        }}
      >
        <span className={revealed}>
          {!detachedWindow && (
            <>
              {/*
               * The index is presentation, computed depth-first by the caller at render time
               * rather than stored: storing it would mean renumbering every sibling on each
               * split and close, and the number exists so "focus pane 3" has something to
               * refer to. Nothing dispatches by it — `pane.navigate.{left,right,up,down}` is
               * the whole of the keyboard's pane addressing — which is why it can live behind
               * the reveal rather than costing a permanent mark on the transcript.
               */}
              <span className={styles.index}>{index}</span>
              <span className={focused ? `${styles.title} ${styles.titleFocused}` : styles.title}>
                {pane.title}
              </span>
            </>
          )}

          <span className={detachedWindow ? styles.controls : styles.actions}>
            {detachedWindow ? (
              /*
               * A detached-pane window's cluster carries WINDOW controls, not pane controls.
               *
               * The user's report: "there is no way to close undocked window, only redock -
               * but it should have same variants of maximize, close, etc". All three of the
               * pane actions were dead here — `DetachedPaneWindow` passes no callbacks — and
               * `DetachedPaneWindow.module.css` hid them with a rule that says, in as many
               * words, "if that prop is added, delete this rule". Withholding them here is
               * that prop, so that rule is deleted; it had begun matching these three too.
               *
               * The pane's own index and title are dropped in this window rather than moved:
               * it holds exactly one pane, so the index is always `1`, and
               * `DetachedPaneWindow` already prints the title in its 34px header — a second
               * copy 26 pixels below the first is what "the header takes a lot of space"
               * was about.
               *
               * `data-window-button` is `WindowFrame`'s contract — see the delegated click
               * handler there, which is what the app header's traffic lights already use.
               */
              <>
                <button
                  type="button"
                  className={styles.control}
                  data-window-button="minimize"
                  title="Minimize window"
                  aria-label="Minimize window"
                >
                  ─
                </button>
                <button
                  type="button"
                  className={styles.control}
                  data-window-button="zoom"
                  title={isMaximized ? 'Restore window' : 'Maximize window'}
                  aria-label={isMaximized ? 'Restore window' : 'Maximize window'}
                  aria-pressed={isMaximized}
                >
                  ▢
                </button>
                <button
                  type="button"
                  className={`${styles.control} ${styles.controlClose}`}
                  data-window-button="close"
                  // The tooltip says what closing means here, because it is not what a close
                  // means anywhere else: the session survives and the pane goes home. Losing a
                  // running conversation to a window control would be unrecoverable, so the
                  // gesture is a re-dock and the label says so rather than surprising anyone.
                  title="Close window — the pane returns to its tab and its session keeps running"
                  aria-label="Close window; the pane returns to its tab"
                >
                  ✕
                </button>
              </>
            ) : (
              <>
                {onAddTile && (
                  /*
                   * A menu trigger, not a one-shot. The `▾` is drawn *inside* the same button
                   * rather than beside it as a second control: a split button's two hit
                   * targets would be two more squares over the transcript, and a caret that
                   * opens the same menu the glyph does would be two ways to do one thing.
                   */
                  <button
                    ref={addTileRef}
                    type="button"
                    className={`${styles.action} ${styles.actionMenu}`}
                    title="Add a pane to this row — bash or claude"
                    aria-label="Add a pane to this row"
                    aria-haspopup="menu"
                    aria-expanded={tileMenu.isOpen}
                    onClick={() => {
                      const anchor = addTileRef.current
                      if (anchor !== null) tileMenu.openFor(anchor)
                    }}
                  >
                    ⊞<span className={styles.caret} aria-hidden="true">▾</span>
                  </button>
                )}
                <button
                  type="button"
                  className={maximized ? `${styles.action} ${styles.actionOn}` : styles.action}
                  title={maximized ? 'Restore pane' : 'Maximize pane'}
                  aria-label={maximized ? 'Restore pane' : 'Maximize pane'}
                  aria-pressed={maximized}
                  onClick={onMaximize}
                >
                  ⛶
                </button>
                {/* `aria-disabled`, not `disabled`, for the reason spelled out on the close
                    button. */}
                <button
                  type="button"
                  className={
                    detachable ? styles.action : `${styles.action} ${styles.actionDisabled}`
                  }
                  title={
                    detachable
                      ? 'Detach into a window'
                      : 'The project console pane cannot be detached'
                  }
                  aria-label={
                    detachable
                      ? 'Detach pane into a window'
                      : 'The project console pane cannot be detached'
                  }
                  aria-disabled={!detachable}
                  onClick={detachable ? onDetach : undefined}
                >
                  ⧉
                </button>
                {/*
                 * `aria-disabled` rather than `disabled`: a disabled button swallows
                 * pointerdown without bubbling, so clicking the close button of the primary
                 * pane would be the one click in the frame that fails to focus it.
                 */}
                <button
                  type="button"
                  className={closable ? styles.action : `${styles.action} ${styles.actionDisabled}`}
                  title={closable ? 'Close pane' : 'The project console pane cannot be closed'}
                  // The reason travels in the name, not only in the tooltip: `title` reaches a
                  // mouse pointer, and the one user who cannot see the dimming is the one
                  // reading this button through a screen reader.
                  aria-label={closable ? 'Close pane' : 'The project console pane cannot be closed'}
                  aria-disabled={!closable}
                  onClick={closable ? onClose : undefined}
                >
                  ×
                </button>
              </>
            )}
          </span>
        </span>

        {/*
         * Outside the reveal, and last in the row, so it keeps the corner and never hides:
         * it is the only mark here that reports on a pane the user is *not* looking at. The
         * box is reserved either way — see the stylesheet for why an empty one still takes
         * its 17px.
         */}
        <span
          className={awaiting ? `${styles.marker} ${styles.markerOn}` : styles.marker}
          role={awaiting ? 'status' : undefined}
          aria-label={awaiting ? 'This session is waiting for you' : undefined}
          title={awaiting ? 'This session has finished and is waiting for you' : undefined}
          aria-hidden={awaiting ? undefined : true}
        />
      </div>

      {/* Both portal to a sibling of `#root`, so the pane's `overflow: hidden` cannot clip
          them. Two hooks rather than one because they answer different gestures — a
          right-click anywhere in the frame, and a click on one button — and `useContextMenu`
          holds one open menu each. */}
      {menu}
      {tileMenu.menu}
    </div>
  )
}
