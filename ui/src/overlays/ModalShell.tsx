/**
 * The 620px modal shell — one component, two uses.
 *
 * It owns the scrim, the card, the input row with its counter, the scroll container the
 * virtualizer measures, and the footer. It owns no *rows*: the file picker and the command
 * palette render their own, which is the whole reason `@tanstack/react-virtual` is the
 * virtualizer here. It is headless, so the exact row markup the mock states survives —
 * a component library that renders the rows for us would not.
 */
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import styles from './Overlay.module.css'

export interface ModalShellProps {
  /** Accessible name for the dialog, e.g. `Go to file`. */
  label: string
  /** `›` for the picker, `>` for the palette. */
  prompt: string
  /** The palette's `>` is drawn in `--accent`; the picker's `›` is not. */
  promptAccent?: boolean | undefined
  value: string
  placeholder?: string | undefined
  /** Right-aligned mono text, e.g. `6 of 2,418`. */
  counter?: string | undefined
  onChange: (value: string) => void
  /** Key handling for the list: arrows, Enter, Escape. Runs on the input. */
  onKeyDown: (ev: React.KeyboardEvent<HTMLInputElement>) => void
  onDismiss: () => void
  /** The scroll container's contents — the virtualizer's spacer and rows. */
  children: ReactNode
  /** A ref the caller hands to `useVirtualizer`'s `getScrollElement`. */
  scrollRef: React.RefObject<HTMLDivElement | null>
  footer: ReactNode
}

export function ModalShell({
  label,
  prompt,
  promptAccent,
  value,
  placeholder,
  counter,
  onChange,
  onKeyDown,
  onDismiss,
  children,
  scrollRef,
  footer,
}: ModalShellProps) {
  const input = useRef<HTMLInputElement>(null)
  const [caretAtEnd, setCaretAtEnd] = useState(true)

  // Layout, not passive: the overlay is opened by a keystroke, and a frame in which the
  // field is not yet focused is a frame in which the next character the user types goes to
  // whatever had focus before — a terminal, which would receive it as shell input.
  useLayoutEffect(() => {
    input.current?.focus()
  }, [])

  /*
   * The block caret is drawn only when the insertion point is at the end of the value.
   *
   * It is opaque, so anywhere else it would paint over the character under it. Falling back
   * to the native thin caret mid-string is a visible inconsistency the mock does not show,
   * but the mock only ever draws the caret at the end — and hiding a character the user is
   * editing is a worse trade than a caret that changes shape when they arrow left.
   */
  useEffect(() => {
    const el = input.current
    if (!el) return
    const sync = () => setCaretAtEnd(el.selectionStart === el.value.length)
    sync()
    // `selectionchange` on the document is the only event that fires for *every* way the
    // caret can move — arrows, click, drag, Home/End, undo. Listening to keyup and click
    // separately misses drag-selection and leaves a block caret drawn over selected text.
    document.addEventListener('selectionchange', sync)
    return () => document.removeEventListener('selectionchange', sync)
  }, [])

  const showBlockCaret = caretAtEnd

  return (
    <OverlayCard label={label} onDismiss={onDismiss}>
      <div className={styles.inputRow}>
        <span
          className={promptAccent === true ? `${styles.prompt} ${styles.promptAccent}` : styles.prompt}
          aria-hidden="true"
        >
          {prompt}
        </span>
        <div className={styles.field}>
          <input
            ref={input}
            className={showBlockCaret ? `${styles.input} ${styles.inputHiddenCaret}` : styles.input}
            data-audit="overlayInput"
            value={value}
            placeholder={placeholder}
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            aria-label={label}
            onChange={(ev) => onChange(ev.target.value)}
            onKeyDown={onKeyDown}
          />
          {showBlockCaret && (
            // `ch` is exact here because the field is monospaced; see Overlay.module.css.
            <span
              className={styles.caret}
              style={{ left: `${value.length}ch` }}
              aria-hidden="true"
            />
          )}
        </div>
        {counter !== undefined && (
          <span className={styles.counter} data-audit="overlayCounter">
            {counter}
          </span>
        )}
      </div>

      <div className={styles.list} ref={scrollRef} data-audit="overlayList">
        {children}
      </div>

      <div className={styles.footer} data-audit="overlayFooter">
        {footer}
      </div>
    </OverlayCard>
  )
}

