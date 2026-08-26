/**
 * The compose dialog: a new task, filled in before it exists. (M21)
 *
 * # What this replaces, and the report that asked for it
 *
 * *New task* used to **create the row and then open its card**. A task titled `New task` appeared
 * in `.cide/tasks.json` on the click, and the four fields were set one at a time afterwards, each
 * one its own write. Reported as *"i'm expecting that all fields are editable and task isn't
 * created while not press Create"*, and the report is right about more than taste — that file is
 * committed and shared with every agent in the project, so a mis-click put a row into the team's
 * tracker that had to be *deleted* rather than abandoned, and a run dispatched in between could
 * read a task whose title was still the placeholder.
 *
 * So nothing is written until Create. The draft lives in `tasksStore.compose` — in this window,
 * in memory, on no wire and in no file — and `TaskDraft`'s own doc argues why that is the one
 * piece of task state allowed outside Rust: it does not exist yet, so there is nothing for two
 * windows to disagree about.
 *
 * # Every field is live at once, which is the opposite of the card
 *
 * `TaskDetail` rests as **text with a pencil on it** and opens one field at a time, and that
 * posture is load-bearing there: it is a record several agents are also writing to, and a wall of
 * live boxes over it invites an edit that was not aimed at anything. Stated here so the two are
 * not "made consistent" by someone reading only one of them.
 *
 * None of that applies to a task that does not exist. There is nothing to read, nothing anyone
 * else can be writing, and no field whose old value could be lost — the whole dialog is one
 * gesture that ends in Create or in nothing at all. A compose form that made the user click a
 * pencil per field would be four ceremonies to write one task.
 *
 * # Create is drawn disabled, not withheld
 *
 * `draftReady` is the gate and the title is the only required field, because `cide_tasks::validate`
 * refuses a task without one. The button is **visibly waiting** rather than absent, which is the
 * opposite of the `ready-queued` rule one panel over and deliberately so: that rule is about a
 * control that can never work in a state the user cannot change, and this one goes live on the
 * first character they type. A Create that materialised mid-word is harder to understand than one
 * that has been sitting there greyed out.
 */
import { useRef, type JSX } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import {
  TASK_STATUSES,
  UNASSIGNED,
  assignableRoles,
  closeCompose,
  draftReady,
  fieldLabel,
  statusLabel,
  type TaskDraft,
} from './model'
import { cx } from './TaskDetail'
import { MentionTextarea } from './MentionTextarea'
import { Icon } from '@/icons/Icon'

import styles from './TasksPanel.module.css'

export interface TaskComposeProps {
  /** What has been typed so far. Controlled — the store holds it; see `tasksStore.compose`. */
  draft: TaskDraft
  /** Agent id → label, for the assignee list. The same map the card is handed. */
  roles: Readonly<Record<string, string>>
  /**
   * Why the assignee list is short — `model.ts::assigneeHint`'s sentence, or `null`/absent when
   * the list speaks for itself. Without it, a disabled-roster dropdown holding only *Unassigned*
   * is indistinguishable from the wiring bug it used to be.
   */
  assigneeHint?: string | null | undefined
  /** A keystroke, a choice, a status. The draft is replaced whole. */
  onDraft: (draft: TaskDraft) => void
  /** Create. Only ever called with a draft `draftReady` admits. */
  onCreate: (draft: TaskDraft) => void
  /** Discard and close. Cancel, the ✕ and Escape all arrive here; so does the scrim when clean. */
  onCancel: () => void
  /**
   * Is the create in flight?
   *
   * The dialog stays up and Create goes inert, rather than the dialog closing on the click. A
   * write that failed after the dialog had gone would take the user's paragraph with it, and
   * there is no draft on disk to recover it from — which is the point of a draft that never
   * reached Rust.
   */
  busy?: boolean | undefined
}

/**
 * The dialog, in its scrim.
 *
 * The scrim's dismiss is the one way out that can be **refused**: `closeCompose` ignores it while
 * the draft is dirty. That is the single asymmetry with `TaskDetailModal`, and its argument is in
 * `closeCompose` — there, leaving keeps what you typed, so a stray click costs nothing; here
 * leaving *discards*, with no undo and nothing on disk to say it happened. Three deliberate ways
 * out remain, all of them visible or standard.
 */
