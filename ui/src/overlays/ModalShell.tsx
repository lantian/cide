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
    // Click-through to dismiss is on the scrim only; `stopPropagation` on the card keeps a
    // click inside from closing it. A `<div>` rather than `<dialog>`: `showModal()` moves
    // the element into the top layer and takes focus itself, which fights the focus rule
    // above and puts the overlay outside the token-scoped `data-theme` subtree.
    <div className={styles.scrim} data-audit="overlayScrim" onMouseDown={onDismiss}>
      <div
        className={styles.card}
        data-audit="overlayCard"
        role="dialog"
        aria-modal="true"
        aria-label={label}
        onMouseDown={(ev) => ev.stopPropagation()}
      >
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
              className={
                showBlockCaret ? `${styles.input} ${styles.inputHiddenCaret}` : styles.input
              }
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
      </div>
    </div>
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
