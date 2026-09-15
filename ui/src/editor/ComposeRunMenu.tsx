/**
 * The compose gutter's little menu of verbs. (M48)
 *
 * Pure in the strong sense `ChangePopup.tsx` is: every value is a prop, there is no store, no IPC
 * and no clock. `composeGutter.ts` mounts this into the CodeMirror tooltip and supplies the
 * callback; nothing here knows that Docker exists.
 *
 * # Why this is not `useContextMenu`
 *
 * Because there is no React tree here to hang one on. The gutter is a CodeMirror extension and its
 * click handler runs inside a DOM listener, so the menu is a tooltip with a React root mounted
 * into it — the road `changeBars.ts` already built for the change card. `useContextMenu` is a hook
 * and would need a component that outlives the click, which is exactly the state a tooltip has
 * instead.
 */
import type { ReactNode } from 'react'
import styles from './ComposeRunMenu.module.css'

export interface ComposeRunMenuProps {
  /** The heading — the service name, or what to call the whole file. */
  readonly subject: string
  /** The rows, in the order they are drawn. */
  readonly verbs: readonly { readonly id: string; readonly label: string }[]
  readonly onPick: (verb: string) => void
}

export function ComposeRunMenu({ subject, verbs, onPick }: ComposeRunMenuProps): ReactNode {
  return (
    <div className={styles.menu}>
      <div className={styles.head}>{subject}</div>
      {verbs.map((verb) => (
        // `type="button"`. A bare <button> inside anything that is ever a form is a submit
        // button, and this lives in a floating tooltip whose ancestor is not this file's to know.
        <button
          key={verb.id}
          type="button"
          className={styles.row}
          onClick={() => onPick(verb.id)}
        >
          {verb.label}
        </button>
      ))}
    </div>
  )
}