export function TaskComposeModal(props: TaskComposeProps) {
  return (
    <OverlayCard
      label="New task"
      onDismiss={() => {
        if (closeCompose(props.draft, 'dismiss')) props.onCancel()
      }}
    >
      <TaskCompose {...props} />
    </OverlayCard>
  )
}

/**
 * The dialog's contents, minus the portal — which is what makes it renderable by
 * `check:agents-render`.
 *
 * `TaskDetail` is split from `TaskDetailModal` for exactly this reason and says so: `OverlayCard`
 * portals to `document.body`, and `react-dom/server` refuses a portal outright. A component this
 * project cannot render under node is a component with no gate on whether it paints, and this
 * repository has shipped a panel that compiled, mounted and drew nothing more than once.
 */
export function TaskCompose({
  draft,
  roles,
  assigneeHint = null,
  onDraft,
  onCreate,
  onCancel,
  busy = false,
}: TaskComposeProps) {
  const ready = draftReady(draft)
  /*
   * The focus handle, for the same reason the card has one: Escape is handled on this element,
   * and React dispatches synthetic events through the React tree, so it hears Escape from every
   * control inside. Nothing here can move focus out of the subtree the way the card's closing
   * editors could, but the handle costs one ref and removes the question.
   */
  const card = useRef<HTMLDivElement>(null)

  const submit = () => {
    // The same gate the button is drawn from, checked again here because Enter and Ctrl+Enter
    // reach this without passing through it. A keyboard path that could write an untitled task
    // is `validate`'s refusal arriving as a failure notice instead of a disabled button.
    if (!ready || busy) return
    onCreate(draft)
  }

  return (
    <div
      ref={card}
      className={styles.taskCard}
      data-audit="taskCompose"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        // Escape always discards, dirty or not: the user asked for a create that has not
        // happened, and there is nothing to commit on the way out. `closeCompose` is still the
        // one that decides, so this handler does not restate the rule.
        event.stopPropagation()
        if (closeCompose(draft, 'escape')) onCancel()
      }}
    >
      <div className={styles.cardHead} data-audit="taskComposeHead">
        <span className={styles.composeTitle}>New task</span>
        {/*
          * The ✕ closes unconditionally — `closeCompose` refuses only the scrim. It is a
          * deliberate, aimed gesture with a Cancel beside it saying the same thing; the scrim is
          * the one that gets hit by accident.
          */}
        <button
          type="button"
          className={styles.close}
          data-audit="taskComposeClose"
          title="Cancel (Esc)"
          aria-label="Cancel new task"
          onClick={() => {
            if (closeCompose(draft, 'cancel')) onCancel()
          }}
        >
          <Icon name="x" size={1} />
        </button>
      </div>

      {/*
        * A real `<form>`, so Enter in the title submits the way Enter submits a form everywhere
        * else, and so the button is a `type="submit"` the browser wires to it. `noValidate` is
        * not needed — nothing here uses HTML validation, because `draftReady` is the rule and a
        * second one in an attribute would be a second answer to what a valid task is.
        */}
      <form
        className={styles.cardBody}
        data-audit="taskComposeBody"
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
      >
        <Field field="title">
          {(id) => (
            <input
              id={id}
              className={styles.input}
              data-audit="taskComposeField"
              data-field="title"
              data-write="true"
              type="text"
              /* Focus lands here on open, which is what makes Escape work before anything is
                 touched and what lets the whole task be typed without reaching for the mouse. */
              autoFocus
              value={draft.title}
              onChange={(event) => onDraft({ ...draft, title: event.target.value })}
            />
          )}
        </Field>

        {/*
          * Status, as the card's segment rather than a `<select>`: it is the same four-state
          * progression, drawn the same way, so the control the user learns here is the one they
          * meet again on the card. `TASK_STATUSES` and not `GROUP_ORDER` — the segment is the
          * progression a task moves along, the list's order is about what to read first.
          *
          * The dialog is the *only* caller that can send this. `TaskNew::status` has the whole
          * argument: a person choosing from four states in front of them saying "I am starting
          * this now" is ordinary, and the restriction that matters — an agent minting finished
          * work — is kept structurally, in a signature the MCP path cannot express.
          */}
        <div className={styles.field} data-audit="taskComposeRow" data-field="status">
          <div className={styles.fieldHead}>
            <span className={styles.fieldLabel}>{fieldLabel('status')}</span>
          </div>
          <div className={styles.segment} data-audit="taskComposeSegment" role="group" aria-label="Status">
            {TASK_STATUSES.map((status) => (
              <button
                key={status}
                type="button"
                className={cx(
                  styles.segmentButton,
                  status === draft.status ? styles.segmentOn : undefined,
                )}
                data-audit="taskComposeStatus"
                data-status={status}
                aria-pressed={status === draft.status}
                onClick={() => onDraft({ ...draft, status })}
              >
                {statusLabel(status)}
              </button>
            ))}
          </div>
        </div>

        <Field field="assignee">
          {(id) => (
            <select
              id={id}
              className={styles.select}
              data-audit="taskComposeField"
              data-field="assignee"
              data-write="true"
              value={draft.assignee}
              onChange={(event) => onDraft({ ...draft, assignee: event.target.value })}
            >
              {/* The empty option **is** unassigned — a `<select>` has no null. `assigneeFromDraft`
                  is the other half of that convention and converts it back in the store. */}
              <option value="">{UNASSIGNED}</option>
              {assignableRoles(null, roles).map((agent) => (
                <option key={agent} value={agent}>
                  {roles[agent] ?? agent}
                </option>
              ))}
            </select>
          )}
        </Field>
        {assigneeHint !== null && (
          <p className={styles.assigneeHint} data-audit="tasksAssigneeHint">
            {assigneeHint}
          </p>
        )}

        <Field field="body">
          {(id) => (
            <MentionTextarea
              id={id}
              className={styles.textarea}
              data-audit="taskComposeField"
              data-field="body"
              data-write="true"
              roles={roles}
              listboxId="compose-body-mentions"
              tools
              value={draft.body}
              onValueChange={(body) => onDraft({ ...draft, body })}
              onKeyDown={(event) => {
                // Enter is a newline in a body, so the submit shortcut is the modified one —
                // the card's body editor makes the same distinction. Create is on screen too: a
                // shortcut nobody can see is not a way out of a field. Plain Enter with the
                // mention popup open never reaches here — the popup claims it.
                if (event.key !== 'Enter' || !(event.ctrlKey || event.metaKey)) return
                event.preventDefault()
                submit()
              }}
            />
          )}
        </Field>

        {/*
          * Cancel first, Create last, and Create is the submit.
          *
          * The destructive-ish one on the left and the one the gesture is *for* on the right,
          * which is the order every other dialog in this app uses. Cancel is `type="button"`
          * explicitly: a bare `<button>` inside a form defaults to `submit`, and a Cancel that
          * created the task would be the exact defect this dialog was written to remove.
          */}
        <div className={styles.composeActions} data-audit="taskComposeActions">
          <button
            type="button"
            className={styles.action}
            data-audit="taskComposeCancel"
            onClick={onCancel}
          >
            Cancel
          </button>
          <button
            type="submit"
            className={cx(styles.action, styles.actionPrimary)}
            data-audit="taskComposeCreate"
            data-write="true"
            disabled={!ready || busy}
            /* Named, because `disabled` alone tells a screen reader the button is off and not
               why. The title is the same sentence a sighted user infers from an empty field. */
            title={ready ? 'Create this task' : 'A task needs a title'}
          >
            Create
          </button>
        </div>
      </form>
    </div>
  )
}

/**
 * One labelled row: the heading, and the control it names.
 *
 * A `<label htmlFor>` in every case, unlike the card — there the row is a `<p>` at rest and a
 * label pointing at nothing is worse than a heading that admits it is one. Here there is always
 * a control, so there is always something to point at.
 */
function Field({
  field,
  children,
}: {
  field: 'title' | 'assignee' | 'body'
  children: (id: string) => JSX.Element
}) {
  const id = `compose-${field}`
  return (
    <div className={styles.field} data-audit="taskComposeRow" data-field={field}>
      <div className={styles.fieldHead}>
        <label className={styles.fieldLabel} htmlFor={id}>
          {fieldLabel(field)}
        </label>
      </div>
      {children(id)}
    </div>
  )
}
