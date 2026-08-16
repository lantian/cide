/**
 * *New scratch file…* — pick the type, get the file.
 *
 * # Why this is not the 620px `OverlayCard`
 *
 * The same argument `GoToLine.tsx` makes one file over, and it applies here unchanged.
 * `ModalShell`'s card is docked 96px from the top and sized for a virtualized result list with a
 * footer of hints. This is fourteen fixed rows with labels four characters long, and a 620px card
 * holding them reads as a dialog that failed to load.
 *
 * So the geometry is `GoToLine`'s, which is `chrome/Switcher.module.css`'s with a scrim
 * added back. The CSS module is **duplicated** rather than extracted, deliberately: two users
 * is where extraction is a guess and three is where it is a fact, and doing it in the same
 * change as the feature would make a regression in Go to line indistinguishable from a bug in
 * scratches. `ScratchType.module.css`'s header names itself as the trigger for that extraction —
 * and with the filter field added in M15 that file now holds the third copy, so the trigger has
 * fired and the next person here should hoist it.
 *
 * # Why the type is chosen *first*
 *
 * Because the extension is the language. `editor/languages.ts` resolves a grammar and a status
 * bar label by extension and nothing else, so a scratch created first and typed afterwards
 * would mean renaming a file to change its highlighting. Choosing up front is also what IDEA
 * does, and it is what makes the whole gesture two keystrokes: the list opens on the type of
 * the file the user is already looking at (see `defaultScratchType`), so ⇧⌥S then ⏎ makes
 * another one of whatever they have open.
 *
 * # The filter, and the argument it reverses
 *
 * This header used to say there was "nothing worth fuzzy-matching in `Rust`, `Go`, `JSON`", and
 * that the box therefore took the keyboard itself because it had no text field to catch
 * keystrokes with. Both halves are now wrong and both are replaced rather than left standing.
 * The list is fourteen rows and reaching the bottom of it is five Down presses; `sql` is three
 * characters. The ranking behind it is `filterScratchTypes`, which lives beside `SCRATCH_TYPES`
 * for the reason that array's own comment gives — and, just as usefully, so that
 * `check-editor.mjs` can drive it, which nothing inside a component can be.
 *
 * # Where focus goes, and why the spelling matters
 *
 * `useLayoutEffect`, not `useEffect`. `ModalShell.tsx:55` states the reason and it is a bug this
 * project has already shipped once: the overlay is opened by a keystroke, and a frame in which
 * the field is not yet focused is a frame in which the next character the user types goes to
 * whatever had focus before — a terminal, which receives it as shell input. `GoToLine` uses the
 * weaker plain `useEffect`; this follows `ModalShell` instead. `check-scratch.mjs` pins the
 * spelling, because "it is focused, usually" is exactly the kind of thing that reads fine in a
 * review.
 */
import { useLayoutEffect, useMemo, useRef, useState } from 'react'

import { SCRATCH_TYPES, defaultScratchType, filterScratchTypes } from '@/editor/languages'
import { focusedTabPath } from '@/keys/target'
import { useWorkspace } from '@/store/workspace'
import { matchCounter } from './format'
import { isListKey, listAction } from './listKeys'
import styles from './ScratchType.module.css'

export interface ScratchTypeProps {
  onDismiss: () => void
  /** Create a scratch of this extension — `'rs'`, `'json'`. Wired to `fs.scratchNew` by the host. */
  onCreate: (ext: string) => void
}

