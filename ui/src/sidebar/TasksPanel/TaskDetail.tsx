/**
 * One task, opened — **a modal card, read-only until a field is put into edit**. (M18)
 *
 * # It is a modal now, and the objection that used to be in this comment has been answered
 *
 * The first version of this card replaced the list in place, and said here that a modal was
 * wrong because "a dialog floating over a sidebar is a dialog the activity rail can hide
 * behind". That was true of a card rendered where it is written: `layout/TabContent.module.css`
 * makes every tab panel its own stacking context, so a `z-index: 60` scrim inside one resolves
 * against that panel's children and paints under things that are siblings of the panel.
 * `overlays/ModalShell.tsx`'s [`OverlayCard`] now portals to `document.body`, which puts the
 * scrim in the root context and makes "mounted at App level" and "mounted inside a sidebar
 * panel" the same thing. So the card is a dialog, over the whole window, with the board still
 * visible behind the scrim — which is the layout a 320px sidebar could not give it.
 *
 * `OverlayCard` is used and not imitated: `role="dialog"`, `aria-modal`, the scrim, the
 * dismiss-on-scrim-click and the 620px geometry are all its, the same as the Settings roles
 * dialog and `ConfirmDestructive`. A fourth definition of what an overlay looks like in this app
 * is how two of them end up 4px apart after somebody adjusts one.
 *
 * # The split into two components is the SSR boundary, not decoration
 *
 * [`TaskDetailModal`] is the portal; [`TaskDetail`] is everything inside the card. A portal
 * cannot be server-rendered at all — `react-dom/server` throws *"Portals are not currently
 * supported by the server renderer"* — and `ui/scripts/check-agents-render.mjs` is the only gate
 * in this repository that can see a panel which compiles, mounts and draws nothing. Keeping the
 * whole card in a portal-free component keeps every one of its fields inside that gate; the
 * one-line wrapper is what is left outside, and the check reads it as source instead.
 *
 * # Read at rest, one field in edit
 *
 * `model.ts` holds the rules — which field is in edit, whether it is dirty, what a second
 * activation does, what closing does — and this file executes the [`EditIntent`] it is handed
 * without deciding one. Read the posture note above [`TASK_FIELDS`] for why `status` keeps its
 * live segment while the other three rest as text behind a pencil.
 *
 * The card reads **no store, calls no IPC and never reads the clock**: every fact and every
 * gesture arrives as a prop, which is what keeps it inside the render gate. In particular the
 * *field* draft lives in `TasksPanelHost`'s state and arrives as `editing`, so the check can
 * draw "the title field, in edit, with these three characters typed" as a story — a draft held
 * in a `useState` here would be reachable from no fixture. The `useRef` is a focus handle, not
 * state; the log's own `useState` belongs to the comment-editing slice and is local to it.
 *
 * # Comments are text, never markup
 *
 * A comment is **not a field**, and the read/edit posture above deliberately does not reach it.
 * The four fields are values a task *has*, so drawing one as text with a pencil on it is the
 * whole of what "edit this one" means; the log is a record of what was said, in time order, and
 * a pencil on the composer would be an affordance for rewriting something nobody has written
 * yet. `TASK_FIELDS` has four members for that reason, and the log's own controls — which are
 * the comment-editing slice's, with their own marks and their own argument — sit inside the log
 * rather than in the field list.
 *
 * The text is rendered into a `pre-wrap` block and **never as HTML or markdown**: it is
 * model-authored, and rendering model-authored markup inside the IDE's own chrome is an
 * injection surface bought for nothing.
 */
import { useRef, useState, type JSX } from 'react'
import { canOpen, canPause, elapsed, phaseGlyph, type RunPhase, type RunView } from '@/sidebar/AgentsPanel/model'
import { OverlayCard } from '@/overlays/ModalShell'
import {
  TASK_STATUSES,
  UNASSIGNED,
  agentChip,
  assignableRoles,
  assigneeFromDraft,
  authorLabel,
  beginEdit,
  cancelEdit,
  closeCard,
  commentOrder,
  commitEdit,
  fieldLabel,
  isFieldEmpty,
  restText,
  statusGlyph,
  statusLabel,
  statusTone,
  type Chip,
  type EditIntent,
  type EditableField,
  type FieldEdit,
  type RunRef,
  type TaskStatus,
  type TaskView,
  type Tone,
} from './model'
import styles from './TasksPanel.module.css'

/**
 * One CSS class per `Tone`.
 *
 * `string | undefined` because Vite types a CSS module as `Record<string, string>` under this
 * project's `noUncheckedIndexedAccess`; asserting a class away would be asserting the
 * stylesheet still defines it, which is what nothing in TypeScript can promise. The render
 * check's `unclassed` counter is what can.
 */
