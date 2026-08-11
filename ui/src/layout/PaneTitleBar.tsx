/**
 * The pane frame and the 26px bar at the top of it: index, centred title, pane actions.
 *
 * The two live in one module because they share a single rule — the focus ring on the
 * frame's border and the lift of the title's colour are the same piece of state, and
 * splitting them across files invites one being updated without the other.
 *
 * The index is computed depth-first by the caller at render time rather than stored.
 * Storing it would mean renumbering every sibling on each split and close, and the number
 * is presentation — it exists so "focus pane 3" has something to refer to.
 *
 * # What changed in M12, and why the old "neither component talks to Rust" is gone
 *
 * This file used to take every action as a callback, so that a detached-pane window could
 * render the same frame from a different workspace slice. Two of the three things added here
 * cannot be done that way:
 *
 * * **The awaiting marker** is driven by a session's *history* of `cide://session-state`
 *   events (see `panes/awaitingRule.ts`), which no caller holds and which would have to be
 *   threaded through `App.tsx` and `DetachedPaneWindow.tsx` to every pane. That is the exact
 *   shape that has already left three controls in this app wired to nothing, so it is read
 *   from the store instead and installs itself on first render.
 * * **The window controls** exist only in a detached-pane window, and `DetachedPaneWindow`
 *   passes this component no props at all. Rather than require an edit there, the bar asks
 *   the URL what kind of window it is in — the same `?window=` parameter every other part of
 *   the app boots from.
 *
 * The pane actions are still callbacks, and still absent-means-hidden, because those *are*
 * the caller's: only `App.tsx` knows the project and tab a pane sits in.
 */
import { useMemo, useRef, type ReactNode } from 'react'
import { useContextMenu, type MenuEntry } from '@/menus'
import { useWindowChrome } from '@/chrome/WindowFrame'
import { windows as windowApi, windowLabel, windowRole, type Pane } from '@/ipc/client'
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
   * because the bar has room for four glyphs and this is the less-used of the two axes.
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
   */
  const seen = () => acknowledge(session)

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
      onPointerDownCapture={() => {
        seen()
        raise?.()
      }}
      onFocusCapture={() => {
        seen()
        raise?.()
      }}
    >
      <PaneTitleBar
        index={index}
        title={pane.title}
        session={session}
        focused={focused}
        maximized={maximized}
        closable={pane.role !== 'primary'}
        // Same role, same answer. `layout::take_pane` refuses the primary for detach exactly
        // as it does for close, so offering a live detach button here would be one gesture
        // in the frame that is guaranteed to fail — and it fails into an error toast, which
        // reads as a bug rather than as a rule.
        detachable={pane.role !== 'primary'}
        onAddTile={onAddTile}
        onSplitDown={onSplitDown}
        onAddRow={onAddRow}
        onMaximize={onMaximize}
        onDetach={onDetach}
        onClose={onClose}
      />
      <div className={styles.body}>{children}</div>
    </div>
  )
}

export interface PaneTitleBarProps {
  index: number
  title: string
  /**
   * The session this pane is showing, for the awaiting marker. Absent — a diff or editor
   * pane, or one whose child has not started — means there is nothing that can wait.
   */
  session?: string | null | undefined
  focused: boolean
  maximized?: boolean | undefined
  /**
   * False for the project console's pane. Withholding the button is a courtesy to the
   * user, not the enforcement — Rust answers `PanePrimary` whether or not we offer it.
   */
  closable?: boolean | undefined
  /**
   * False for the project console's pane, for the same reason as `closable`: detaching goes
   * through the same `take_pane` refusal, so the button would never once succeed.
   */
  detachable?: boolean | undefined
  /**
   * Add a tile beside this pane, in its row. Absent hides the button — a detached-pane
   * window has no row to add to.
   *
   * This bar had no split affordance at all until M11, which is a large part of why the
   * gesture was unfindable: the only one in the app was the header's ⊞, six inches away from
   * the pane it acts on.
   */
  onAddTile?: (() => void) | undefined
  onSplitDown?: (() => void) | undefined
  onAddRow?: (() => void) | undefined
  onMaximize?: (() => void) | undefined
  onDetach?: (() => void) | undefined
  onClose?: (() => void) | undefined
}

