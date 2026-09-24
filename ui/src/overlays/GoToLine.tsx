/**
 * Go to line — IDEA's Ctrl+G, as a small centred popup.
 *
 * # Why this is not the 620px `OverlayCard`
 *
 * That shell is a *search* surface: a full-width field, a match counter, a scrolling list and a
 * footer of hints, docked 96px from the top of the window because a list needs the height. This
 * takes one short number and shows at most one sentence, and a 620px card with nine characters in
 * it reads as a dialog that failed to load. `chrome/Switcher.module.css` already made this
 * exact call for the Ctrl+Tab popup — its header says "deliberately *not* the 620px
 * `OverlayCard`" — and the geometry here is that one's, with a scrim added back.
 *
 * The scrim is the one place the two diverge, and it is not decoration. The switcher must not
 * capture pointer events, because it is up for a few hundred milliseconds while a key is held and
 * a drag started before the keystroke has to survive it. This box takes typing and stays until it
 * is answered, so it dims what is behind it and swallows clicks — a click outside dismisses it,
 * which is the same contract every other overlay in the app offers.
 *
 * # Why it does not reuse `@codemirror/search`'s `gotoLine`
 *
 * `searchKeymap` ships one, on `Mod-Alt-g`, and `editor/find.ts` now filters it out. Three
 * reasons, in increasing order of weight: it renders through `showDialog`, which is a `showPanel`
 * with no `top`, so it docks at the *bottom of the pane* rather than the centre of the window;
 * its chrome is `.cm-dialog` over `.cm-panels`, whose colours are literals in CodeMirror's base
 * theme and survive a theme switch — the same objection `find.ts` records for the stock search
 * panel; and, decisively, a CodeMirror binding is invisible to `cide-core::commands`, which makes
 * it bindable-but-unlistable. That is the one state CLAUDE.md's "commands and keys are one
 * registry" rule forbids outright: it could not appear in the palette and `keymap.json` could not
 * rebind it.
 *
 * What was worth taking from it is the *semantics*, and those are taken: `line`, `line:column`,
 * and a clamp rather than an error for a line past the end. The parsing lives in `gotoLineModel.ts`,
 * which imports nothing so `check:picker` can drive every case — including the ones that are only
 * visible as a wrong caret position.
 */
import { Modal } from './ModalShell'
import { PickerFrame } from '@/kit/components/Overlay'
import { Button } from '@/kit/components/Button'
import { TextInput } from '@/kit/components/Field'
import { useEffect, useMemo, useRef, useState } from 'react'

import { focusedCaret } from '@/editor/caretTrack'
import { canGo, resolveGoto, gotoNote, parseGoto } from './gotoLineModel'
import styles from './GoToLine.module.css'

export interface GoToLineProps {
  onDismiss: () => void
  /** Put the caret there. Wired to `requestReveal` by the host. */
  onGoTo: (path: string, line: number, column: number) => void
}

export function GoToLine({ onDismiss, onGoTo }: GoToLineProps) {
  /*
   * Read once, on open, and held. The caret keeps moving while this box is up — the editor
   * underneath still has its claim, and `revealRequest` may deliver into it — and a popup whose
   * idea of "which file" changed mid-typing would jump into whichever buffer happened to answer
   * last. `StructurePicker` freezes it for the same reason.
   */
  const caret = useMemo(() => focusedCaret(), [])
  /*
   * Prefilled with the line the caret is on, and selected below, which is IDEA's behaviour and is
   * the useful default in both directions: typing replaces it outright, and the number itself
   * tells the user where they are starting from without them having to look at the status bar.
   */
  const [text, setText] = useState(() => (caret === null ? '' : String(caret.line)))
  const field = useRef<HTMLInputElement>(null)

  useEffect(() => {
    field.current?.focus()
    field.current?.select()
  }, [])

  const parsed = useMemo(() => parseGoto(text), [text])
  const note = gotoNote(parsed, caret?.lines ?? 1)

  const go = () => {
    // Guarded here as well as by the button's `disabled`, because Enter does not consult a
    // button. `canGo` is the single predicate both of them ask, so they cannot answer differently.
    if (caret === null || !canGo(parsed)) return
    // Dismiss first, like every other picker: acting while the popup is still up leaves the
    // caret behind a scrim for a frame.
    onDismiss()
    // Through `resolveGoto`, which turns a typed `0` into line 1 — the thing the note under
    // the field has already promised. Passing the raw value handed `jump.ts` its own
    // `UNKNOWN_LINE` sentinel and the jump silently did nothing at all.
    const at = resolveGoto(parsed.at)
    onGoTo(caret.path, at.line, at.column)
  }

  return (
    <Modal onDismiss={onDismiss}>
      <PickerFrame
        label="Go to line"
        narrow
        data-audit="gotoPopup"
        /*
         * Escape on the box rather than on `document`: a window listener would also answer for
         * the terminal underneath and for any other overlay that happens to be open. `stopPropagation`
         * so the editor's own Escape — which closes the find bar — does not also fire.
         */
        onKeyDown={(ev) => {
          if (ev.key !== 'Escape') return
          ev.stopPropagation()
          onDismiss()
        }}
      >
        <div className={styles.head}>Go to line</div>
        <div className={styles.row}>
          <div className={styles.field}>
            {/* Mono, because the content is a number the user compares with the line count in
                the note underneath. */}
            <TextInput
              ref={field}
              mono
              data-audit="gotoInput"
              value={text}
              placeholder="line or line:column"
              aria-label="Line number"
              spellCheck={false}
              autoComplete="off"
              onChange={(ev) => setText(ev.target.value)}
              onKeyDown={(ev) => {
                if (ev.key !== 'Enter') return
                ev.preventDefault()
                go()
              }}
            />
          </div>
          <Button variant="primary" data-audit="gotoSubmit" disabled={!canGo(parsed)} onClick={go}>
            Go
          </Button>
        </div>
        {/*
          * One slot, always rendered, and its height reserved in CSS (`min-height`), so the box
          * does not resize as the user types. A popup that grows and shrinks under the pointer is
          * how a click aimed at `Go` lands on nothing. `aria-live` because this note is the only
          * feedback a refusal gives, and a disabled button is silent to a screen reader.
          */}
        <div className={styles.note} data-audit="gotoNote" aria-live="polite">
          {note}
        </div>
      </PickerFrame>
    </Modal>
  )
}