export const TONE_CLASS: Record<Tone, string | undefined> = {
  idle: styles.toneIdle,
  busy: styles.toneBusy,
  attention: styles.toneAttention,
  done: styles.toneDone,
}

/**
 * Join class names, dropping anything a missing stylesheet rule turned into `undefined`.
 *
 * The trap this avoids is the template literal: `` `${styles.row} ${maybe}` `` interpolates a
 * missing rule as the *string* `"undefined"`, which lands in the DOM as a real class token —
 * invisible to `tsc`, to `vite build`, and to any counter that only looks for an empty
 * attribute. The render check greps for that token too.
 */
export function cx(...parts: Array<string | undefined | false>): string {
  return parts.filter((part): part is string => typeof part === 'string' && part !== '').join(' ')
}

/**
 * The chip's class set, and the whole "an exited agent must not look like a working one" claim
 * expressed in markup.
 *
 * Both `lit` and `tone` are consulted, because `agentChip` carries both on purpose: two
 * independent discriminators mean a view has to go out of its way to collapse the two
 * renderings into one. `check-agents-render.mjs` asserts the class sets differ between a story
 * with a live run and the same list without one.
 */
export function chipClass(chip: Chip): string {
  if (!chip.lit) return cx(styles.chip, styles.chipAssigned)
  return cx(styles.chip, chip.tone === 'attention' ? styles.chipAttention : styles.chipLive)
}

/* ------------------------------------------------------------------- the delete control */

/**
 * What the unarmed control says. Three clauses, and the third is the one that makes it pressable.
 *
 * It names the act, the file it happens in, and the fact that this press does not perform it.
 * `AgentsPanel`'s `INTEGRATE_TITLE` is the same sentence one panel over, for the same reason: a
 * control whose first press is harmless has to say so, or a cautious user never presses it and
 * never finds out what it does.
 */
export const DELETE_TITLE =
  'Delete this task from .cide/tasks.json. Asks to confirm first.'

/**
 * What the armed one says — and, as with Integrate, it is the sentence that has to be *true*.
 *
 * `.cide/tasks.json` is a **committed file**, so a deleted task is in `git log -p .cide/tasks.json`
 * and `git checkout` brings it back. That is what makes two clicks the right weight here rather
 * than a modal: the cost of a mis-click is a `git` command, not lost work. It is also why the
 * sentence says so — a user who does not know the file is tracked is being asked to gamble.
 *
 * It is also why the card's delete stayed a two-click arming **after** the card became a modal.
 * A confirmation over a dialog is a dialog over a dialog: it takes the board off screen behind
 * two scrims to ask about an act `git` already has a copy of. Arming in place is the same weight
 * at the same distance, and the row in the list behind uses the identical control.
 */
export const DELETE_CONFIRM_TITLE =
  'Delete it now. .cide/tasks.json is committed, so git still has it.'

/** The card's unarmed label. A word, because the card has room for one. */
export const DELETE_LABEL = 'Delete task'

/**
 * The confirming label, **the same in both places**.
 *
 * One constant and not two: the row and the card are one gesture at two sizes, and a user who
 * learns what *Confirm delete* means in the card must not meet a different word in the list.
 * It is also the literal `check-agents-render.mjs` greps for, and a second spelling would let
 * one of the two call sites lose its confirmation while the check still passed on the other.
 */
export const DELETE_CONFIRM = 'Confirm delete'

/**
 * The row's unarmed control, which is a glyph rather than a word.
 *
 * 320px, and the row already carries a status glyph, an id, a title and an agent chip. A word
 * there would either push the title out or wrap the row. The glyph carries `title` and
 * `aria-label` with the whole sentence, and the *armed* state is a word — so the destructive
 * label appears exactly when the destructive press is available, which is also the property the
 * render check pins.
 */
export const DELETE_GLYPH = '×'

/**
 * The delete gesture: **at most one button, and the conditions in one function.**
 *
 * `AgentsPanel`'s `IntegrateControl` shape, deliberately — the same pair of handlers, the same
 * two-click arming, and the same refusal to draw a disabled confirming button when there is no
 * handler to act on it. Both of this panel's call sites go through here, so the list row and the
 * card cannot drift into two different ideas of what confirms a delete.
 *
 * `compact` is the only difference between them, and it is a *size*, not a behaviour: the row
 * gets a glyph and the card a word when unarmed, and both get [`DELETE_CONFIRM`] when armed.
 */
