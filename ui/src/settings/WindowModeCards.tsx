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
import { ChoiceCards } from '@/kit/components/Choice'
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
  // The kit's `ChoiceCards`: one of two options that each need a picture — the wizard's project
  // type, drawn the same way, so a choice with art looks like one choice everywhere.
  return (
    <ChoiceCards
      label="Window layout"
      value={value}
      onChange={onChange}
      options={CARDS.map((card) => ({
        value: card.value,
        title: card.label,
        text: card.hint,
        art: <Diagram kind={card.diagram} />,
      }))}
    />
  )
}
