/**
 * The OpenSpec panel's view — pure, and the only thing it is given is props. (M28)
 *
 * No store, no IPC, no clock. Everything it needs arrives as a prop, which is what lets
 * `check:openspec-render` SSR it under node with fixed fixtures and compare a digest byte for
 * byte. `OpenSpecPanelHost` is where the store and the commands live.
 *
 * # The brand is shown, not hidden
 *
 * Every screen here names OpenSpec, names the real directories (`openspec/specs/`,
 * `openspec/changes/`), and says what a button will run. That is deliberate: the format is an
 * open standard read by thirty other tools, so everything a user learns from these strings
 * transfers to the CLI, to the docs, and to a teammate's editor. A cide-flavoured vocabulary
 * would have to be translated at every one of those boundaries.
 */
import { Icon, asIcon } from '@/icons/Icon'
import styles from './OpenSpecPanel.module.css'
import {
  ABSENT_CLAIM,
  CLI_INSTALL,
  CLI_MISSING_CLAIM,
  EXPLORE_COMMAND,
  EXPLORE_LABEL,
  PROPOSE_COMMAND,
  PROPOSE_LABEL,
  RETRY_LABEL,
  SETUP_BUSY_LABEL,
  SETUP_LABEL,
  SETUP_TITLE,
  CHANGE_ICON,
  CONFIGURE_LABEL,
  CONFIGURE_TITLE,
  commandTitle,
  metaFigure,
  rowAction,
  type TrackerKind,
  specRows,
  stageLabel,
  type Board,
  type ChangeStage,
  type SpecRow,
} from './model'

export interface OpenSpecPanelViewProps {
  board: Board
  /** Which sections are open. Absent means open — a tree that starts shut shows nothing. */
  expanded: Readonly<Record<string, boolean>>
  busy: boolean
  onToggleSection: (id: string) => void
  onOpenChange: (name: string) => void
  /** Which task tracks which change, so a row can offer the right action. */
  tasks: Readonly<Record<string, string>>
  /**
   * What the *task* tracker has said — `unknown` until it has. (M28)
   *
   * Threaded in rather than derived from `tasks`, because an empty map has two meanings and the
   * row has to draw them differently: no tasks, or nobody has looked. `rowAction`'s doc has the
   * bug that taught us.
   */
  tracker: TrackerKind
  /** Start work on a change (creating its task), or open the task it already has. */
  onRowAction: (change: string, action: 'start' | 'open') => void
  onOpenSpec: (id: string) => void
  onSetUp: () => void
  /**
   * Which command the composer is open on, or `null`. (M28)
   *
   * The composer itself is `ProposeDialog.tsx` and is rendered by `App.tsx`, not here — see the
   * note where the box used to be. This view keeps the value only so the two buttons can carry
   * `aria-expanded`, which is what tells a screen reader that pressing one opened something.
   */
  asking: string | null
  /** Open the composer on a command. The whole of what this view knows about it. */
  onAsk: (command: string | null) => void
  onRetry: () => void
  /**
   * From the host's `useContextMenu`. Absent in a render with no window to open one in — which
   * is every SSR story, so the menu never has to be faked to render the panel. (M28)
   */
  onContextMenu?: ((event: React.MouseEvent) => void) | undefined
  /** The portalled menu itself. It must be rendered or nothing appears. */
  menu?: React.ReactNode
  /**
   * Open this project's `openspec/config.yaml`. (M28)
   *
   * The gear lives on **this** header and not under Settings, where it shipped. cide's Settings
   * is global — nine of its eleven sections write `Settings`, which follows a person into every
   * repository they open — and this is one committed YAML file in one project. Putting it in that
   * list said it was a preference; putting it here says what it is, by where the control is
   * rather than by a paragraph under a heading that contradicts it.
   *
   * In the header rather than in the tree, which is the distinction that answers the objection
   * the old placement raised against a panel modal: this panel is a *board*, and a row is not a
   * surface anybody opened in order to edit a committed file. A panel header is where a panel's
   * own affordances live, and nothing about a change row offers this.
   *
   * `undefined` withholds the gear entirely — the render check's `board-unknown` story is what
   * pins that, because a control for configuring OpenSpec on a project nobody has looked at yet
   * is a control that cannot know what it would open.
   */
  onConfigure?: (() => void) | undefined
}

const STAGE_CLASS: Readonly<Record<ChangeStage, string>> = {
  proposed: '',
  inProgress: styles.stageInProgress ?? '',
  ready: styles.stageReady ?? '',
  invalid: styles.stageInvalid ?? '',
  // Unreachable from a *row* — `openspec list` does not list an archived change, so no summary
  // ever carries this stage — and present because the record is `Record<ChangeStage, …>` and a
  // missing key would be `undefined` in a `className`, which stringifies. See `specRows`.
  archived: '',
}

/** Joins class names, dropping everything falsy. Never a template string — see `check:openspec-render`. */
function cx(...parts: readonly (string | false | null | undefined)[]): string {
  return parts.filter((part): part is string => typeof part === 'string' && part !== '').join(' ')
}

