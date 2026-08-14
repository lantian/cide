/**
 * *New scratch file…* — pick the type, get the file.
 *
 * # Why this is not the 620px `OverlayCard`
 *
 * The same argument `GoToLine.tsx` makes one file over, and it applies here unchanged.
 * `ModalShell`'s card is a *search* surface: a full-width query field, a match counter, a
 * virtualized scroll container and a footer of hints, docked 96px from the top because a result
 * list needs the height. This is thirteen fixed rows with labels four characters long, and
 * there is nothing worth fuzzy-matching in `Rust`, `Go`, `JSON`. A 620px card holding them
 * reads as a dialog that failed to load.
 *
 * So the geometry is `GoToLine`'s, which is `chrome/ProjectSwitcher.module.css`'s with a scrim
 * added back. The CSS module is **duplicated** rather than extracted, deliberately: two users
 * is where extraction is a guess and three is where it is a fact, and doing it in the same
 * change as the feature would make a regression in Go to line indistinguishable from a bug in
 * scratches. `ScratchType.module.css`'s header names itself as the trigger for that extraction.
 *
 * # Why the type is chosen *first*
 *
 * Because the extension is the language. `editor/languages.ts` resolves a grammar and a status
 * bar label by extension and nothing else, so a scratch created first and typed afterwards
 * would mean renaming a file to change its highlighting. Choosing up front is also what IDEA
 * does, and it is what makes the whole gesture two keystrokes: the list opens on the type of
 * the file the user is already looking at (see `defaultScratchType`), so ⇧⌥S then ⏎ makes
 * another one of whatever they have open.
 */
import { useEffect, useMemo, useRef, useState } from 'react'

import { SCRATCH_TYPES, defaultScratchType } from '@/editor/languages'
import { focusedTabPath } from '@/keys/target'
import { useWorkspace } from '@/store/workspace'
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
  // The lazy initialiser is what freezes it: `useState(fn)` calls `fn` on the first render and
  // never again, so a later `boot` cannot move the selection under the user's fingers.
  const [at, setAt] = useState(() => defaultScratchType(focusedTabPath(boot)))
  const box = useRef<HTMLDivElement>(null)
  const rows = useRef<Array<HTMLButtonElement | null>>([])

  /*
   * Focus goes to the box, not to a row, and it is what makes the arrows work at all: this
   * overlay has no text field to catch keystrokes the way every other picker does, so without
   * a focused element here the keys would go to whatever had them before — a terminal, which
   * would receive `ESC S` from the very chord that opened this.
   */
  useEffect(() => {
    box.current?.focus()
  }, [])

  // Follow the selection with the scroll, for the case the list outgrows its max-height on a
  // short window. `nearest` and not `center`: this list is thirteen rows and centring it would
  // jump the whole box for a one-row move.
  useEffect(() => {
    rows.current[at]?.scrollIntoView({ block: 'nearest' })
  }, [at])

  const accept = (index: number) => {
    const type = SCRATCH_TYPES[index]
    if (type === undefined) return
    // Dismiss first, like every other picker: acting while the popup is still up leaves the
    // caret behind a scrim for a frame, and the file this opens wants the keyboard.
    onDismiss()
    onCreate(type.ext)
  }

  /** `13 types`, in the head. Constant, so it is memoised into one string rather than rebuilt. */
  const total = useMemo(() => `${SCRATCH_TYPES.length} types`, [])

  return (
    <div className={styles.scrim} data-audit="scratchScrim" onMouseDown={onDismiss}>
      <div
        ref={box}
        className={styles.popup}
        data-audit="scratchPopup"
        role="dialog"
        aria-modal="true"
        aria-label="New scratch file"
        tabIndex={-1}
        onMouseDown={(ev) => ev.stopPropagation()}
        /*
         * Handled on the box rather than on `document`, exactly as `GoToLine` argues: a window
         * listener would also answer for the terminal underneath and for any other overlay that
         * happens to be open. `listAction` is the shared list vocabulary — the same wrap-around,
         * the same Page Up jump, the same Escape as the pickers — so this cannot drift into a
         * list that feels different from the others.
         */
        onKeyDown={(ev) => {
          const action = listAction(ev, SCRATCH_TYPES.length, at)
          if (!isListKey(action)) return
          ev.preventDefault()
          ev.stopPropagation()
          if (action.kind === 'select') setAt(action.index)
          else if (action.kind === 'dismiss') onDismiss()
          else if (action.kind === 'accept') accept(at)
        }}
      >
        <div className={styles.head}>
          New scratch file
          <span className={styles.count}>{total}</span>
        </div>
        <div className={styles.list} role="listbox" aria-label="Scratch file type">
          {SCRATCH_TYPES.map((type, index) => (
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
               * `onMouseDown` and not `onClick`: the box holds focus, and a `mousedown` on a
               * button moves it before the click resolves — which unmounts this overlay from
               * under a half-finished gesture on the slow path. The pickers all bind the same
               * event for the same reason.
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
      </div>
    </div>
  )
}