export function PaneTitleBar({
  index,
  title,
  session,
  focused,
  maximized = false,
  closable = true,
  detachable = true,
  onAddTile,
  onSplitDown,
  onAddRow,
  onMaximize,
  onDetach,
  onClose,
}: PaneTitleBarProps): ReactNode {
  const awaiting = useAwaiting(session)
  const detachedWindow = useMemo(inDetachedPaneWindow, [])
  const barRef = useRef<HTMLDivElement>(null)
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
   * Reach the window controls through the buttons this bar already draws.
   *
   * Close, minimize and zoom are `WindowFrame`'s: it owns the one delegated `click` listener
   * that turns `data-window-button` into a call on `@tauri-apps/api/window`, and that
   * module is the only one besides `ipc/client.ts` allowed to import it. A menu item that
   * imported `getCurrentWindow` itself would be a third — so the item activates the button
   * instead, and the behaviour stays in exactly one place.
   *
   * The buttons are always rendered when these items are offered, because both are gated on
   * the same `detachedWindow`.
   */
  function fireWindowButton(action: 'minimize' | 'zoom' | 'close'): void {
    barRef.current
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
        disabledReason: onAddRow ? undefined : 'Use the + row strip below the tab',
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

  const { onContextMenu, menu } = useContextMenu({ label: 'Pane', items })

  return (
    <div
      ref={barRef}
      className={styles.bar}
      data-audit="paneTitle"
      data-awaiting={awaiting ? 'true' : 'false'}
      onContextMenu={onContextMenu}
    >
      <span className={styles.index}>{index}</span>
      {/*
       * Always rendered, filled only when it means something.
       *
       * The box is reserved either way, and that is the requirement rather than a nicety: a
       * bar that grows when the marker appears changes the height of the pane body under it,
       * every terminal in the tab refits, and a `claude` mid-turn reflows its output — a
       * notification that costs the thing it is notifying you about.
       */}
      <span
        className={awaiting ? `${styles.marker} ${styles.markerOn}` : styles.marker}
        role={awaiting ? 'status' : undefined}
        aria-label={awaiting ? 'This session is waiting for you' : undefined}
        title={awaiting ? 'This session has finished and is waiting for you' : undefined}
        aria-hidden={awaiting ? undefined : true}
      />
      <span className={focused ? `${styles.title} ${styles.titleFocused}` : styles.title}>
        {title}
      </span>
      <span className={styles.actions}>
        {detachedWindow ? (
          /*
           * A detached-pane window's bar carries WINDOW controls, not pane controls.
           *
           * The user's report: "there is no way to close undocked window, only redock - but
           * it should have same variants of maximize, close, etc". All three of the pane
           * actions were dead here — `DetachedPaneWindow` passes no callbacks, so ⛶, ⧉ and ×
           * were three buttons that did nothing — and `DetachedPaneWindow.module.css` hid
           * them with a rule that says, in as many words, "if that prop is added, delete this
           * rule". Withholding them here is that prop. **That CSS rule now hides these
           * controls too and should be deleted**; the class nesting below outspecifies it in
           * the meantime.
           *
           * `data-window-button` is `WindowFrame`'s contract — see the delegated click
           * handler there, which is what the app header's traffic lights already use.
           */
          <span className={styles.controls}>
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
          </span>
        ) : (
          <>
            {onAddTile && (
              <button
                type="button"
                className={styles.action}
                title="Add a pane to this row"
                aria-label="Add pane to this row"
                onClick={onAddTile}
              >
                ⊞
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
            {/* `aria-disabled`, not `disabled`, for the reason spelled out on the close button. */}
            <button
              type="button"
              className={detachable ? styles.action : `${styles.action} ${styles.actionDisabled}`}
              title={
                detachable ? 'Detach into a window' : 'The project console pane cannot be detached'
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
             * `aria-disabled` rather than `disabled`: a disabled button swallows pointerdown
             * without bubbling, so clicking the close button of the primary pane would be the
             * one click in the frame that fails to focus it.
             */}
            <button
              type="button"
              className={closable ? styles.action : `${styles.action} ${styles.actionDisabled}`}
              title={closable ? 'Close pane' : 'The project console pane cannot be closed'}
              // The reason travels in the name, not only in the tooltip: `title` reaches a
              // mouse pointer, and the one user who cannot see the dimming is the one reading
              // this button through a screen reader.
              aria-label={closable ? 'Close pane' : 'The project console pane cannot be closed'}
              aria-disabled={!closable}
              onClick={closable ? onClose : undefined}
            >
              ×
            </button>
          </>
        )}
      </span>
      {/* Portals to a sibling of `#root`, so the pane's `overflow: hidden` cannot clip it. */}
      {menu}
    </div>
  )
}
