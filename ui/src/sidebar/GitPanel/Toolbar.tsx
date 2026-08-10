/**
 * The commit tool window's icon toolbar: `↻ ↺ ↻ ⤓ ◫ ◉ ⌃ ⌄`.
 *
 * The glyphs are the mock's literal characters — no icon font or SVG set is bundled to
 * substitute for them, exactly as in the activity rail. `↻` appears twice in the mock and
 * that is not a transcription slip: IDEA's toolbar carries both "refresh the changes view"
 * and "update the project from the remote", and they share a circular-arrow icon there
 * too. They are told apart by tooltip, which is why every button here has one and why the
 * tooltip is also the accessible name.
 *
 * `◉` is the one stateful button: IDEA's "use Git staging area instead of changelists"
 * mode (§1). It is a toggle, so it gets `aria-pressed` rather than a click handler that
 * silently flips something invisible.
 */
import styles from './Toolbar.module.css'

export interface ToolbarProps {
  stagingArea: boolean
  /** Disabled when nothing is ticked — every one of these acts on the selection. */
  hasSelection: boolean
  onRefresh: () => void
  onUnstage: () => void
  /**
   * Pull. Optional, and the button is disabled while it is absent: there is no pull in the
   * command surface this panel was built against, and a button labelled "Update project"
   * that quietly re-reads status instead would be a lie about whether the remote was
   * contacted. Disabled-with-a-tooltip is the honest placeholder.
   */
  onUpdate?: (() => void) | undefined
  onShelve: () => void
  onShowDiff: () => void
  onStagingArea: (on: boolean) => void
  onCollapseAll: () => void
  onExpandAll: () => void
}

export function Toolbar(props: ToolbarProps) {
  const { hasSelection } = props
  const items: {
    glyph: string
    label: string
    onClick: () => void
    disabled?: boolean
    pressed?: boolean
  }[] = [
    { glyph: '↻', label: 'Refresh changes', onClick: props.onRefresh },
    {
      /*
       * IDEA's ↺ is Rollback — discard the change entirely. This calls `git_unstage`
       * instead, and the label says so.
       *
       * `git_rollback` does exist, and this button deliberately does not call it: rollback
       * destroys uncommitted work and the handler will not ask first ("a confirmation the
       * backend cannot show is not a safeguard" — `cmd/git.rs`). Wiring a one-click toolbar
       * glyph to it before this panel has a confirmation dialog would make the most
       * destructive command in the surface the easiest one to hit by accident.
       */
      glyph: '↺',
      label: 'Unstage selected changes',
      onClick: props.onUnstage,
      disabled: !hasSelection,
    },
    {
      glyph: '↻',
      label:
        props.onUpdate === undefined ? 'Update project — needs a pull command' : 'Update project',
      onClick: props.onUpdate ?? (() => {}),
      disabled: props.onUpdate === undefined,
    },
    { glyph: '⤓', label: 'Shelve selected changes', onClick: props.onShelve, disabled: !hasSelection },
    { glyph: '◫', label: 'Show diff', onClick: props.onShowDiff, disabled: !hasSelection },
    {
      glyph: '◉',
      label: 'Use Git staging area instead of changelists',
      onClick: () => props.onStagingArea(!props.stagingArea),
      pressed: props.stagingArea,
    },
    { glyph: '⌃', label: 'Collapse all', onClick: props.onCollapseAll },
    { glyph: '⌄', label: 'Expand all', onClick: props.onExpandAll },
  ]

  return (
    <div className={styles.bar} role="toolbar" aria-label="Changes" data-audit="gitToolbar">
      {items.map((item, i) => (
        <button
          // Index is part of the key on purpose: `↻` genuinely appears twice, so the glyph
          // alone is not unique and React would warn about duplicate keys.
          key={`${item.glyph}-${i}`}
          type="button"
          className={styles.button}
          title={item.label}
          aria-label={item.label}
          {...(item.pressed === undefined ? {} : { 'aria-pressed': item.pressed })}
          data-pressed={item.pressed === true ? '' : undefined}
          disabled={item.disabled === true}
          onClick={item.onClick}
        >
          <span aria-hidden="true">{item.glyph}</span>
        </button>
      ))}
    </div>
  )
}