export function DeleteControl({
  task,
  label,
  armed,
  compact,
  onDeleteArm,
  onDelete,
}: {
  task: string
  /**
   * The task's title, for the accessible sentence.
   *
   * The row is one of many in a list; a screen reader announcing "delete" with no object makes
   * the user move off the control to find out which one. `IntegrateControl` names the role for
   * the identical reason.
   */
  label: string
  armed: boolean
  /** The row's control is a glyph, the card's a word. See [`DELETE_GLYPH`]. */
  compact: boolean
  onDeleteArm?: ((task: string) => void) | undefined
  onDelete?: ((task: string) => void) | undefined
}) {
  if (armed && onDelete !== undefined) {
    return (
      <button
        type="button"
        className={cx(
          styles.action,
          styles.actionDanger,
          compact ? styles.actionCompact : undefined,
        )}
        data-audit="tasksDeleteConfirm"
        data-write="true"
        title={DELETE_CONFIRM_TITLE}
        aria-label={`Delete ${label} now. ${DELETE_CONFIRM_TITLE}`}
        onClick={() => onDelete(task)}
      >
        {DELETE_CONFIRM}
      </button>
    )
  }
  if (onDeleteArm !== undefined) {
    return (
      <button
        type="button"
        className={compact ? cx(styles.rowDelete) : cx(styles.action)}
        data-audit="tasksDelete"
        data-write="true"
        title={DELETE_TITLE}
        aria-label={`Delete ${label}. ${DELETE_TITLE}`}
        onClick={() => onDeleteArm(task)}
      >
        {compact ? DELETE_GLYPH : DELETE_LABEL}
      </button>
    )
  }
  /*
   * Armed with no `onDelete`, or neither handler. Nothing is drawn — not a disabled *Confirm
   * delete*, which would be a control that has already taken the user's decision and then cannot
   * act on it. `IntegrateControl` says the same thing; `TasksPanelHost` passes the pair together.
   */
  return null
}

/**
 * Widen a chip into the shape `canOpen`/`canPause` take.
 *
 * Those two predicates are `AgentsPanel/model.ts`'s and they read one field between them —
 * but restating their rules here would be a second answer to "may this run be opened" and "may
 * it be frozen", and the first time the two disagreed the user would get an empty pane or a
 * pause that does nothing. So the chip is padded out instead. Every field but `phase` and
 * `session` is inert; nothing reads them.
 *
 * `phase` is cast because the chip carries it verbatim and unvalidated — which is correct, and
 * exactly what the predicates' own guards are for.
 */
function asRun(chip: Chip): RunView {
  return {
    run: chip.run ?? '',
    agent: '',
    agentLabel: chip.label,
    harness: 'claude',
    session: chip.session,
    phase: (chip.phase ?? '') as RunPhase,
    task: null,
    startedMs: 0,
    pausedSinceMs: null,
    exitCode: null,
    failure: null,
    staleTurn: false,
    note: null,
  }
}

export interface TaskDetailProps {
  task: TaskView
  /** Every run cide knows about, so the live-run strip can find this task's. */
  runs: readonly RunRef[]
  /** Agent id → label, for the assignee list and the chip's fallback ladder. */
  roles: Readonly<Record<string, string>>
  /** The clock, as a prop, so the comment log's ages are deterministic under SSR. */
  nowMs: number
  /**
   * The one field in edit and what has been typed into it, or `null` for a fully read-only card.
   *
   * `TasksPanelHost` owns it and `model.ts::activeEdit` has already refused an edit whose task
   * left the board, so this component draws what it is given rather than re-deciding.
   */
  editing?: FieldEdit | null | undefined
  /** Set the edit state — a keystroke, an intent's `editing`, or `null` to leave edit. */
  onEditing?: ((edit: FieldEdit | null) => void) | undefined
  /** Close the card. Required — a modal with no way out is a trap. */
  onClose: () => void
  onSetTitle?: ((task: string, title: string) => void) | undefined
  onSetStatus?: ((task: string, status: TaskStatus) => void) | undefined
  onSetAssignee?: ((task: string, agent: string | null) => void) | undefined
  onSetBody?: ((task: string, body: string) => void) | undefined
  onAddComment?: ((task: string, text: string) => void) | undefined
  /**
   * Replace one comment's text, and remove one. (M21)
   *
   * Optional together with `onAddComment`'s reason: a host that cannot write does not draw the
   * controls at all rather than drawing them dead. Rust refuses both for any caller but the
   * user, so these being present is a statement about *this* surface, not about permission.
   */
  onEditComment?: ((task: string, comment: string, text: string) => void) | undefined
  onDeleteComment?: ((task: string, comment: string) => void) | undefined
  /**
   * Is *this* task's delete armed — that is, waiting for the confirming second click?
   *
   * A `boolean` and not the armed id, because the card draws one task: `armedDelete(board, …)`
   * has already answered "which id, on which board" one layer up, and handing the raw id down
   * would let this component decide the question a second time and differently.
   */
  deleteArmed?: boolean | undefined
  /** First click: arm. Writes nothing. */
  onDeleteArm?: ((task: string) => void) | undefined
  /** Second click: remove the task from `.cide/tasks.json`. Only ever reachable from armed. */
  onDelete?: ((task: string) => void) | undefined
  /** Attach a pane to the live run's transcript. */
  onOpenRun?: ((run: string) => void) | undefined
  onPauseRun?: ((run: string) => void) | undefined
  onResumeRun?: ((run: string) => void) | undefined
}