export function OpenSpecPanelView(props: OpenSpecPanelViewProps) {
  const { board } = props
  const figure = metaFigure(board)
  return (
    <div className={styles.panel} data-audit="openspecPanel">
      <div className={styles.head} data-audit="openspecHead">
        <span className={styles.title}>OpenSpec</span>
        {figure !== null && (
          <span className={styles.meta} data-audit="openspecMeta">
            {figure}
          </span>
        )}
        {/*
          * Last in the header, hard against the right edge — `TasksPanel`'s own header control
          * sits the same way, and a header whose actions were on the left would put them where
          * every panel in this app puts its title.
          */}
        {props.onConfigure !== undefined && (
          <button
            type="button"
            className={styles.headAction}
            data-audit="openspecConfigure"
            title={CONFIGURE_TITLE}
            onClick={props.onConfigure}
          >
            <Icon name={asIcon('settings')} size={1} label={CONFIGURE_LABEL} />
          </button>
        )}
      </div>
      {/*
        * The right-click surface is the body, not each row.
        *
        * One handler rather than one per row: `useContextMenu` hands the item builder the element
        * under the pointer, so the host resolves which row was hit from `data-change`/`data-spec`
        * on the way up. A right-click on the empty space below the last row therefore opens
        * nothing — an empty box at the pointer reads as a broken surface rather than as one with
        * nothing to offer, which is `ChangesTree`'s rule.
        */}
      <div
        className={styles.body}
        data-audit="openspecBody"
        onContextMenu={props.onContextMenu}
      >
        {body(props)}
      </div>
      {props.menu}
    </div>
  )
}

function body(props: OpenSpecPanelViewProps) {
  const { board } = props

  // Nobody has asked yet. Deliberately nothing at all — not the pitch card, whose button writes a
  // tracked directory into the repository, and not an empty tree, which would claim to have
  // looked. See `Board`'s doc.
  if (board.kind === 'unknown') return null

  if (board.kind === 'absent') {
    return (
      <div className={styles.screen} data-audit="openspecAbsent">
        <p className={styles.claim} data-audit="openspecClaim">
          {ABSENT_CLAIM}
        </p>
        <p className={styles.detail} data-audit="openspecDetail">
          {board.hint}
        </p>
        {/*
         * The path *before* the button, and this ordering is the point: Set up writes a folder
         * that the user's repository will then contain and that a teammate will see in the next
         * pull request. A control that commits something has to say where, above itself, where it
         * is read before the click rather than after it.
         */}
        <p className={styles.path} data-audit="openspecPath">
          {board.path}
        </p>
        <div className={styles.actions}>
          <button
            type="button"
            className={cx(styles.button, styles.primary)}
            data-audit="openspecSetUp"
            data-write="true"
            title={SETUP_TITLE}
            disabled={props.busy}
            data-busy={props.busy ? 'true' : undefined}
            aria-busy={props.busy ? true : undefined}
            onClick={props.onSetUp}
          >
            {/*
              * A mark and a present participle while init runs, never only `disabled` — the
              * task card's rule (`busyLabel`), for its reason: init is two node subprocesses
              * and takes seconds, and a slightly dimmer label was read as a missed click. The
              * mark **turns** — see `.busyMark`. It was static, which is a shape rather than a
              * signal, and that was reported against the task card's copy of it.
              */}
            {props.busy && (
              <span className={styles.busyMark} aria-hidden="true">
                <Icon name={asIcon('loader-circle')} size={0} />
              </span>
            )}
            {props.busy ? SETUP_BUSY_LABEL : SETUP_LABEL}
          </button>
        </div>
      </div>
    )
  }

  if (board.kind === 'unusable') {
    return (
      <div className={styles.screen} data-audit="openspecUnusable">
        <p className={styles.claim} data-audit="openspecClaim">
          {CLI_MISSING_CLAIM}
        </p>
        {/*
         * The reason comes from Rust, which is the only layer that can tell a missing binary from
         * a refusal from a root that belongs to a parent repository — and it carries the
         * launcher-PATH sentence, which is the whole answer to "but I have it installed".
         */}
        <p className={styles.detail} data-audit="openspecDetail">
          {board.reason}
        </p>
        <p className={styles.path} data-audit="openspecInstall">
          {CLI_INSTALL}
        </p>
        <div className={styles.actions}>
          <button
            type="button"
            className={styles.button}
            data-audit="openspecRetry"
            data-write="true"
            onClick={props.onRetry}
          >
            {RETRY_LABEL}
          </button>
        </div>
      </div>
    )
  }

  const rows = specRows(board, props.expanded, props.tasks)

  return (
    <>
      {/*
        * The two things to do, above everything and always — not tucked into an empty state.
        *
        * They lived inside a "no changes yet" screen, so they vanished the moment the first
        * change appeared: the panel offered its actions exactly until the user had reason to use
        * them again. Proposing a second change is the ordinary case, not the exceptional one.
        */}
      <div className={styles.toolbar} data-audit="openspecToolbar">
        <button
          type="button"
          className={cx(styles.button, styles.primary)}
          data-audit="openspecPropose"
          data-command={PROPOSE_COMMAND}
          data-write="true"
          title={commandTitle(board, PROPOSE_COMMAND)}
          aria-expanded={props.asking === PROPOSE_COMMAND}
          onClick={() => props.onAsk(PROPOSE_COMMAND)}
        >
          {PROPOSE_LABEL}
        </button>
        <button
          type="button"
          className={styles.button}
          data-audit="openspecExplore"
          data-command={EXPLORE_COMMAND}
          data-write="true"
          title={commandTitle(board, EXPLORE_COMMAND)}
          aria-expanded={props.asking === EXPLORE_COMMAND}
          onClick={() => props.onAsk(EXPLORE_COMMAND)}
        >
          {EXPLORE_LABEL}
        </button>
      </div>
      {rows.map((row) => (
        <Row key={`${row.kind}:${row.id}`} row={row} props={props} />
      ))}
    </>
  )
}

