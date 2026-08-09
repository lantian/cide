/**
 * *Stack projects in the header* vs *One window per project*.
 *
 * The one setting in this screen that is not a stored preference: choosing it redistributes
 * every open project across OS windows, opening and closing real windows. It goes through
 * `window.setMode` rather than a settings patch — see `SettingsPatch` in the Rust DTOs for
 * why having two paths to the same field would let the desktop and the workspace disagree.
 *
 * That it costs about forty lines of domain code is the dividend of sessions being owned by
 * the process rather than by any window (ADR 0002): both modes are different mappings from
 * `ProjectId` to window label over identical state, and no child notices the change.
 */
import type { WindowMode } from '@/ipc/client'
import styles from './WindowModeCards.module.css'

interface Card {
  value: WindowMode
  label: string
  hint: string
  diagram: 'stacked' | 'perProject'
}

const CARDS: readonly Card[] = [
  {
    value: 'stacked',
    label: 'Stack projects in the header',
    hint: 'Every open project is a tab in one window’s header.',
    diagram: 'stacked',
  },
  {
    value: 'perProject',
    label: 'One window per project',
    hint: 'Each project gets its own window. Sessions are untouched either way.',
    diagram: 'perProject',
  },
]

/** A miniature window: a header strip carrying `tabs` project chips, over a body. */
function MiniWindow({ tabs, activeTab }: { tabs: number; activeTab: number }) {
  return (
    <div className={styles.window}>
      <div className={styles.windowBar}>
        {Array.from({ length: tabs }, (_, i) => (
          <span
            key={i}
            className={i === activeTab ? `${styles.chip} ${styles.chipActive}` : styles.chip}
          />
        ))}
      </div>
      <div className={styles.windowBody} />
    </div>
  )
}

function Diagram({ kind }: { kind: Card['diagram'] }) {
  return (
    <div className={styles.diagram} aria-hidden="true">
      {kind === 'stacked' ? (
        <MiniWindow tabs={3} activeTab={0} />
      ) : (
        // Three windows, each holding one project — the same three projects as the stacked
        // miniature, so the two cards are a picture of the same workspace arranged twice.
        <>
          <MiniWindow tabs={1} activeTab={0} />
          <MiniWindow tabs={1} activeTab={0} />
          <MiniWindow tabs={1} activeTab={0} />
        </>
      )}
    </div>
  )
}

export interface WindowModeCardsProps {
  value: WindowMode
  onChange: (mode: WindowMode) => void
}

export function WindowModeCards({ value, onChange }: WindowModeCardsProps) {
  return (
    <div className={styles.cards} role="radiogroup" aria-label="Window layout">
      {CARDS.map((card) => {
        const active = card.value === value
        return (
          <button
            key={card.value}
            type="button"
            role="radio"
            aria-checked={active}
            className={active ? `${styles.card} ${styles.cardActive}` : styles.card}
            onClick={() => onChange(card.value)}
          >
            <Diagram kind={card.diagram} />
            <div className={styles.cardLabel}>
              <span className={styles.radio}>
                {active && <span className={styles.radioDot} />}
              </span>
              {card.label}
            </div>
            <div className={styles.cardHint}>{card.hint}</div>
          </button>
        )
      })}
    </div>
  )
}
