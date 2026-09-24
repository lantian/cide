/**
 * The commit tool window's icon toolbar.
 *
 * It was the mock's eight literal characters — `↻ ↺ ↻ ⤓ ◫ ◉ ⌃ ⌄` — and this header used to say
 * that no icon font or SVG set was bundled to substitute for them, "exactly as in the activity
 * rail". The rail stopped being an argument for that the moment it drew paths instead, and this
 * toolbar was the worst-off surface left behind: `◫` and `◉` resolve to whatever face fontconfig
 * hands over, `⤓` frequently to nothing at all, and every one of them sat at a different optical
 * weight from the one beside it.
 *
 * **The duplicate `↻` is gone, and it was never really a design.** IDEA's toolbar does carry
 * both "refresh the changes view" and "update the project from the remote" on a circular arrow,
 * and this file faithfully copied that — but the reason two actions could share one mark here
 * was that there was no set to draw a second mark from. There is now: refresh keeps
 * `refresh-cw`, and update takes `arrow-down-to-line`, which is what it does. The tooltips stay,
 * because they are still the accessible name.
 *
 * `circle-dot` is the one stateful button: IDEA's "use Git staging area instead of changelists"
 * mode (§1). It is a toggle, so it gets `aria-pressed` rather than a click handler that
 * silently flips something invisible.
 */
import type { IconName } from '@/icons/Icon'
import { IconButton } from '@/kit/components/Button'
import { Toolbar as KitToolbar } from '@/kit/components/Surface'

export interface ToolbarProps {
  stagingArea: boolean
  /** Disabled when nothing is ticked — every one of these acts on the selection. */
  hasSelection: boolean
  onRefresh: () => void
  onUnstage: () => void
  /**
   * Pull — IDEA's *Update project*.
   *
   * Still optional, and the button still disables itself when it is absent, but the reason has
   * changed. It used to be that *there is no pull in the command surface this panel was built
   * against*, which stopped being true in M10 and left this button dead for two milestones
   * because nothing was ever passed. Now it is absent only where there is no dispatcher to
   * route to — the fixture stories and `check:render`'s SSR pass — and a button that quietly
   * re-read status instead would still be a lie about whether the remote was contacted.
   */
  onUpdate?: (() => void) | undefined
  onShelve: () => void
  onShowDiff: () => void
  onStagingArea: (on: boolean) => void
  onCollapseAll: () => void
  onExpandAll: () => void
  /**
   * New changelist. Absent ⇒ the button is disabled and says why.
   *
   * The panel passes it only when exactly one repository is open, because a changelist lives
   * in one repository's sidecar and the toolbar has no row under it to say which. This is the
   * third route to the same dialog — the others are a group row's menu and the move chooser's
   * own name field — and it is the one that works when there are no changelists yet to
   * right-click.
   */
  onNewChangelist?: (() => void) | undefined
  /** Only to word the disabled reason above. The toolbar draws nothing per repository. */
  repoCount?: number | undefined
}

export function Toolbar(props: ToolbarProps) {
  const { hasSelection } = props
  const items: {
    icon: IconName
    label: string
    onClick: () => void
    disabled?: boolean
    pressed?: boolean
  }[] = [
    { icon: 'refresh-cw', label: 'Refresh changes', onClick: props.onRefresh },
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
      icon: 'undo-2',
      label: 'Unstage selected changes',
      onClick: props.onUnstage,
      disabled: !hasSelection,
    },
    {
      icon: 'arrow-down-to-line',
      label:
        props.onUpdate === undefined
          ? 'Update project — not available in this view'
          : 'Update project',
      onClick: props.onUpdate ?? (() => {}),
      disabled: props.onUpdate === undefined,
    },
    {
      /*
       * Not in the mock's `↻ ↺ ↻ ⤓ ◫ ◉ ⌃ ⌄`, and added anyway: without it a workspace whose
       * repository has only the default changelist offers no way to make a second one, because
       * every other route starts from right-clicking a changelist row. A feature reachable only
       * from something the user has to create first is not reachable.
       */
      icon: 'plus',
      label:
        props.onNewChangelist !== undefined
          ? 'New changelist'
          : (props.repoCount ?? 0) === 0
            ? 'New changelist — no repository is open'
            : `New changelist — right-click a repository row to pick which of the `
              + `${props.repoCount ?? 0} repositories it belongs to`,
      onClick: props.onNewChangelist ?? (() => {}),
      disabled: props.onNewChangelist === undefined,
    },
    { icon: 'archive', label: 'Shelve selected changes', onClick: props.onShelve, disabled: !hasSelection },
    { icon: 'columns-2', label: 'Show diff', onClick: props.onShowDiff, disabled: !hasSelection },
    {
      icon: 'circle-dot',
      label: 'Use Git staging area instead of changelists',
      onClick: () => props.onStagingArea(!props.stagingArea),
      pressed: props.stagingArea,
    },
    { icon: 'chevrons-down-up', label: 'Collapse all', onClick: props.onCollapseAll },
    { icon: 'chevrons-up-down', label: 'Expand all', onClick: props.onExpandAll },
  ]

  // The kit's `Toolbar` of `IconButton`s: hover `--panel-2`, the staging-area toggle "on" as the
  // accent on a 12% wash — the one drawing every icon toggle in the app has.
  return (
    <div data-audit="gitToolbar">
      <KitToolbar label="Changes">
        {items.map((item, i) => (
          <IconButton
            key={`${item.icon}-${i}`}
            icon={item.icon}
            label={item.label}
            pressed={item.pressed}
            disabled={item.disabled === true}
            onClick={item.onClick}
          />
        ))}
      </KitToolbar>
    </div>
  )
}
