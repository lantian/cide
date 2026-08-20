/**
 * Shift+Ctrl+P — the command palette.
 *
 * Same 620px shell as the file picker; the differences the mock states are the `>` prompt in
 * `--accent` and the row shape (name, right-aligned group, bordered key chip).
 *
 * The command table arrives whole in `app_get_bootstrap` — a few dozen entries — so matching
 * happens in the window rather than over IPC. `overlays/score.ts` is a port of
 * `cide_core::commands::score` and exists to keep the two orderings the same; see the note
 * there about why the port is preferred to a round trip.
 *
 * Rows are filtered by their `when` clause through the *same* evaluator and the *same*
 * context the key gate uses (`keys/context.ts`), so a command the palette offers is exactly a
 * command a key could reach. That was already the intent and it was quietly false: the
 * context came from `App.tsx` alone, which never set `repoOpen`, so every git row was
 * filtered out of a list titled *Show all commands*.
 *
 * # Two kinds of "you cannot run this", and only one of them hides a row
 *
 * A `when` clause that does not hold means *not here, not now* — focus an editor and the row
 * comes back — so the row is hidden, as before. `Command::unavailable` means *not in this
 * build, whatever you do*, and those rows are drawn greyed with the reason instead. Hiding
 * them was the older, simpler rule and it loses: a user looking for a feature and finding
 * nothing concludes it does not exist, whereas "Restart Claude session — needs a respawn path
 * in the pane host" is the truth and takes one line. It is also the only honest alternative
 * to what shipped, which was ~35 rows that ran into silence.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ModalShell, Hint } from './ModalShell'
import { searchCommands } from './score'
import { isListKey, listAction } from './listKeys'
import { evaluateWhen } from '@/keys/when'
import { useMergedContext } from '@/keys/context'
import type { KeyContext } from '@/keys/keymap'
import type { Keymap } from '@/keys/keymap'
import type { Command } from '@/ipc/client'
import styles from './Overlay.module.css'
import { scaledRow, useUiScale } from '@/settings/useUiScale'

const ROW_HEIGHT = 26

export interface CommandPaletteProps {
  /** `Bootstrap.commands`, unfiltered. */
  commands: readonly Command[]
  /** For the key chips. Built from `Bootstrap.keymap`. */
  keymap: Keymap
  /**
   * The host's own context flags — the overlay and menu state only `App.tsx` knows.
   *
   * Everything derivable from the workspace mirror is merged over it by `useMergedContext`,
   * which is the same merge `useKeyGate` performs, so the palette and the gate cannot end up
   * filtering against two different worlds.
   */
  context: KeyContext
  onDismiss: () => void
  /** ⏎ */
  onRun: (id: string) => void
  /** ⌘⏎ — Ctrl+Enter on Linux. */
  onRunInNewSession: (id: string) => void
}

export function CommandPalette({
  commands,
  keymap,
  context,
  onDismiss,
  onRun,
  onRunInNewSession,
}: CommandPaletteProps) {
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState(0)
  const scrollRef = useRef<HTMLDivElement>(null)

  const merged = useMergedContext(context)
  const applicable = useMemo(
    () =>
      commands.filter(
        // An unavailable command ignores its clause: the clause describes where it *would*
        // apply, and the row is here to explain that it cannot apply anywhere yet. Filtering
        // it by `when` would make the explanation itself context-dependent — *Restart Claude
        // session* would appear only with a Claude pane focused, which is the one state where
        // a user has least reason to go looking for it.
        (command) => command.unavailable !== null || evaluateWhen(command.when, merged),
      ),
    [commands, merged],
  )
  const rows = useMemo(() => searchCommands(applicable, query), [applicable, query])
  // The chrome scale, for the one measurement CSS never sees: a virtualized row is placed by
  // an absolute transform off this number. See `settings/useUiScale.ts`.

  const rowHeight = scaledRow(ROW_HEIGHT, useUiScale())


  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 8,
  })

  useEffect(() => {
    if (rows.length > 0) virtualizer.scrollToIndex(Math.min(selected, rows.length - 1))
  }, [selected, rows.length, virtualizer])

  const accept = useCallback(
    (modifier: 'plain' | 'shift' | 'alt' | 'ctrl') => {
      const row = rows[selected]
      if (!row) return
      // A disabled row swallows ⏎ and stays open, rather than dismissing and running
      // nothing. Dismissing would be indistinguishable from having run something.
      if (row.unavailable !== null) return
      onDismiss()
      if (modifier === 'ctrl') onRunInNewSession(row.id)
      else onRun(row.id)
    },
    [rows, selected, onDismiss, onRun, onRunInNewSession],
  )

  const onKeyDown = useCallback(
    (ev: React.KeyboardEvent<HTMLInputElement>) => {
      const action = listAction(ev, rows.length, selected)
      if (!isListKey(action)) return
      ev.preventDefault()
      if (action.kind === 'select') setSelected(action.index)
      else if (action.kind === 'dismiss') onDismiss()
      else if (action.kind === 'accept') accept(action.modifier)
    },
    [rows.length, selected, onDismiss, accept],
  )

  return (
    <ModalShell
      label="Show all commands"
      prompt=">"
      promptAccent
      value={query}
      placeholder="Type a command name"
      counter={`${rows.length} of ${applicable.length}`}
      onChange={(next) => {
        setQuery(next)
        setSelected(0)
      }}
      onKeyDown={onKeyDown}
      onDismiss={onDismiss}
      scrollRef={scrollRef}
      footer={
        <>
          <Hint keys="↑↓">navigate</Hint>
          <Hint keys="⏎">run</Hint>
          <Hint keys="⌃⏎">run in new session</Hint>
          <Hint keys="esc">dismiss</Hint>
        </>
      }
    >
      {rows.length === 0 ? (
        <div className={styles.status}>No matching commands</div>
      ) : (
        <div className={styles.viewport} style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index]
            if (!row) return null
            const chip = keymap.chipFor(row.id)
            const classes = [styles.row]
            if (item.index === selected) classes.push(styles.rowSelected)
            if (row.unavailable !== null) classes.push(styles.rowDisabled)
            return (
              <div
                key={row.id}
                className={classes.join(' ')}
                data-audit={row.unavailable === null ? 'paletteRow' : 'paletteRowDisabled'}
                // Read out as one sentence — "Restart Claude session, unavailable: …" — so a
                // screen reader gets the reason the greying carries visually.
                aria-disabled={row.unavailable !== null}
                title={row.unavailable ?? undefined}
                style={{ height: `${item.size}px`, transform: `translateY(${item.start}px)` }}
                onMouseMove={() => setSelected(item.index)}
                onMouseDown={(ev) => {
                  ev.preventDefault()
                  accept(ev.ctrlKey || ev.metaKey ? 'ctrl' : 'plain')
                }}
              >
                <span className={styles.name}>{row.title}</span>
                {row.unavailable === null ? (
                  <>
                    <span className={styles.group}>{row.group}</span>
                    {/* The chip box is withheld rather than emptied for an unbound command:
                        an empty bordered rectangle reads as a shortcut that failed to
                        render. Nothing may be bound to an unavailable command at all, so
                        that branch has no chip by construction. */}
                    {chip !== null && <span className={styles.chip}>{chip}</span>}
                  </>
                ) : (
                  <span className={styles.reason}>{row.unavailable}</span>
                )}
              </div>
            )
          })}
        </div>
      )}
    </ModalShell>
  )
}