/**
 * Execute one [`EditIntent`]: the write, then the new edit state, then the close.
 *
 * **In that order, and it matters.** `onClose` clears the host's `selected`, which unmounts this
 * component; a commit issued after it would be a write from a card that is gone. The mapping
 * from a field to its `TaskEdit` handler is the only decision here, and the one part of it that
 * is a rule — the empty option meaning *unassigned* — is `assigneeFromDraft`'s, in `model.ts`,
 * beside the `agent ?? ''` it has to agree with.
 */
function runIntent(props: TaskDetailProps, intent: EditIntent): void {
  const commit = intent.commit
  if (commit !== null) {
    if (commit.field === 'title') props.onSetTitle?.(props.task.id, commit.value)
    else if (commit.field === 'body') props.onSetBody?.(props.task.id, commit.value)
    else props.onSetAssignee?.(props.task.id, assigneeFromDraft(commit.value))
  }
  props.onEditing?.(intent.editing)
  if (intent.close) props.onClose()
}

/**
 * The card, in its dialog. **This is what the app mounts**; `TasksPanelHost` renders it beside
 * the list rather than in place of it, so the board stays on screen behind the scrim.
 *
 * One line of its own because [`OverlayCard`] portals: see the file header for why that boundary
 * is exactly the line between what a server render can see and what it cannot.
 *
 * The scrim's dismiss is a **deliberate close**, so a field left mid-edit is committed rather
 * than discarded — `closeCard`'s rule, not this component's.
 */
export function TaskDetailModal(props: TaskDetailProps) {
  const { task, roles, editing = null } = props
  return (
    <OverlayCard
      label={`Task ${task.id}: ${restText(task, 'title', roles)}`}
      onDismiss={() => runIntent(props, closeCard(task, editing, 'dismiss'))}
    >
      <TaskDetail {...props} />
    </OverlayCard>
  )
}