/*
 * The composer is **not here any more**. (M28)
 *
 * It was a box that grew inside the sidebar, under the toolbar and above the tree; it is
 * `ProposeDialog.tsx` now. Two reasons, and the second decided it: a 320px column somebody
 * dragged to the width they wanted their *tree* at is the wrong place to write the most
 * consequential sentence this feature ever asks for — and the gesture had exactly one door, which
 * meant opening the sidebar and switching it to OpenSpec before *propose a change* was reachable
 * at all. It is a command now (`spec.propose`), so it is in the palette, and a command needs a
 * surface that does not assume this panel is on screen.
 *
 * The buttons stay here, because this is where somebody already looking at their changes expects
 * them. They open the dialog; `onAsk` is the whole of what this view knows about it.
 */

function Row({ row, props }: { row: SpecRow; props: OpenSpecPanelViewProps }) {
  if (row.kind === 'section') {
    const open = props.expanded[row.id] !== false
    return (
      <>
        <button
          type="button"
          className={styles.section}
          data-audit="openspecSection"
          data-section={row.id}
          data-open={open ? 'true' : 'false'}
          aria-expanded={open}
          onClick={() => props.onToggleSection(row.id)}
        >
          <Icon name={asIcon(open ? 'chevron-down' : 'chevron-right')} size={1} />
          <span>{row.label}</span>
          <span className={styles.sectionCount}>{row.count}</span>
        </button>
        {/*
          * Only when the section is empty. A section with rows is explained by its rows, and a
          * caption above every one of them is four wrapped lines of prose burying two lines of
          * content on a 320px panel.
          */}
        {open && row.count === 0 && (
          <p className={styles.sectionHint} data-audit="openspecSectionHint">
            {row.hint}
          </p>
        )}
      </>
    )
  }

  if (row.kind === 'spec') {
    return (
      <button
        type="button"
        className={styles.row}
        data-audit="openspecSpecRow"
        data-spec={row.id}
        onClick={() => props.onOpenSpec(row.id)}
      >
        <Icon name={asIcon('file-text')} size={1} />
        <span className={styles.rowLabel}>{row.label}</span>
        <span className={styles.rowMeta}>
          {row.requirements} req{row.requirements === 1 ? '' : 's'}
        </span>
      </button>
    )
  }

  return <ChangeRow row={row} props={props} />
}

/**
 * A change row: the row itself opens the proposal, the action beside it starts or opens the work.
 *
 * Two controls and not one, because they answer different questions — *what is this change*, and
 * *get me to the work on it* — and a row that did both on one click would have to guess which was
 * meant. Nested buttons are not legal markup, so they are siblings in a wrapper rather than a
 * button inside a button.
 */
function ChangeRow({ row, props }: { row: SpecRow & { kind: 'change' }; props: OpenSpecPanelViewProps }) {
  const action = rowAction(row.task, props.tracker)
  return (
    <div className={styles.changeRow} data-audit="openspecChangeRowWrap">
      <button
        type="button"
        className={styles.row}
        data-audit="openspecChangeRow"
        data-change={row.id}
        data-stage={row.stage}
        onClick={() => props.onOpenChange(row.id)}
      >
        <Icon name={asIcon(CHANGE_ICON)} size={1} />
        <span className={styles.rowLabel}>{row.label}</span>
        <span className={cx(styles.stage, STAGE_CLASS[row.stage])}>{stageLabel(row.stage)}</span>
        <span className={styles.rowMeta}>
          {row.done}/{row.total}
        </span>
      </button>
      <button
        type="button"
        className={styles.rowAction}
        data-audit="openspecRowAction"
        data-action={action.id}
        data-change={row.id}
        data-write="true"
        /*
         * The reason *is* the tooltip when there is one, and the button goes inert with it.
         * Drawing it live over a board nobody has read yet was the bug: `tasksStore.create`
         * refuses on `unknown` and `unreadable` and returns without a word, so the row offered
         * *Start work* on a change that already had a finished task and the click did nothing at
         * all. See `rowAction`.
         */
        title={action.disabledReason ?? action.title}
        disabled={action.disabledReason !== undefined}
        data-disabled={action.disabledReason === undefined ? undefined : 'true'}
        onClick={() => props.onRowAction(row.id, action.id)}
      >
        {action.label}
      </button>
    </div>
  )
}