/**
 * The scrim and the 620px card, without an opinion about what goes inside them.
 *
 * Extracted from `ModalShell` when the close confirmation arrived: that dialog wants the
 * same ground, width, radius, shadow and scrim, and none of the search apparatus above —
 * no input, no counter, no scroll container for a virtualizer. Copying the two elements
 * would have been three lines and a second definition of what an overlay looks like in this
 * app, which is how the file picker and the confirmation end up 4px apart after someone
 * adjusts one of them.
 *
 * # It portals to `document.body`, and that is not decoration
 *
 * Reported as "resizer line of git panel is above agent modal", and the numbers say it should
 * not be: this scrim is `z-index: 60` and the sidebar splitter is `z-index: 1`. `z-index` is
 * only ever compared *within one stacking context*, and `layout/TabContent.module.css` makes
 * every tab panel one — `.panel { position: absolute; inset: 0; z-index: 0 }`, `.panelActive
 * { z-index: 1 }`. A card rendered where it is written from inside a tab therefore resolves
 * its 60 against its siblings inside that panel and is capped at the panel's own level, so it
 * paints under anything that is a sibling of the panel. No number fixes that; raising 60 to
 * 600 only makes the bug depend on which context happens to be topmost where you tested.
 *
 * The overlays that never had the problem — the command palette, the file picker — are
 * mounted by `App.tsx` at top level and were already in the root context. So the portal is
 * what makes "mounted at App level" and "mounted inside a tab, a pane or a scroller" the same
 * thing, which is the whole reason it lives here and not at the one call site that reported
 * it. Three more failure modes come free, and each has already bitten something in this repo:
 * `position: fixed` resolves against the nearest ancestor with a `transform`, `filter` or
 * `contain` rather than the viewport; `overflow: hidden` on a pane clips a non-fixed child;
 * and `TabContent` hides an inactive tab with `visibility: hidden`, which would leave a
 * still-mounted dialog invisible and still eating keystrokes. `settings/KeymapSection.tsx`
 * and `menus/ContextMenu.tsx` portal for exactly these reasons and say so.
 *
 * **The theme survives**, and that was checked rather than assumed: `public/theme-boot.js`
 * writes `document.documentElement.dataset.theme`, `styles/tokens.css` defines the palette on
 * `:root` / `[data-theme='…']`, and `settings/useSettings.ts`'s `paintTheme` writes the same
 * attribute on `<html>`. `document.body` is inside that subtree, so every `var(--…)` in
 * `Overlay.module.css` resolves exactly as before. Portalling to anything *above* `<html>`
 * — the top layer via `<dialog>.showModal()`, say — is the case that would need thought.
 *
 * **Bubbling survives too.** React dispatches synthetic events through the *React* tree, not
 * the DOM tree, so a parent component's `onKeyDown`/`onClick` around an `<OverlayCard>` still
 * sees events from inside it. The two window-level listeners this app has are unaffected for
 * their own reasons: `keys/gate.ts` is `window.addEventListener('keydown', …, true)` —
 * capture, so it runs before the target either way — and `chrome/WindowFrame.tsx`'s document
 * listeners look for `[data-window-drag="true"]` / `[data-window-button]`, which appear only
 * on leaf nodes in the header and were never ancestors of a card.
 */
export function OverlayCard({
  label,
  onDismiss,
  children,
}: {
  label: string
  onDismiss: () => void
  children: ReactNode
}) {
  return createPortal(
    // Click-through to dismiss is on the scrim only; `stopPropagation` on the card keeps a
    // click inside from closing it. A `<div>` rather than `<dialog>`: `showModal()` moves the
    // element into the top layer and takes focus itself, which fights the focus rule in
    // `ModalShell` — this component's callers all decide for themselves where focus lands, and
    // one of them (`settings/AgentsSection.tsx`) has a reason it must not be the first control.
    <div className={styles.scrim} data-audit="overlayScrim" onMouseDown={onDismiss}>
      <div
        className={styles.card}
        data-audit="overlayCard"
        role="dialog"
        aria-modal="true"
        aria-label={label}
        onMouseDown={(ev) => ev.stopPropagation()}
      >
        {children}
      </div>
    </div>,
    // No SSR guard, deliberately, and the same call `menus/ContextMenu.tsx` and
    // `settings/KeymapSection.tsx` make. Nothing in `ui/scripts/check-*-render.mjs` renders a
    // card — every smoke entry was checked — and a future one that did would fail loudly on
    // `document is not defined`, which is a better answer than a fallback branch that quietly
    // renders the dialog back into the capped context this portal exists to escape.
    document.body,
  )
}

/** One `⏎ open`-style footer hint. The glyph is mono and dim; the words are not. */
export function Hint({ keys, children }: { keys: string; children: ReactNode }) {
  return (
    <span>
      <span className={styles.footerKey}>{keys}</span>
      {children}
    </span>
  )
}
