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
import { useRef, useState, type JSX } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import {
  LINK_KINDS,
  TASK_STATUSES,
  UNASSIGNED,
  assignableRoles,
  closeCompose,
  draftReady,
  fieldLabel,
  isLinkKind,
  linkLabel,
  statusLabel,
  type LinkKind,
  type LinkTargetOption,
  type TaskDraft,
} from './model'
import { cx } from './TaskDetail'
import { LinkTargetInput } from './LinkTargetInput'
import { MentionTextarea } from './MentionTextarea'
import { Icon } from '@/icons/Icon'

import styles from './TasksPanel.module.css'

/**
 * The sentence under the picker.
 *
 * One fact a user cannot get anywhere else, and it is about what pressing Create will *do*:
 * assigning is what starts the agent, which is the single most surprising thing about this
 * feature if nobody says it.
 *
 * It said one thing more — the directory a proposed change would take — until the option that
 * proposed one was removed. See the picker below for why.
 */
function specNote(draft: TaskDraft): string {
  const assigned = draft.assignee.trim() !== ''
  return assigned
    ? 'Creating this assigns the role and starts it on this change — approving a change is assigning it.'
    : 'Links this task to that change, so a run started on it is pointed at the checklist.'
}

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
  /**
   * The changes this project has in flight, newest question first. (M28)
   *
   * **Absent means the row is not drawn at all** — a project with no `openspec/`, or one whose
   * board has not been read, sees exactly the dialog it saw before M28. That is the optionality
   * claim, made structurally rather than by a flag somebody could forget to check.
   */
  changes?: readonly string[] | undefined
  /**
   * The tasks a link may point at — the board's rows, in panel order. (M30)
   *
   * Absent means the links row is not drawn: `changes`' structural optionality, for its reason
   * — and it is also simply true that a board that has not been read has nothing to link to.
   */
  tasks?: readonly LinkTargetOption[] | undefined
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
  changes,
  tasks,
  onDraft,
  onCreate,
  onCancel,
  busy = false,
}: TaskComposeProps) {
  const ready = draftReady(draft)
  /*
   * Which kind the next link takes. (M30) Local, unlike everything else the dialog holds,
   * because it is not part of the draft: choosing a *target* is the add — the assignee
   * `<select>`'s choosing-is-the-commit argument — and this only says what that add will mean.
   * The markup is identical whatever it holds, so nothing the render check could story lives
   * here.
   */
  const [linkKind, setLinkKind] = useState<LinkKind>('related')
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
            <>
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
              {/* Inside the row, not after it: the rows are bordered blocks now, and a sentence
                  about the assignee list floating between two boxes belongs to neither. The card's
                  editor already draws it inside the field for the same reason. */}
              {assigneeHint !== null && (
                <p className={styles.assigneeHint} data-audit="tasksAssigneeHint">
                  {assigneeHint}
                </p>
              )}
            </>
          )}
        </Field>

        {/*
          * The OpenSpec row. (M28)
          *
          * Its own row, hooked `taskComposeSpecRow` and **not** `taskComposeRow`, which is not
          * cosmetic: `check-agents-render.mjs` asserts `composeControls === 3` with a paragraph
          * arguing exactly three — the title box, the assignee select and the body box. Adding a
          * fourth control to that hook would break a check whose comment explains a different
          * number, and the honest fix is a hook of its own.
          */}
        {changes !== undefined && (
          <div
            className={styles.field}
            data-audit="taskComposeSpecRow"
            data-field="change"
          >
            <label className={styles.fieldLabel} htmlFor="compose-change">
              OpenSpec change
            </label>
            <select
              id="compose-change"
              className={styles.select}
              data-audit="taskComposeSpecPicker"
              data-write="true"
              value={draft.change}
              onChange={(event) => onDraft({ ...draft, change: event.target.value })}
            >
              <option value="">Not a spec change</option>
              {/*
                * **There is no "new change from this task" option, and there must not be.**
                *
                * There was. It called `spec_propose`, which scaffolds the directory and writes a
                * stub proposal — no delta specs, no task checklist. So pressing Create produced
                * a change that `openspec validate` refuses and whose board row reads `0/0` for
                * ever: cide manufacturing a broken artefact out of a title, at the one moment
                * the user has told it least about the work.
                *
                * Writing a proposal *is* work — it needs the codebase and the existing specs —
                * so the gesture moved to after the task exists, where a conversation can do it:
                * the card offers *Make a proposal*, which runs `/openspec-propose` and tells it
                * to link the change back. This picker only ever links to a change that is
                * already there.
                */}
              {changes.map((change) => (
                <option key={change} value={change}>
                  {change}
                </option>
              ))}
            </select>
            {draft.change !== '' && (
              <p className={styles.assigneeHint} data-audit="taskComposeSpecNote">
                {specNote(draft)}
              </p>
            )}
          </div>
        )}

        {/*
          * The links row. (M30)
          *
          * Its own hooks — `taskComposeLinkRow`, not `taskComposeRow` — for exactly the reason
          * the spec row's comment gives: `check-agents-render.mjs` asserts `composeControls === 3`
          * with a paragraph arguing exactly three, and a fourth control under that hook would
          * break a check whose comment explains a different number.
          *
          * Choosing a target **is** the add — the assignee `<select>`'s argument: the choice
          * carries the whole value, and a third press to confirm it would be a button with
          * nothing left to do. The kind `<select>` beside it says what the add will mean. Links
          * are here at all, rather than left to the card afterwards, because a create naming an
          * assignee dispatches immediately — `TaskDraft::links` carries the interleaving.
          */}
        {tasks !== undefined && (
          <div className={styles.field} data-audit="taskComposeLinkRow" data-field="links">
            <label className={styles.fieldLabel} htmlFor="compose-link-target">
              Links
            </label>
            {draft.links.length > 0 && (
              <div className={styles.linkChips} data-audit="taskComposeLinkChips">
                {draft.links.map((link) => (
                  <span className={styles.linkPair} key={`${link.kind}:${link.target}`}>
                    <span
                      className={styles.linkChip}
                      data-audit="taskComposeLinkChip"
                      data-kind={link.kind}
                      data-target={link.target}
                    >
                      {linkLabel(link.kind, 'out')} {link.target}
                    </span>
                    <button
                      type="button"
                      className={styles.linkRemove}
                      data-audit="taskComposeLinkRemove"
                      title="Remove this link from the draft"
                      aria-label={`Do not link ${link.target}`}
                      onClick={() =>
                        onDraft({
                          ...draft,
                          links: draft.links.filter(
                            (kept) => kept.kind !== link.kind || kept.target !== link.target,
                          ),
                        })
                      }
                    >
                      <Icon name="x" size={0} />
                    </button>
                  </span>
                ))}
              </div>
            )}
            <div className={styles.linkAdd}>
              <select
                className={cx(styles.select, styles.linkKind)}
                data-audit="taskComposeLinkKind"
                aria-label="Link kind"
                value={linkKind}
                onChange={(event) => {
                  const kind = event.target.value
                  if (isLinkKind(kind)) setLinkKind(kind)
                }}
              >
                {LINK_KINDS.map((kind) => (
                  <option key={kind} value={kind}>
                    {linkLabel(kind, 'out')}
                  </option>
                ))}
              </select>
              {/*
                * The target, by search — the same id-or-title autocomplete the card's picker
                * uses, so finding a task reads identically on both surfaces. Picking clears the
                * input, ready for the next one; no `onDismiss`, so Escape on the empty input
                * still falls through to the dialog's own Escape, exactly as it does from every
                * other field here.
                */}
              <LinkTargetInput
                targets={tasks}
                listboxId="compose-link-targets"
                id="compose-link-target"
                data-audit="taskComposeLinkTarget"
                data-write="true"
                aria-label="Link a task"
                onPick={(target) => {
                  // A duplicate pick collapses silently to the chip that is already drawn — the
                  // gesture's outcome is on screen either way, which is more than a refusal
                  // sentence would say here.
                  if (
                    draft.links.some((link) => link.kind === linkKind && link.target === target)
                  ) {
                    return
                  }
                  onDraft({ ...draft, links: [...draft.links, { kind: linkKind, target }] })
                }}
              />
            </div>
          </div>
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