export function TaskDetail(props: TaskDetailProps) {
  const {
    task,
    runs,
    roles,
    nowMs,
    editing = null,
    onAddComment,
    onEditComment,
    onDeleteComment,
    onSetStatus,
    deleteArmed = false,
    onDeleteArm,
    onDelete,
    onOpenRun,
    onPauseRun,
    onResumeRun,
  } = props

  const chip = agentChip(task, runs, roles)
  const comments = commentOrder(task)
  /**
   * Which comment is open in its editor, by id.
   *
   * By id and not by index: the log is re-sorted on every board refresh and a merge can insert a
   * comment above the one being edited, so an index would move the textarea onto a different
   * comment mid-sentence. One at a time — two open editors would be two unsaved drafts with one
   * Escape between them.
   */
  const [editingComment, setEditingComment] = useState<string | null>(null)

  /*
   * The focus handle, and the reason this component still has no effects.
   *
   * A field's editor is `autoFocus`ed on mount, so activating one moves focus into it. When it
   * leaves edit the editor unmounts and focus would fall to `document.body` — outside this
   * React subtree — and the Escape handler below would stop hearing anything, which is a modal
   * whose Escape works until the first time you cancel an edit. Focusing the card back is done
   * in the gesture that closed the editor rather than in an effect after it: the handler runs
   * before React re-renders, so the focus lands while the editor is still there and stays on
   * the card when it goes.
   */
  const card = useRef<HTMLDivElement>(null)

  const run = (intent: EditIntent) => {
    if (!intent.close && intent.editing === null) card.current?.focus()
    runIntent(props, intent)
  }

  return (
    /*
     * `tabIndex={-1}` so the card can hold focus programmatically but never lands in the tab
     * order, and one `onKeyDown` for the whole card: React dispatches synthetic events through
     * the *React* tree, so this sees Escape from the editor inside the portal as well as from
     * the close button. `closeCard` decides what Escape means — cancel the field if one is open,
     * close the card if none is — and this handler does not.
     */
    <div
      ref={card}
      className={styles.taskCard}
      data-audit="taskCard"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.stopPropagation()
        run(closeCard(task, editing, 'escape'))
      }}
    >
      <div className={styles.cardHead} data-audit="taskCardHead">
        <span
          className={cx(styles.glyph, TONE_CLASS[statusTone(task.status)])}
          data-audit="tasksDetailGlyph"
          aria-hidden="true"
        >
          {statusGlyph(task.status)}
        </span>
        <span className={styles.detailId} data-audit="tasksDetailId">
          {task.id}
        </span>
        {/*
          * Who asked for this task, in the head with the id rather than as a field. (M21)
          *
          * It belongs beside the id because it is the same kind of thing: an unchangeable fact
          * about which task this is, not a value the card is offering to edit. A `FieldRow` would
          * have put it in the column of four rows that all carry a pencil, and the one without
          * one reads as a field whose affordance failed to render — the failure `restText` and
          * the status segment are both written against, one row down.
          *
          * The **creator**, never the assignee: `Task::agent` is the role the work is *for* and
          * it already has its own row and its own chip. Two names on one card that mean different
          * things need the words that tell them apart, which is why this says "Created by" in
          * full rather than leaning on position.
          *
          * **No timestamp.** `elapsed` is a run's clock — `12s`, `4m`, `1h 04m` — and a tracker
          * holds tasks that are weeks old, so it would print `847h 00m ago` on the row a user is
          * most likely to be reading. A creation *date* wants a second time vocabulary (days,
          * then weeks) that nothing else in this panel has, and the question asked was who, not
          * when; `Task::createdMs` is on the view when that day comes.
          */}
        <span
          className={styles.detailCreator}
          data-audit="tasksCreator"
          data-creator={task.createdBy.kind}
        >
          Created by {authorLabel(task.createdBy)}
        </span>
        {/*
          * The explicit way out, and the one that is always available.
          *
          * Three close the card — this, the scrim, and Escape — and a modal needs all three: the
          * scrim is not discoverable, Escape is not visible, and neither is reachable by someone
          * driving the app with a pointer and no keyboard. `autoFocus` puts focus inside the
          * dialog on open, which is what makes Escape work at all before anything else is
          * touched.
          */}
        <button
          type="button"
          className={styles.close}
          data-audit="tasksClose"
          autoFocus
          title="Close (Esc)"
          aria-label="Close task"
          onClick={() => run(closeCard(task, editing, 'dismiss'))}
        >
          ×
        </button>
      </div>

      <div className={styles.cardBody} data-audit="taskCardBody">
        <FieldRow field="title" task={task} roles={roles} editing={editing} run={run} props={props} />

        {/*
          * Status: the one field with no pencil, live at all times.
          *
          * `TASK_STATUSES` and not `GROUP_ORDER`: the segment is a progression a task moves
          * along, and the list's ordering is about what to read first — two different questions
          * with two different answers. `model.ts`'s posture note argues why this field keeps its
          * control while the other three rest as text; the short version is that a segment
          * cannot be changed by a gesture that was not aimed at it, and this is the gesture the
          * tracker exists for.
          */}
        <div className={styles.field} data-audit="taskField" data-field="status" data-mode="live">
          <div className={styles.fieldHead}>
            <span className={styles.fieldLabel}>{fieldLabel('status')}</span>
          </div>
          <div className={styles.segment} data-audit="tasksSegment" role="group" aria-label="Status">
            {TASK_STATUSES.map((status) => (
              <button
                key={status}
                type="button"
                className={cx(
                  styles.segmentButton,
                  status === task.status ? styles.segmentOn : undefined,
                )}
                data-audit="tasksStatusButton"
                data-status={status}
                data-write="true"
                aria-pressed={status === task.status}
                onClick={() => onSetStatus?.(task.id, status)}
              >
                {statusLabel(status)}
              </button>
            ))}
          </div>
        </div>

        <FieldRow field="assignee" task={task} roles={roles} editing={editing} run={run} props={props} />
        <FieldRow field="body" task={task} roles={roles} editing={editing} run={run} props={props} />

        {/*
          * The live-run strip: what is running against this task right now, from the same
          * `agentChip` call the list row makes, so the two cannot disagree about which of the
          * three states this task is in. Drawn only when a run is genuinely live — an assignment
          * alone is the assignee row above, not a strip claiming activity.
          */}
        {chip !== null && chip.lit && (
          <div className={styles.runStrip} data-audit="tasksRunStrip">
            <span className={chipClass(chip)} data-audit="tasksStripChip" data-lit="true">
              <span className={styles.chipDot} aria-hidden="true">
                {phaseGlyph((chip.phase ?? '') as RunPhase)}
              </span>
              <span className={styles.chipLabel}>{chip.label}</span>
            </span>
            <span className={styles.runStripText}>{chip.phase}</span>
            {canOpen(asRun(chip)) && chip.run !== null && onOpenRun !== undefined && (
              <button
                type="button"
                className={styles.action}
                data-audit="tasksOpenRun"
                onClick={() => onOpenRun(chip.run ?? '')}
              >
                Open
              </button>
            )}
            {canPause(asRun(chip)) && chip.run !== null && onPauseRun !== undefined && (
              <button
                type="button"
                className={styles.action}
                data-audit="tasksPauseRun"
                onClick={() => onPauseRun(chip.run ?? '')}
              >
                Pause
              </button>
            )}
            {chip.phase === 'paused' && chip.run !== null && onResumeRun !== undefined && (
              <button
                type="button"
                className={styles.action}
                data-audit="tasksResumeRun"
                onClick={() => onResumeRun(chip.run ?? '')}
              >
                Resume
              </button>
            )}
          </div>
        )}

        {/*
          * The log, and then the composer.
          *
          * The log **is** editable now, by the user and only by the user. (M21) This card used to
          * say the opposite at length, and the half of that argument which still holds is worth
          * keeping: a comment an *agent* can rewrite after the fact is a line nobody can trust.
          * That is enforced in Rust on the caller — `TasksStore::edit` refuses both variants for
          * anything but `TaskAuthor::User`, and no MCP tool can express them — so it is not
          * something these buttons are load-bearing for.
          *
          * An edited comment is marked as edited, including one the user edited that an agent
          * wrote. That case is the residue of the original hazard and the mark is the answer
          * to it.
          */}
        <div className={styles.field} data-audit="taskLogField">
          <span className={styles.fieldLabel}>Log</span>
          {comments.length === 0 ? (
            <p className={styles.empty} data-audit="tasksLogEmpty">
              No comments yet.
            </p>
          ) : (
            <div className={styles.log} data-audit="tasksLog">
              {comments.map((comment) => (
                <div
                  className={styles.logEntry}
                  data-audit="tasksComment"
                  data-author={comment.author.kind}
                  key={comment.id}
                >
                  <div className={styles.logHead}>
                    <span
                      className={cx(
                        styles.logAuthor,
                        comment.author.kind === 'user' ? styles.logAuthorUser : undefined,
                      )}
                    >
                      {authorLabel(comment.author)}
                    </span>
                    <span className={styles.logTime}>{elapsed(nowMs, comment.atMs)} ago</span>
                    {comment.editedMs !== null && (
                      <span className={styles.logEdited} data-audit="tasksCommentEdited">
                        edited
                      </span>
                    )}
                  </div>
                  {editingComment === comment.id && onEditComment !== undefined ? (
                    <form
                      className={styles.logEdit}
                      data-audit="tasksCommentEditor"
                      onSubmit={(event) => {
                        event.preventDefault()
                        const next = String(
                          new FormData(event.currentTarget).get('text') ?? '',
                        ).trim()
                        setEditingComment(null)
                        // An empty edit is a delete asked for the long way round, and answering
                        // it with an empty comment would leave a line in the log saying nothing.
                        // Refusing to write is the honest answer: Delete is right there.
                        if (next === '' || next === comment.text) return
                        onEditComment(task.id, comment.id, next)
                      }}
                    >
                      <textarea
                        className={styles.composerText}
                        name="text"
                        defaultValue={comment.text}
                        rows={3}
                        autoFocus
                        onKeyDown={(event) => {
                          if (event.key === 'Escape') {
                            event.stopPropagation()
                            setEditingComment(null)
                          }
                        }}
                      />
                      <div className={styles.logEditActions}>
                        <button type="submit" className={styles.logAction}>
                          Save
                        </button>
                        <button
                          type="button"
                          className={styles.logAction}
                          onClick={() => setEditingComment(null)}
                        >
                          Cancel
                        </button>
                      </div>
                    </form>
                  ) : (
                    <>
                      {/* Text, in a `pre-wrap` block. Never `dangerouslySetInnerHTML`, never a
                          markdown renderer — see the file header. */}
                      <p className={styles.logText}>{comment.text}</p>
                      {/*
                        * The two controls, on their own row under the comment rather than
                        * crammed into the head beside the timestamp. (M21)
                        *
                        * The head is provenance — who wrote this, when, and whether it has been
                        * edited since — and it reads as one right-aligned fact. Two buttons
                        * spliced into that row made both jobs worse: the timestamp stopped
                        * being the rightmost thing, and the buttons were a pair of 40x13
                        * targets 4px apart at the end of a line nobody was pointing at, which
                        * is a mis-click waiting to happen and was reported as one.
                        *
                        * Drawn only at rest. The editor below has its own Save and Cancel, and
                        * a Delete beside them would act on a comment the user is halfway
                        * through rewriting — one gesture answering two different questions.
                        */}
                      {(onEditComment !== undefined || onDeleteComment !== undefined) && (
                        <div className={styles.logActions} data-audit="tasksCommentActions">
                          {onEditComment !== undefined && (
                            <button
                              type="button"
                              className={styles.logAction}
                              data-audit="tasksCommentEdit"
                              aria-label={`Edit this comment on ${task.id}`}
                              onClick={() => setEditingComment(comment.id)}
                            >
                              Edit
                            </button>
                          )}
                          {onDeleteComment !== undefined && (
                            /* No confirmation. A comment is one line, deleting it is one click
                               to undo by writing it again, and `ConfirmDestructive`'s own rule
                               is that a dialog is for what cannot be undone — spending one here
                               is how a user learns to click through the ones that matter. */
                            <button
                              type="button"
                              className={styles.logAction}
                              data-audit="tasksCommentDelete"
                              aria-label={`Delete this comment on ${task.id}`}
                              onClick={() => onDeleteComment(task.id, comment.id)}
                            >
                              Delete
                            </button>
                          )}
                        </div>
                      )}
                    </>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>

        {onAddComment !== undefined && (
          <form
            className={styles.composer}
            data-audit="tasksComposer"
            onSubmit={(event) => {
              event.preventDefault()
              const form = event.currentTarget
              const text = String(new FormData(form).get('text') ?? '').trim()
              // An empty comment is not a comment. Appending one to a file the whole team reads
              // would be a line in the log saying nothing, and the log is append-only.
              if (text === '') return
              onAddComment(task.id, text)
              form.reset()
            }}
          >
            <label className={styles.fieldLabel} htmlFor={`task-comment-${task.id}`}>
              Comment
            </label>
            {/*
              * Uncontrolled, and the only uncontrolled control left on the card. It is not a
              * field: nothing on the board can contradict it, there is no value to be dirty
              * against, and `form.reset()` after a successful append is the whole of its state.
              */}
            <textarea
              id={`task-comment-${task.id}`}
              className={styles.textarea}
              data-audit="tasksCommentField"
              data-write="true"
              name="text"
              defaultValue=""
            />
            <div className={styles.composerActions}>
              <button
                type="submit"
                className={styles.action}
                data-audit="tasksCommentSubmit"
                data-write="true"
              >
                Add comment
              </button>
            </div>
          </form>
        )}

        {/*
          * Delete, at the foot of the card and nowhere else in it.
          *
          * Last, and after the log, because a destructive control that sits beside the fields is
          * one the hand reaches on its way to the title. It is not in the head either: the close
          * button is the one control a user presses without reading, and a delete next to it
          * would be pressed the same way.
          *
          * It is offered here **as well as** on the list row, which is not a duplicate. The row
          * is where you delete something you created by mistake and never opened; the card is
          * where you delete the one you are looking at. Both go through `DeleteControl`, so
          * there is one answer to what confirms a delete — and it stays a two-click arming here
          * rather than becoming a confirmation dialog, for `DELETE_CONFIRM_TITLE`'s reason.
          */}
        <div className={styles.detailActions} data-audit="tasksDetailActions">
          <DeleteControl
            task={task.id}
            label={task.title.trim() !== '' ? task.title : task.id}
            armed={deleteArmed}
            compact={false}
            onDeleteArm={onDeleteArm}
            onDelete={onDelete}
          />
        </div>
      </div>
    </div>
  )
}

/**
 * One editable field: **its value as text with a pencil on it, or its editor**.
 *
 * The three of them are one component because the difference between them is which control the
 * edit state draws, and everything else — the heading, the affordance, the accessible names,
 * which gesture produces which intent — is identical. Three copies is how the body field comes
 * to commit on blur again six months from now while the title does not.
 *
 * `data-field` and `data-mode` are what `check-agents-render.mjs` reads: *rest* must contain no
 * control at all, which is the assertion the whole change is about.
 */
function FieldRow({
  field,
  task,
  roles,
  editing,
  run,
  props,
}: {
  field: EditableField
  task: TaskView
  roles: Readonly<Record<string, string>>
  editing: FieldEdit | null
  run: (intent: EditIntent) => void
  /** For `onEditing`, which a keystroke calls directly — a draft is not an intent. */
  props: TaskDetailProps
}) {
  const label = fieldLabel(field)
  const open = editing !== null && editing.field === field
  const id = `task-${field}-${task.id}`
  const editors: Record<EditableField, () => JSX.Element> = {
    title: () => (
      <input
        id={id}
        className={styles.input}
        data-audit="taskEditor"
        data-field={field}
        data-write="true"
        type="text"
        autoFocus
        aria-label={label}
        value={editing?.draft ?? ''}
        onChange={(event) => props.onEditing?.({ field, draft: event.target.value })}
        onKeyDown={(event) => {
          // Enter saves a one-line field, which is what Enter means in one. The card's Escape
          // handler is a level up and cancels; neither is duplicated here.
          if (event.key !== 'Enter') return
          event.preventDefault()
          run(commitEdit(task, editing))
        }}
      />
    ),
    assignee: () => (
      <select
        id={id}
        className={styles.select}
        data-audit="taskEditor"
        data-field={field}
        data-write="true"
        autoFocus
        aria-label={label}
        value={editing?.draft ?? ''}
        /*
         * Choosing **is** the commit, so there is no Save beside this one. A `<select>` already
         * costs a click to open and a click to choose; a third press to confirm the choice the
         * user just made would be a button with nothing left to do. The draft is built here
         * rather than stored first because the change carries the whole value.
         */
        onChange={(event) => run(commitEdit(task, { field, draft: event.target.value }))}
      >
        <option value="">{UNASSIGNED}</option>
        {assignableRoles(task.agent, roles).map((agent) => (
          <option key={agent} value={agent}>
            {roles[agent] ?? agent}
          </option>
        ))}
      </select>
    ),
    body: () => (
      <textarea
        id={id}
        className={styles.textarea}
        data-audit="taskEditor"
        data-field={field}
        data-write="true"
        autoFocus
        aria-label={label}
        value={editing?.draft ?? ''}
        onChange={(event) => props.onEditing?.({ field, draft: event.target.value })}
        onKeyDown={(event) => {
          // Enter is a newline in a body, so the shortcut is the modified one. Save is on
          // screen as well — a shortcut nobody can see is not a way out of a field.
          if (event.key !== 'Enter' || !(event.ctrlKey || event.metaKey)) return
          event.preventDefault()
          run(commitEdit(task, editing))
        }}
      />
    ),
  }

  return (
    <div
      className={styles.field}
      data-audit="taskField"
      data-field={field}
      data-mode={open ? 'edit' : 'rest'}
    >
      <div className={styles.fieldHead}>
        {/* A `<label>` only while there is a control for it to name. At rest the row's value is
            a `<p>`, and a label pointing at nothing is worse than a heading that admits it is
            one. */}
        {open ? (
          <label className={styles.fieldLabel} htmlFor={id}>
            {label}
          </label>
        ) : (
          <span className={styles.fieldLabel}>{label}</span>
        )}
        {/*
          * The affordance, and the **same element in both states**: a `<button>` in the same
          * place in the same parent, so React updates it rather than unmounting one and mounting
          * another. That is what keeps a field's control in one position as it opens and closes,
          * instead of a pencil that disappears and a Cancel that appears somewhere else.
          *
          * It is drawn on all three editable fields and on none of the status segment. No
          * `data-write`: at rest it opens an editor and writes nothing. It *can* flush a
          * different field that was left dirty, but that write belongs to the field being left —
          * `beginEdit` is where that is decided and where it is explained.
          */}
        <button
          type="button"
          className={styles.fieldEdit}
          data-audit={open ? 'taskFieldCancel' : 'taskFieldEdit'}
          data-field={field}
          title={open ? `Stop editing ${label.toLowerCase()}` : `Edit ${label.toLowerCase()}`}
          aria-label={
            open
              ? `Cancel editing ${label.toLowerCase()} of ${task.id}`
              : `Edit ${label.toLowerCase()} of ${task.id}`
          }
          onClick={() => run(open ? cancelEdit() : beginEdit(task, editing, field))}
        >
          {open ? 'Cancel' : '✎'}
        </button>
      </div>

      {open ? (
        <>
          {editors[field]()}
          {/*
            * Save, for the two fields whose editor cannot commit itself. The assignee's
            * `<select>` commits on choice and draws none, which is why this is conditional
            * rather than uniform: a Save button that never has anything to save is a control
            * that teaches the user their choice did not take.
            */}
          {field !== 'assignee' && (
            <div className={styles.fieldActions}>
              <button
                type="button"
                className={cx(styles.action, styles.actionPrimary)}
                data-audit="taskFieldSave"
                data-field={field}
                data-write="true"
                onClick={() => run(commitEdit(task, editing))}
              >
                Save
              </button>
            </div>
          )}
        </>
      ) : (
        /*
         * At rest. **Text, never a disabled control** — a greyed-out `<input>` is still an input:
         * it is in the tab order on some engines, it looks broken rather than settled, and it
         * would put the wall of boxes back on screen that this posture exists to remove.
         * `restText` is total and never empty, so a task with no body still draws a row.
         */
        <p
          className={cx(
            styles.fieldValue,
            field === 'body' ? styles.fieldValueBody : undefined,
            isFieldEmpty(task, field) ? styles.fieldValueEmpty : undefined,
          )}
          data-audit="taskFieldValue"
          data-field={field}
        >
          {restText(task, field, roles)}
        </p>
      )}
    </div>
  )
}
