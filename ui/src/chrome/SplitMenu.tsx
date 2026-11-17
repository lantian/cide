/**
 * The header's one split control: a `columns-2` icon that opens a menu of every way to add a pane.
 *
 * # Why one icon
 *
 * The header used to carry three controls for the same family of gesture — `⊞ bash row`,
 * `⊞ harness row` (`RowControls`) and a bare "Split pane" icon — which spent a third of the
 * free header width on labels for something done a few times a session, and made the bare icon
 * a different gesture (a tile of the domain's *default* kind) from the labelled pair beside it.
 * The request was to hide them under the icon. The icon now opens a menu, in two groups by what
 * the new pane runs (bash, harness), each offering both shapes: a **pane beside** the focused one
 * (a tile in its row) and a full-width **row below** it.
 *
 * # Hotkeys come from the keymap, not from here
 *
 * Every item names its command, and `resolveMenu` fills the chip from the same resolved keymap
 * the key gate dispatches from, so a rebound key shows up here with no edit to this file. The
 * four commands are the *kind-specific* ones (`terminal.splitRight`, `claude.split.right`, …),
 * because the generic `pane.split.right` splits with the domain's default and a chip for it on a
 * "bash" line would promise that keystroke makes a shell. (`PaneTitleBar`'s own `⊞` menu leaves
 * `command` off for exactly that reason; here the commands are the per-kind ones, so the chip is
 * true.) A chip is cosmetic: clicking runs the gesture directly through the store.
 *
 * # Why it wires itself
 *
 * Same argument `ProjectMenu` and the old `RowControls` made: a control that moves to the header
 * *and* waits for a prop is a control that ships dead. It reads the target pane at click time so a
 * pane focused since the last paint is the one that splits. `onAddRow` is honoured for the row
 * items when a host passes one — that is how a fixture drives it — but nothing has to.
 *
 * # Works on every tab
 *
 * There is no tab-kind gate. A Settings, OpenSpec or Docker tab has a pane tree like any other
 * (`App.tsx`'s `renderTree` draws the page as that tree's editor-kind pane once it holds a second
 * pane), so a split there is as real as on a terminal tab. `rowGate` refuses only "no project"
 * and "a pane is maximized".
 */
import { useCallback, useRef } from 'react'
import type { SplitIntent } from '@/ipc/generated'
import { useContextMenu, type MenuEntry } from '@/menus'
import { useWorkspace } from '@/store/workspace'
import { rowGate, rowTarget } from './menuModel'
import { Icon } from '@/icons/Icon'

import styles from './AppHeader.module.css'

/**
 * The two things a new pane can be, in the order the menu offers them.
 *
 * **This is the app's one list of that choice.** The pane title bar's `⊞` (`layout/PaneTitleBar.tsx`)
 * offers the same two kinds, and a second literal there would be two lists that agree until
 * somebody edits one. `word` is the noun-less half of a label so a caller can say "bash row" or
 * "bash pane" without this file knowing which surface is asking; `what` is the object of a
 * tooltip's sentence, so both surfaces describe the same intent in the same words.
 *
 * `beside` / `row` are the command ids whose chip each shape shows.
 */
export const PANE_KINDS: ReadonlyArray<{
  id: 'bash' | 'claude'
  word: string
  what: string
  intent: SplitIntent
  beside: string
  row: string
}> = [
  {
    id: 'bash',
    word: 'bash',
    what: 'a shell',
    intent: { kind: 'shell' },
    beside: 'terminal.splitRight',
    row: 'terminal.addRow',
  },
  {
    id: 'claude',
    word: 'harness',
    what: 'a new Harness session',
    intent: { kind: 'newClaude' },
    beside: 'claude.split.right',
    row: 'claude.addRow',
  },
]

export interface SplitMenuProps {
  /** Override for the row items. Absent — the ordinary case — they drive the workspace store. */
  onAddRow?: ((intent: SplitIntent) => void) | undefined
}

export function SplitMenu({ onAddRow }: SplitMenuProps) {
  const reason = useWorkspace((s) => rowGate(s.boot))
  const anchor = useRef<HTMLButtonElement>(null)

  // Re-read at click time rather than closing over a render-time target: the user may have
  // focused a different pane since the menu was built, and a pane that lands next to the one
  // they *were* in is the kind of near-miss nobody reports and everybody works around.
  const beside = useCallback((intent: SplitIntent) => {
    const target = rowTarget(useWorkspace.getState().boot)
    if (target === null) return
    void useWorkspace
      .getState()
      .splitPane(target.project, target.tab, target.after, 'row', 'after', intent)
  }, [])

  const row = useCallback(
    (intent: SplitIntent) => {
      if (onAddRow) {
        onAddRow(intent)
        return
      }
      const target = rowTarget(useWorkspace.getState().boot)
      if (target === null) return
      void useWorkspace
        .getState()
        .addRow(target.project, target.tab, target.after, 'after', intent)
    },
    [onAddRow],
  )

  const { openFor, isOpen, menu } = useContextMenu({
    label: 'Split pane',
    items: (): MenuEntry[] => {
      // Read when the menu opens, not when the header last painted — see `useContextMenu`.
      const why = onAddRow ? null : rowGate(useWorkspace.getState().boot)
      const out: MenuEntry[] = []
      PANE_KINDS.forEach(({ id, word, what, intent, beside: besideCommand, row: rowCommand }, i) => {
        if (i > 0) out.push({ kind: 'separator' })
        // A heading row (`information`) is how this menu draws a group: it is not focusable and
        // keyboard navigation skips it, which a disabled item would not.
        out.push({ id: `${id}-group`, label: word === 'bash' ? 'Bash' : 'Harness', information: true })
        out.push({
          id: `${id}-beside`,
          label: `${word} pane beside`,
          command: besideCommand,
          tooltip: `Split the focused pane: add ${what} in a tile beside it, in the same row`,
          disabledReason: why ?? undefined,
          run: () => beside(intent),
        })
        out.push({
          id: `${id}-row`,
          label: `${word} row below`,
          command: rowCommand,
          tooltip: `Add a full-width row holding ${what}, under the focused pane`,
          disabledReason: why ?? undefined,
          run: () => row(intent),
        })
      })
      return out
    },
  })

  return (
    // A fragment, not the menu inside the button: React events bubble through portals, so a
    // click on a menu row would reach the button's `onClick` and reopen what it just closed.
    <>
      <button
        ref={anchor}
        type="button"
        className={`${styles.action} ${styles.actionSplit}`}
        data-audit="headerSplit"
        title={reason ?? 'Split — add a bash or harness pane or row'}
        aria-label="Split pane"
        aria-haspopup="menu"
        aria-expanded={isOpen}
        disabled={reason !== null && !onAddRow}
        onClick={() => {
          if (anchor.current) openFor(anchor.current)
        }}
      >
        <Icon name="columns-2" size={1} />
      </button>
      {menu}
    </>
  )
}
