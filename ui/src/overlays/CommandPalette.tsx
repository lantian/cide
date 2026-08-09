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
 * Rows are filtered by their `when` clause through the *same* evaluator the key gate uses,
 * so a command the palette offers is exactly a command a key could reach. Showing the rest
 * greyed out was the alternative: it loses because the mock's row has no disabled state, and
 * a list that grows by a third with entries that do nothing is a worse answer to "show all
 * commands" than a list of the ones that work.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ModalShell, Hint } from './ModalShell'
import { searchCommands } from './score'
import { isListKey, listAction } from './listKeys'
import { evaluateWhen } from '@/keys/when'
import type { KeyContext } from '@/keys/keymap'
import type { Keymap } from '@/keys/keymap'
import type { Command } from '@/ipc/client'
import styles from './Overlay.module.css'

const ROW_HEIGHT = 26

export interface CommandPaletteProps {
  /** `Bootstrap.commands`, unfiltered. */
  commands: readonly Command[]
  /** For the key chips. Built from `Bootstrap.keymap`. */
  keymap: Keymap
  /** Context flags, the same object the key gate resolves `when` against. */
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

  const applicable = useMemo(
    () => commands.filter((command) => evaluateWhen(command.when, context)),
    [commands, context],
  )
  const rows = useMemo(() => searchCommands(applicable, query), [applicable, query])

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  })

  useEffect(() => {
    if (rows.length > 0) virtualizer.scrollToIndex(Math.min(selected, rows.length - 1))
  }, [selected, rows.length, virtualizer])

  const accept = useCallback(
    (modifier: 'plain' | 'shift' | 'alt' | 'ctrl') => {
      const row = rows[selected]
      if (!row) return
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
            return (
              <div
                key={row.id}
                className={item.index === selected ? `${styles.row} ${styles.rowSelected}` : styles.row}
                data-audit="paletteRow"
                style={{ height: `${item.size}px`, transform: `translateY(${item.start}px)` }}
                onMouseMove={() => setSelected(item.index)}
                onMouseDown={(ev) => {
                  ev.preventDefault()
                  accept(ev.ctrlKey || ev.metaKey ? 'ctrl' : 'plain')
                }}
              >
                <span className={styles.name}>{row.title}</span>
                <span className={styles.group}>{row.group}</span>
                {/* The chip box is withheld rather than emptied for an unbound command: an
                    empty bordered rectangle reads as a shortcut that failed to render. */}
                {chip !== null && <span className={styles.chip}>{chip}</span>}
              </div>
            )
          })}
        </div>
      )}
    </ModalShell>
  )
}