export function ScratchType({ onDismiss, onCreate }: ScratchTypeProps) {
  /*
   * Read once, on open, and held — the same rule `GoToLine` and `StructurePicker` state. The
   * tab underneath can change while this box is up (a hook finishing, another window activating
   * a project), and a preselection that moved mid-keystroke would create a file of a type the
   * user never looked at.
   *
   * From the workspace mirror directly rather than through a prop, which is the arrangement
   * `BranchPopup` already uses: this overlay is opened from the palette, where the dispatcher
   * runs outside React and has nothing to hand props from.
   */
  const boot = useWorkspace((s) => s.boot)
  const [query, setQuery] = useState('')
  const shown = useMemo(() => filterScratchTypes(query), [query])
  /*
   * The selection is an index into **`shown`**, not into `SCRATCH_TYPES`.
   *
   * That distinction is the whole of what a filter costs. Every one of `at`'s readers below
   * indexes the filtered array, and `defaultScratchType` — which answers an index into the full
   * list — is therefore only consulted while the query is empty, where the two are the same
   * array. Getting this wrong is not a crash: it is Enter creating a file of a type two rows
   * away from the one that is highlighted.
   *
   * The lazy initialiser is what freezes the preselection: `useState(fn)` calls `fn` on the
   * first render and never again, so a later `boot` cannot move it under the user's fingers.
   */
  const [at, setAt] = useState(() => defaultScratchType(focusedTabPath(boot)))
  const field = useRef<HTMLInputElement>(null)
  const rows = useRef<Array<HTMLButtonElement | null>>([])

  /*
   * Layout, not passive — see the module header. A frame with the field unfocused is a frame in
   * which the user's keystrokes go into a terminal.
   */
  useLayoutEffect(() => {
    field.current?.focus()
  }, [])

  // Follow the selection with the scroll, for the case the list outgrows its max-height on a
  // short window. `nearest` and not `center`: this list is fourteen rows and centring it would
  // jump the whole box for a one-row move.
  useLayoutEffect(() => {
    rows.current[at]?.scrollIntoView({ block: 'nearest' })
  }, [at])

  const accept = (index: number) => {
    const type = shown[index]
    if (type === undefined) return
    // Dismiss first, like every other picker: acting while the popup is still up leaves the
    // caret behind a scrim for a frame, and the file this opens wants the keyboard.
    onDismiss()
    onCreate(type.ext)
  }

  /**
   * Reset the highlight to the top on every keystroke.
   *
   * `FilePicker` does the same and states why: keeping the old index would leave the highlight
   * on whatever row happens to land there, which is a *different type*. The one exception is an
   * empty query, where the list is the full one again and the preselection — "another one of
   * what I am looking at" — is the right place to be.
   */
  const retype = (next: string) => {
    setQuery(next)
    setAt(next.trim() === '' ? defaultScratchType(focusedTabPath(boot)) : 0)
  }

  return (
    <div className={styles.scrim} data-audit="scratchScrim" onMouseDown={onDismiss}>
      <div
        className={styles.popup}
        data-audit="scratchPopup"
        role="dialog"
        aria-modal="true"
        aria-label="New scratch file"
        onMouseDown={(ev) => ev.stopPropagation()}
      >
        <div className={styles.head}>
          New scratch file
          {/*
            * Live, not the constant `13 types` it used to be. A filter with a fixed total beside
            * it is a counter that contradicts the list under it the moment anything is typed.
            */}
          <span className={styles.count}>{matchCounter(shown.length, SCRATCH_TYPES.length)}</span>
        </div>
        <div className={styles.filterRow}>
          <input
            ref={field}
            className={styles.input}
            data-audit="scratchFilter"
            aria-label="Filter scratch file types"
            placeholder="Filter…"
            value={query}
            onChange={(ev) => retype(ev.target.value)}
            /*
             * Handled on the field rather than on `document`, exactly as `GoToLine` argues: a
             * window listener would also answer for the terminal underneath and for any other
             * overlay that happens to be open. `listAction` is the shared list vocabulary — the
             * same wrap-around, the same Page Up jump, the same Escape as the pickers — so this
             * cannot drift into a list that feels different from the others.
             *
             * With **no matches** `listAction` answers `{kind:'none'}` for Enter, so the key is
             * not claimed, nothing is created, and the popup stays open with the sentence below
             * explaining why. That falls out of the shared vocabulary rather than being a rule
             * here, which is the point of using it. What it deliberately does *not* do is create
             * a scratch of the typed extension: that is a free-text path into
             * `cide_core::scratch::check_ext` and a separate decision.
             */
            onKeyDown={(ev) => {
              const action = listAction(ev, shown.length, at)
              if (!isListKey(action)) return
              ev.preventDefault()
              ev.stopPropagation()
              if (action.kind === 'select') setAt(action.index)
              else if (action.kind === 'dismiss') onDismiss()
              else if (action.kind === 'accept') accept(at)
            }}
          />
        </div>
        {shown.length === 0 ? (
          // Said out loud. An empty box under a query is otherwise indistinguishable from a
          // popup that failed to load, which is the class of silence this project keeps paying
          // for — and here it is one line of markup.
          <p className={styles.none} data-audit="scratchNoMatch" role="status">
            No type matches ‘{query.trim()}’.
          </p>
        ) : (
          <div className={styles.list} role="listbox" aria-label="Scratch file type">
            {shown.map((type, index) => (
              <button
                key={type.ext}
                ref={(el) => {
                  rows.current[index] = el
                }}
                type="button"
                role="option"
                aria-selected={index === at}
                className={index === at ? `${styles.row} ${styles.rowOn}` : styles.row}
                /*
                 * `onMouseDown` and not `onClick`, and now for a sharper reason than before: the
                 * *field* holds focus, and a `mousedown` on a button moves it before the click
                 * resolves — which would blur the input, and a blur that reached anything would
                 * unmount this overlay from under a half-finished gesture. `preventDefault`
                 * keeps the caret in the field. The pickers all bind the same event.
                 */
                onMouseDown={(ev) => {
                  ev.preventDefault()
                  ev.stopPropagation()
                  accept(index)
                }}
                onMouseEnter={() => setAt(index)}
              >
                <span className={styles.label}>{type.label}</span>
                {/* The extension, because it is what the file will actually be called and what
                    decides the highlighting — `scratch.rs`, not "a Rust file". */}
                <span className={styles.ext}>.{type.ext}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
