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
 * *field* draft lives in `TaskDetailHost`'s state and arrives as `editing`, so the check can
 * draw "the title field, in edit, with these three characters typed" as a story — a draft held
 * in a `useState` here would be reachable from no fixture. The `useRef` is a focus handle, not
 * state; the log's own `useState` belongs to the comment-editing slice and is local to it.
 *
 * # Comments are not fields
 *
 * A comment is **not a field**, and the read/edit posture above deliberately does not reach it.
 * The four fields are values a task *has*, so drawing one as text with a pencil on it is the
 * whole of what "edit this one" means; the log is a record of what was said, in time order, and
 * a pencil on the composer would be an affordance for rewriting something nobody has written
 * yet. `TASK_FIELDS` has four members for that reason, and the log's own controls — which are
 * the comment-editing slice's, with their own marks and their own argument — sit inside the log
 * rather than in the field list.
 *
 * # Comments and the body render as markdown now — through the AST, never through HTML (M27)
 *
 * This header used to refuse markup here outright, on injection grounds, and the half of that
 * argument that was about *mechanism* still stands: model-authored text must never reach
 * `dangerouslySetInnerHTML`, with or without a sanitizer in front of it. What changed is that
 * this project now owns a parser with no HTML node in its grammar (`editor/markdown/types.ts`
 * carries the argument; `<b>` in a comment is four characters of text), so `TaskMarkdown.tsx`
 * renders the tree as React elements and every string still goes through React's escaping.
 * Agents write `**bold**`, fences and lists into this log all day; drawing the syntax raw was
 * the panel refusing to read what its main authors write.
 */
import { useRef, useState, type JSX } from 'react'
import {
  canOpen,
  canPause,
  elapsed,
  glyphSpins,
  phaseGlyph,
  type RunPhase,
  type RunView,
} from '@/sidebar/AgentsPanel/model'
import { OverlayCard } from '@/overlays/ModalShell'
import {
  LINK_KINDS,
  TASK_STATUSES,
  UNASSIGNED,
  agentChip,
  assignableRoles,
  assigneeFromDraft,
  authorLabel,
  beginEdit,
  cancelEdit,
  clock,
  closeCard,
  commentOrder,
  commitEdit,
  historyOrder,
  fieldLabel,
  isFieldEmpty,
  isLinkKind,
  linkLabel,
  restText,
  statusGlyph,
  statusLabel,
  statusTone,
  type Chip,
  type EditIntent,
  type EditableField,
  type FieldEdit,
  type LinkChip,
  type LinkKind,
  type LinkTargetOption,
  type RunRef,
  type TaskStatus,
  type TaskView,
  type Tone,
  dropTargetKey,
  type AttachmentPreview,
  type AttachTargetView,
  type StagedAttachment,
} from './model'
import { Icon, asIcon } from '@/icons/Icon'
import { MentionTextarea } from './MentionTextarea'
import { LinkTargetInput } from './LinkTargetInput'
import { TaskMarkdown } from './TaskMarkdown'
import { AttachmentStrip, StagedChips } from './AttachmentStrip'
import { wantsImagePaste } from '@/editor/pasteImage'
import { RequirementEditor } from '@/sidebar/OpenSpecPanel/RequirementEditor'
import {
  artifactPath,
  progressLabel,
  progressPercent,
  sessionHint,
  sessionState,
  showsAssignee,
  targetKey,
  validityLabel,
  type DispatchTarget,
  busyLabel,
  type SpecCardView,
  type SpecEditView,
} from './specCard'

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
 * independent discriminators mean a view has to go out of its way to collapse the four
 * renderings into fewer. `check-agents-render.mjs` asserts the class sets differ between a
 * story with a live run and the same list without one — and that the **stalled** unlit chip (a
 * `doing` task whose role has no run engaged; `agentChip`'s fourth arm carries the boundary)
 * differs from the plain assigned one, because "nobody is coming" rendered identically to
 * "assigned, quietly" is the orphaned-task invisibility the orchestrator's debug notes
 * reported.
 */
export function chipClass(chip: Chip): string {
  if (!chip.lit) {
    return cx(styles.chip, chip.tone === 'attention' ? styles.chipStalled : styles.chipAssigned)
  }
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
 * 320px, and the row already carries a status mark, an id, a title and an agent chip. A word
 * there would either push the title out or wrap the row. The mark carries `title` and
 * `aria-label` with the whole sentence, and the *armed* state is a word — so the destructive
 * label appears exactly when the destructive press is available, which is also the property the
 * render check pins.
 */
/*
 * A bin, not the `×` this used to be: `×` is *close* everywhere else in the app — the tab strip,
 * the pane cluster, every dialog — and a delete that wears the close mark is the one confusion a
 * destructive control cannot afford. Two different actions may not share a picture now that
 * there is a set to draw two from.
 */
export const DELETE_ICON = 'trash-2' as const

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
  /** The row's control is a mark, the card's a word. See [`DELETE_ICON`]. */
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
        /*
         * Red **unarmed** as well as armed. It was `--red` only once it had been pressed, on the
         * theory that arming is what makes it dangerous — but the colour is what tells somebody
         * scanning the foot of the card which of these two buttons is the one that destroys
         * something, and by the time it is armed they have already pressed it. `--red` means
         * "this destroys work" in exactly one place in this app; a delete is that place.
         */
        className={
          compact ? cx(styles.rowDelete) : cx(styles.action, styles.actionDanger)
        }
        data-audit="tasksDelete"
        data-write="true"
        title={DELETE_TITLE}
        aria-label={`Delete ${label}. ${DELETE_TITLE}`}
        onClick={() => onDeleteArm(task)}
      >
        {compact ? <Icon name={DELETE_ICON} size={1} /> : DELETE_LABEL}
      </button>
    )
  }
  /*
   * Armed with no `onDelete`, or neither handler. Nothing is drawn — not a disabled *Confirm
   * delete*, which would be a control that has already taken the user's decision and then cannot
   * act on it. `IntegrateControl` says the same thing; `TaskDetailHost` passes the pair together.
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

/**
 * The change's artifacts, by the schema's id for each, and the file each one *is*. (M28)
 *
 * The file names are printed beside the section headings on purpose. The brand is shown, not
 * hidden: a user who has driven three changes through this card knows the directory layout
 * without ever having been taught it, and everything they learned transfers to the CLI, to the
 * docs and to a teammate's editor. `artifactPath` resolves the real path from what the CLI
 * reported — nothing here joins a filename, because the artifact set is schema-driven.
 */
const SPEC_SECTIONS: readonly { id: string; label: string; file: string }[] = [
  { id: 'proposal', label: 'Proposal', file: 'proposal.md' },
  { id: 'design', label: 'Design', file: 'design.md' },
  { id: 'tasks', label: 'Steps', file: 'tasks.md' },
]

/**
 * The add picker's state: which kind the next link takes. (M30)
 *
 * It used to carry a `target` as well, chosen from a `<select>` and committed by an Add
 * button. The target lives in the search input now (`LinkTargetInput`, whose query is its own
 * transient state, the mention popup's arrangement) and **choosing is the commit** — so the
 * only fact the host must hold is which kind a pick will mean, and the shape says exactly
 * that.
 */
export interface LinkAdd {
  kind: LinkKind
}

export interface TaskDetailProps {
  task: TaskView
  /** Every run cide knows about, so the live-run strip can find this task's. */
  runs: readonly RunRef[]
  /** Agent id → label, for the assignee list and the chip's fallback ladder. */
  roles: Readonly<Record<string, string>>
  /**
   * Why the assignee list is short — `model.ts::assigneeHint`'s sentence, or `null`/absent when
   * the list speaks for itself. Drawn in the assignee editor only: at rest the row already reads
   * honestly (*Unassigned*, or the raw id).
   */
  assigneeHint?: string | null | undefined
  /** The clock, as a prop, so the comment log's ages are deterministic under SSR. */
  nowMs: number
  /**
   * The OpenSpec change this task implements, once it has been read. (M28)
   *
   * **`undefined` means "this task has no spec block"** and is the whole of the optionality
   * claim: with the prop absent the card's markup is identical to what it was before M28, which
   * is what `check:agents-render`'s `task-spec-none` story asserts. `null` is the different
   * statement *there is a change and cide has not read it yet*, which draws the chip and a
   * pending progress row rather than nothing.
   */
  spec?: SpecCardView | null | undefined
  /**
   * Why there is no card, when `spec` is `null` and the read has finished. (M28)
   *
   * Consulted **only** while `spec === null`, so it is not a second source of truth about what
   * the block draws — it is the sentence for one of that state's two outcomes. `null` there means
   * *still reading*; a string means *the read finished and there is nothing*, most often because
   * the change was archived, which is something the user did and should be told about.
   *
   * Without this the two are the same screen, and a failed read is a spinner that never stops.
   */
  specProblem?: string | null | undefined
  /** Press the one prominent action. Absent leaves it drawn but inert with its reason. */
  onSpecPrimary?: ((task: string, action: 'approve' | 'accept') => void) | undefined
  /**
   * `Integrate & Archive` is running. (M28)
   *
   * The button goes inert and says so. It is one press that merges a branch, runs
   * `openspec archive`, re-validates and closes the task — four subprocesses and a git merge, so
   * seconds, sometimes many — and it shipped with no feedback at all: the card sat exactly as it
   * was, which reads as a click that missed. A second press while the first is in flight would
   * be a second merge.
   */
  specBusy?: boolean | undefined
  /**
   * Start OpenSpec's propose workflow for a task that has no change. (M28)
   *
   * **Absent means the button is not drawn**, and that is the optionality claim made
   * structurally: a project with no `openspec/`, one whose board has not been read, one with no
   * propose command installed, and a task that already has a change all see the card exactly as
   * it was before M28. `TaskComposeProps::changes` makes the same claim the same way.
   */
  onProposeChange?: ((task: string) => void) | undefined
  /** Open one of the change's files in an editor pane. */
  onOpenSpecFile?: ((path: string) => void) | undefined
  /**
   * Get back to the conversation this task's work went to, and close the card. (M28)
   *
   * `mode` is the whole of the difference. `open` puts the transcript back on screen and stops;
   * `resume` does that and hands the task to it again, through the same dispatch every other run
   * gets. Both spawn `claude --resume <id>` when nothing is showing the conversation — a
   * `SessionId` *is* what cide passes to `--session-id`, so the id, the transcript and the task's
   * own record of where the work went all stay the same one.
   *
   * Closing the card is part of it rather than a second call: a modal left standing over the pane
   * it just revealed puts the thing the user asked to see behind a scrim.
   */
  onOpenSession?: ((session: string, mode: 'open' | 'resume') => void) | undefined
  /**
   * The requirement editor, when one is open. (M28)
   *
   * `undefined` means no editor and — more importantly — **no pencils**: with it absent the
   * delta cards are exactly the read-only cards they were, which is what a card belonging to a
   * change an agent is holding should be.
   */
  specEdit?: SpecEditView | null | undefined
  /** Open the editor on one requirement, by `data-target`. */
  onSpecEditOpen?: ((target: string) => void) | undefined
  /**
   * Where a run could happen — roles, open conversations, and a fresh one. (M28)
   *
   * Absent means the picker is not offered at all, which is every ordinary task.
   */
  dispatchTargets?: readonly DispatchTarget[] | undefined
  /** Is the picker open? Owned by the host, so this component stays a function of its props. */
  dispatchOpen?: boolean | undefined
  onDispatchOpen?: ((open: boolean) => void) | undefined
  /** Chosen. The card never decides *how* a target starts — see `DispatchTarget`. */
  onDispatchTo?: ((task: string, target: DispatchTarget) => void) | undefined
  /**
   * The card's link chips, both directions, from `model.ts::taskLinks`. (M30)
   *
   * **Absent draws no Links section at all** — the `spec` prop's optionality claim, restated:
   * with this and `onLink` absent the card's markup is identical to what it was before M30,
   * which is what the render check's unchanged stories assert. Present-but-empty draws the
   * section with only the add affordance, because a card that *can* link should say so.
   */
  links?: readonly LinkChip[] | undefined
  /** What the add picker offers — every other task, in panel order (`linkableTargets`). */
  linkTargets?: readonly LinkTargetOption[] | undefined
  /**
   * The add editor's state, or `null`/absent for at rest. Controlled by the host (like
   * `editing`) rather than a `useState` here, so the render check can draw the open picker as
   * a story.
   */
  linkAdd?: LinkAdd | null | undefined
  /** Set or clear the add editor's state. */
  onLinkAdd?: ((state: LinkAdd | null) => void) | undefined
  /** Write one edge. The host maps it onto `TaskEdit::Link`. */
  onLink?: ((task: string, kind: LinkKind, target: string) => void) | undefined
  /** Tombstone one edge. `kind` is the chip's own, verbatim — the store names the refusals. */
  onUnlink?: ((task: string, kind: string, target: string) => void) | undefined
  /** Open another task's card — a link chip's click. The host moves `selected`. */
  onOpenTask?: ((task: string) => void) | undefined
  /**
   * The one field in edit and what has been typed into it, or `null` for a fully read-only card.
   *
   * `TaskDetailHost` owns it and `model.ts::activeEdit` has already refused an edit whose task
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
  /**
   * A new comment — with, since M39, the files the composer had staged. The host lands text
   * alone through `TaskEdit::Comment` and text-with-files through `task_attach`'s `newComment`
   * target, so a comment and its screenshots are one mutation.
   */
  onAddComment?:
    | ((task: string, text: string, attachments: readonly StagedAttachment[]) => void)
    | undefined
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
  /*
   * Attachments. (M39) All optional, and every story written before M39 passes none of them, so
   * a card handed no attachment props draws exactly what it drew — which is how the render check
   * knows the strip appears only where a file is.
   */
  /** Thumbnail state by attachment id; the host's `attachmentPreviews` snapshot. */
  previews?: Readonly<Record<string, AttachmentPreview>> | undefined
  /** The native picker. Empty means cancelled. */
  onPickAttachments?: (() => Promise<readonly StagedAttachment[]>) | undefined
  /** The clipboard's image, staged to a file for the composer. `null` when it holds none. */
  onStageClipboard?: (() => Promise<StagedAttachment | null>) | undefined
  onAttach?:
    | ((task: string, target: AttachTargetView, sources: readonly StagedAttachment[]) => void)
    | undefined
  /** The clipboard's image straight onto the body or a comment — a paste on the card. */
  onAttachClipboard?: ((task: string, target: AttachTargetView) => void) | undefined
  onDetachAttachment?: ((task: string, attachment: string) => void) | undefined
  onOpenAttachment?: ((task: string, attachment: string) => void) | undefined
  onRevealAttachment?: ((task: string, attachment: string) => void) | undefined
  onViewAttachment?: ((task: string, attachment: string) => void) | undefined
  /** The composer's staged files. Controlled by the host, so a drop from the desktop can add
   *  to them from outside the card. */
  composerStaged?: readonly StagedAttachment[] | undefined
  onComposerStaged?: ((staged: readonly StagedAttachment[]) => void) | undefined
  /** The `data-attach-drop` key under a desktop drag right now, or `null`. */
  dropHot?: string | null | undefined
}

const NO_STAGED: readonly StagedAttachment[] = []

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
 * The card, in its dialog. **This is what the app mounts**; `TaskDetailHost` renders it from
 * `App.tsx`, beside the list rather than in place of it — and outside the sidebar branches, so
 * a task opened from the Agents panel appears without switching panels. The board stays on
 * screen behind the scrim whenever the Tasks panel is the view.
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
    spec,
    specProblem,
    onSpecPrimary,
    specBusy,
    onProposeChange,
    onOpenSpecFile,
    onOpenSession,
    specEdit,
    onSpecEditOpen,
    links,
    linkTargets,
    linkAdd,
    onLinkAdd,
    onLink,
    onUnlink,
    onOpenTask,
    dispatchTargets,
    dispatchOpen,
    onDispatchOpen,
    onDispatchTo,
    previews,
    onPickAttachments,
    onStageClipboard,
    onAttach,
    onAttachClipboard,
    onDetachAttachment,
    onOpenAttachment,
    onRevealAttachment,
    onViewAttachment,
    composerStaged = NO_STAGED,
    onComposerStaged,
    dropHot = null,
  } = props

  const chip = agentChip(task, runs, roles)
  // The picker-then-attach road, shared by the body's button and a comment's Attach.
  // `undefined` when either half is missing, which is what withholds every button.
  const pickInto =
    onPickAttachments !== undefined && onAttach !== undefined
      ? (target: AttachTargetView) =>
          void onPickAttachments().then((picked) => {
            if (picked.length > 0) onAttach(task.id, target, picked)
          })
      : undefined
  const taskDropKey = dropTargetKey({ kind: 'task', task: task.id })

  /*
   * Which change this card is looking at, whether or not it has been read. (M28)
   *
   * `spec === undefined` is *this task has no change* and must stay indistinguishable from a
   * pre-M28 card — that is the whole optionality claim. Anything else means there is a change, so
   * the name comes off the task itself while `spec` is still `null`, and off the card once it
   * arrives. It was read only off the card, so a task with a change drew **nothing at all** until
   * four subprocesses had answered: the modal opened with no chip, no block and nothing on screen
   * saying OpenSpec was being read, and then a whole section appeared out of nowhere.
   */
  const specChange = spec === undefined ? null : (spec?.change ?? task.change)
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
      data-attach-drop={taskDropKey}
      data-drop-hot={dropHot === taskDropKey ? 'true' : undefined}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.stopPropagation()
        run(closeCard(task, editing, 'escape'))
      }}
      onPaste={(event) => {
        /*
         * A screenshot pasted anywhere on the card attaches to the task. (M39) The composer's
         * textarea claims its own paste first (and stops it here) so a screenshot meant for a
         * comment is staged with the comment; everything else — the body at rest, the body
         * editor, a comment's editor — is the task's. `wantsImagePaste` is the editor's own
         * detector: a copied file path carries text as well and must go on pasting as text.
         */
        if (onAttachClipboard === undefined) return
        if (!wantsImagePaste([...event.clipboardData.types])) return
        event.preventDefault()
        onAttachClipboard(task.id, { kind: 'task' })
      }}
    >
      <div className={styles.cardHead} data-audit="taskCardHead">
        <span
          className={cx(styles.glyph, TONE_CLASS[statusTone(task.status)])}
          data-audit="tasksDetailGlyph"
          aria-hidden="true"
        >
          {/* Through `Icon`/`asIcon`, exactly as the list row draws it — the raw table value is
              a *name* (`circle-dot`), and rendering it as text prints the name. */}
          <Icon name={asIcon(statusGlyph(task.status))} size={1} />
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
          * The change this task implements. (M28)
          *
          * Beside the creator and **not** as a `FieldRow`, for the creator's own reason: it is an
          * unchangeable fact about *which* task this is rather than a value the card offers to
          * edit, and a row with no pencil in a column of rows that all have one reads as a
          * broken affordance. Linking and unlinking are the compose dialog's and the agent's;
          * this is the card saying what it is looking at.
          */}
        {specChange !== null && (
          <span className={styles.specChip} data-audit="taskSpecChip" data-change={specChange}>
            openspec: {specChange}
          </span>
        )}
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
          <Icon name="x" size={1} />
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
          {/*
            * The status log: who moved this task, when, from where to where. (M27)
            *
            * A native `<details>`, **collapsed by default** — it is the record you need rarely
            * and must be able to trust absolutely when you do, so it costs one quiet line under
            * the segment until it is asked for. Uncontrolled on purpose: whether a disclosure
            * is open is exactly the transient gesture state the webview is allowed to own, and
            * a `useState` here would be reachable from no fixture while buying nothing.
            *
            * Drawn only when there is history to show. An empty disclosure would be a control
            * that opens onto nothing, and a task that never moved has nothing to audit — the
            * segment above already says where it is, and the head says who created it.
            */}
          {task.history.length > 0 && (
            <details className={styles.history} data-audit="tasksHistory">
              <summary className={styles.historySummary} data-audit="tasksHistorySummary">
                Status history ({task.history.length})
              </summary>
              <div className={styles.historyRows}>
                {historyOrder(task).map((change, index) => (
                  <div
                    className={styles.historyRow}
                    data-audit="tasksHistoryRow"
                    key={`${change.atMs}:${index}`}
                  >
                    <span className={styles.historyTime}>
                      {clock(change.atMs)} ({elapsed(nowMs, change.atMs)} ago)
                    </span>
                    {/* Through `statusLabel`, never raw: a status a future cide wrote renders
                        as `Unknown` rather than as an unreadable token or a throw. */}
                    <span className={styles.historyMove}>
                      {statusLabel(change.from)} → {statusLabel(change.to)}
                    </span>
                    <span className={styles.historyBy}>{authorLabel(change.by)}</span>
                  </div>
                ))}
              </div>
            </details>
          )}
        </div>

        {/*
          * Hidden on an unapproved OpenSpec task. (M28)
          *
          * There the assignee is not a field somebody fills in — it is what Approve & dispatch
          * *sets*, and leaving the row here would be a second road to the same fact that skips
          * the one choice that matters: whether this work goes to a role, to a conversation
          * already open, or to a new one. Once something is assigned the row returns, so it can
          * still be changed the ordinary way. An ordinary task has no Approve button and is
          * unaffected — `showsAssignee` is the whole rule.
          */}
        {showsAssignee(spec, task.agent !== null) && (
          <FieldRow field="assignee" task={task} roles={roles} editing={editing} run={run} props={props} />
        )}
        <FieldRow field="body" task={task} roles={roles} editing={editing} run={run} props={props} />

        {/*
          * Files on the body, as a section under it. (M39) Drawn when there are files, or when
          * the host can add one — so a card that can attach shows the paperclip even on a task
          * with nothing attached, and a read-only card (a build with no handler, a board that
          * cannot be written) shows the strip only when there is something in it. The strip is
          * its own component and never goes through `TaskMarkdown`: a thumbnail is an `<img>`
          * over a URL Rust vouched for, which that renderer's no-IPC rule cannot produce.
          */}
        {(task.attachments.length > 0 || pickInto !== undefined) && (
          <div className={styles.field} data-audit="taskAttachmentsField">
            <div className={styles.fieldHead}>
              <span className={styles.fieldLabel}>Attachments</span>
              {pickInto !== undefined && (
                <button
                  type="button"
                  className={styles.fieldEdit}
                  data-audit="tasksAttachButton"
                  data-write="true"
                  title="Attach files"
                  aria-label={`Attach files to ${task.id}`}
                  onClick={() => pickInto({ kind: 'task' })}
                >
                  <Icon name="paperclip" size={1} />
                </button>
              )}
            </div>
            <AttachmentStrip
              task={task.id}
              attachments={task.attachments}
              previews={previews}
              onView={onViewAttachment}
              onOpen={onOpenAttachment}
              onReveal={onRevealAttachment}
              onDetach={onDetachAttachment}
            />
          </div>
        )}

        {/*
          * Typed links, as a **section** — deliberately not a fifth `FieldRow`. (M30)
          *
          * A field is one value with one pencil; an edge set is add-and-remove, with no draft
          * and no single commit — the argument is written beside `TASK_FIELDS`, whose vocabulary
          * this section leaves untouched. After the body because a link is context for reading
          * the statement above it; before the spec block and the run strip, which are about the
          * change and the process rather than about which tasks this one stands in relation to.
          *
          * A chip is a `<button>` that *navigates* — `onOpenTask` moves the card to the target —
          * and its ✕ is the write. The ✕ is omitted on a directed *incoming* chip: that edge is
          * stored on (and belongs to) the other task, and the chip itself is the road there. A
          * `related` chip keeps its ✕ from either end — the store looks on both.
          */}
        {links !== undefined && (links.length > 0 || onLink !== undefined) && (
          <div className={styles.field} data-audit="taskLinks">
            <div className={styles.fieldHead}>
              <span className={styles.fieldLabel}>Links</span>
              {/*
                * No `data-write`, the pencil's own rule: at rest this opens a picker and writes
                * nothing. Same element in both states, so the control keeps its place.
                */}
              {onLink !== undefined && (
                <button
                  type="button"
                  className={styles.fieldEdit}
                  data-audit={linkAdd == null ? 'taskLinkAddOpen' : 'taskLinkAddCancel'}
                  title={linkAdd == null ? 'Link to another task' : 'Stop adding a link'}
                  aria-label={
                    linkAdd == null
                      ? `Link ${task.id} to another task`
                      : `Cancel linking ${task.id}`
                  }
                  onClick={() => onLinkAdd?.(linkAdd == null ? { kind: 'related' } : null)}
                >
                  {linkAdd == null ? 'Link…' : 'Cancel'}
                </button>
              )}
            </div>
            {links.length > 0 && (
              <div className={styles.linkChips} data-audit="taskLinkChips">
                {links.map((chip) => (
                  <span
                    className={styles.linkPair}
                    key={`${chip.kind}:${chip.direction}:${chip.target}`}
                  >
                    <button
                      type="button"
                      className={cx(styles.linkChip, chip.gone ? styles.linkChipGone : undefined)}
                      data-audit="taskLinkChip"
                      data-kind={chip.kind}
                      data-direction={chip.direction}
                      data-target={chip.target}
                      data-gone={chip.gone ? 'true' : 'false'}
                      /*
                       * The title resolves the target for a hover; a `gone` chip says so instead.
                       * The chip's own text stays `label id` — a 320px panel cannot afford the
                       * target's whole title inline, and the id is what agents quote anyway.
                       */
                      title={
                        chip.gone
                          ? `${chip.target} is no longer on the board`
                          : `${chip.target}: ${chip.targetTitle?.trim() !== '' ? chip.targetTitle : 'Untitled'}`
                      }
                      aria-label={`${chip.label} ${chip.target} — open it`}
                      onClick={() => onOpenTask?.(chip.target)}
                    >
                      {chip.label} {chip.target}
                      {chip.gone ? ' (gone)' : ''}
                    </button>
                    {/* No ✕ on a kind this build cannot spell on the wire — an unlink the store
                        would refuse is a dead control; the chip itself still draws and still
                        navigates. */}
                    {onUnlink !== undefined &&
                      isLinkKind(chip.kind) &&
                      (chip.direction === 'out' || chip.kind === 'related') && (
                        <button
                          type="button"
                          className={styles.linkRemove}
                          data-audit="taskLinkRemove"
                          data-write="true"
                          title={`Remove this ${chip.label.toLowerCase()} link`}
                          aria-label={`Unlink ${chip.target} from ${task.id}`}
                          onClick={() => onUnlink(task.id, chip.kind, chip.target)}
                        >
                          <Icon name="x" size={0} />
                        </button>
                      )}
                  </span>
                ))}
              </div>
            )}
            {linkAdd != null && onLink !== undefined && (
              <div className={styles.linkAdd} data-audit="taskLinkAdd">
                <select
                  className={cx(styles.select, styles.linkKind)}
                  data-audit="taskLinkKind"
                  aria-label="Link kind"
                  value={linkAdd.kind}
                  onChange={(event) => {
                    const kind = event.target.value
                    // Through the guard, never a cast: the DOM hands back a string, and the
                    // options below are the only legal ones — `isTaskStatus`'s posture.
                    if (isLinkKind(kind)) onLinkAdd?.({ kind })
                  }}
                >
                  {LINK_KINDS.map((kind) => (
                    <option key={kind} value={kind}>
                      {linkLabel(kind, 'out')}
                    </option>
                  ))}
                </select>
                {/*
                  * The target, by search — id-or-title autocomplete, and picking a row IS the
                  * write (`LinkTargetInput`'s header carries the argument). `data-write` sits on
                  * the input because a pick from it commits an edge, the same claim the old
                  * target `<select>` + Add pair made in two controls.
                  */}
                <LinkTargetInput
                  targets={linkTargets ?? []}
                  listboxId={`task-link-targets-${task.id}`}
                  data-audit="taskLinkTarget"
                  data-write="true"
                  aria-label="Link target"
                  autoFocus
                  onPick={(target) => {
                    onLink(task.id, linkAdd.kind, target)
                    onLinkAdd?.(null)
                  }}
                  onDismiss={() => onLinkAdd?.(null)}
                />
              </div>
            )}
          </div>
        )}

        {/*
          * The live-run strip: what is running against this task right now, from the same
          * `agentChip` call the list row makes, so the two cannot disagree about which of the
          * three states this task is in. Drawn only when a run is genuinely live — an assignment
          * alone is the assignee row above, not a strip claiming activity.
          */}
        {/*
          * The change, in full. (M28)
          *
          * **After the body and before the run strip**, and the order is argued: the run strip is
          * *what is happening right now*, while this is *what the work is against* — which is
          * context for reading the body immediately above it. Putting it below the strip would
          * separate a task's statement from its specification with a line about process.
          *
          * The whole block is behind `spec === undefined`, which is what keeps a task with no
          * change rendering byte for byte as it did before M28.
          */}
        {/*
          * The block before it has been read — a state, not an absence. (M28)
          *
          * Reading a change is four `openspec` invocations, and each one is a node process: it is
          * about six tenths of a second even now that they run at once, and it was two and a half
          * before. That whole time the card drew nothing where the block would be, so a task with
          * a change opened looking like a task without one, and then grew a section.
          *
          * The bar is drawn here too, empty and `aria-valuenow` absent rather than `0`: a bar
          * reporting zero percent is a *claim about the checklist*, and this state has not read
          * it. `aria-busy` is what tells a screen reader the difference between an empty bar and
          * a bar at nothing.
          */}
        {spec === null && specChange !== null && (
          <div
            className={styles.field}
            data-audit="taskSpecBlock"
            data-state={specProblem == null ? 'reading' : 'failed'}
          >
            {specProblem == null ? (
              <>
                <p className={styles.specPending} data-audit="specPending">
                  Reading {specChange} from OpenSpec…
                </p>
                <div
                  className={styles.specBar}
                  data-audit="specProgress"
                  role="progressbar"
                  aria-busy="true"
                  aria-valuemin={0}
                  aria-valuemax={100}
                />
              </>
            ) : (
              /*
               * The read finished and there is nothing. Most often the change was archived, which
               * is a thing the user did — so it is a sentence, not a spinner that never stops.
               * The `.catch` in the host is what makes this reachable at all; without one the
               * promise rejected into nowhere and the card sat on "Reading…" for ever.
               */
              <p className={styles.specPending} data-audit="specFailed">
                {specProblem}
              </p>
            )}
          </div>
        )}
        {spec != null && (
          <div
            className={styles.field}
            data-audit="taskSpecBlock"
            data-state={spec.archived == null ? 'ready' : 'archived'}
          >
            <div className={styles.specSummary}>
              <span
                className={styles.specValidity}
                data-audit="specValidate"
                data-state={validityLabel(spec).state}
              >
                {validityLabel(spec).label}
              </span>
              <span className={styles.specProgressLabel} data-audit="specProgressLabel">
                {progressLabel(spec)}
              </span>
            </div>
            {/*
              * A real `progressbar`, because a bar that is only a coloured div tells a screen
              * reader nothing — and this one is the main thing on the card that moves while an
              * agent works.
              *
              * **Not drawn for an archived change.** Its checklist is not recoverable from the
              * directory — which file is the checklist and what counts as an item are the
              * schema's business — so the numbers come back `0/0`, and an empty bar over work
              * that is finished and merged is a picture that is wrong rather than absent. The
              * summary line above says *archived as …* in its place.
              */}
            {spec.archived == null && (
            <div
              className={styles.specBar}
              data-audit="specProgress"
              role="progressbar"
              aria-valuenow={progressPercent(spec)}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              {/*
                * The scale is inline because typed `attr()` is not implemented here — see
                * `.specBarFill`. `data-pct` is what the digest reads.
                */}
              <span
                className={styles.specBarFill}
                data-pct={progressPercent(spec)}
                style={{ transform: `scaleX(${progressPercent(spec) / 100})` }}
              />
            </div>
            )}

            {/*
              * Where the work went, when it went to a conversation. (M28)
              *
              * This row *is* what stands in place of Approve & dispatch: `primaryAction` returns
              * `none` once `Task::session` is set, so a card that drew neither would be a card
              * with nothing to do and nothing to look at. Choosing *New Claude session* used to
              * leave the button standing over a conversation that was already working, which is
              * an invitation to dispatch the same task twice.
              *
              * Three states and not two — see `sessionState`. *Closed* is not a quieter shade of
              * *not waiting*: it is work with nowhere to continue, and the only one of the three
              * that names a next step.
              *
              * The role path is untouched by all of this. A task assigned to a role has no
              * session — Rust clears one when the other is set — so this row is absent and the
              * run strip below is what reports the subagent, exactly as before.
              */}
            {spec.session !== null && (
              <div
                className={styles.specSession}
                data-audit="specSession"
                data-state={sessionState(spec.session, spec.archived !== null).state}
              >
                <span className={styles.specSessionState}>
                  {sessionState(spec.session, spec.archived !== null).label}
                </span>
                <span className={styles.specSessionName}>{spec.session.label}</span>
                {/*
                  * Drawn whether or not a pane still holds it, which is the fix. (M28)
                  *
                  * It used to be withheld once the pane was gone, on the reasoning that a
                  * control opening onto nothing is what this card refuses — and that left a
                  * closed conversation with **no way to proceed at all** except handing the work
                  * somewhere else, which throws away everything it had already worked out. The
                  * premise was wrong: a closed conversation is not nothing. Its transcript is on
                  * disk under the id cide passed to `--session-id`, and `claude --resume <id>`
                  * brings all of it back.
                  *
                  * Two controls, and the difference is one nudge: **Open** puts it back on screen
                  * and stops, **Resume** puts it back and hands the task to it again through the
                  * same dispatch every other run gets. Resume leads, because on a conversation
                  * somebody deliberately closed it is the one that carries the work forward.
                  */}
                {/*
                  * Both write controls go once the change is archived. (M31)
                  *
                  * **Resume** hands the task to the conversation again through the same dispatch
                  * every other run gets, and **Hand it elsewhere** dispatches it somewhere new —
                  * on work that is merged into `openspec/specs/` and whose change directory has
                  * moved, each of them starts a run against a change that is not there any more.
                  * `primaryAction` already refuses this arm (`id: 'none'`, *this change has been
                  * archived*); the row is the second door onto the same gesture and had none of
                  * that reasoning, so it kept offering both under the card's own *Archived* line.
                  *
                  * **Open** stays, and it is the reason the row stays at all: which conversation
                  * did this work is exactly the thing worth keeping a record of, and reading it
                  * back writes nothing.
                  */}
                {onOpenSession !== undefined && spec.archived === null && (
                  <button
                    type="button"
                    className={styles.specSessionResume}
                    data-audit="specSessionResume"
                    data-write="true"
                    title={
                      spec.session.open
                        ? 'Show this conversation and hand it this task again'
                        : 'Bring this conversation back with `claude --resume` and hand it this task again'
                    }
                    onClick={() => onOpenSession(spec.session?.id ?? '', 'resume')}
                  >
                    Resume
                  </button>
                )}
                {onOpenSession !== undefined && (
                  <button
                    type="button"
                    className={styles.specSessionOpen}
                    data-audit="specSessionOpen"
                    title={
                      spec.session.open
                        ? 'Show this conversation and close this card'
                        : 'Bring this conversation back with `claude --resume`, without asking it for anything'
                    }
                    onClick={() => onOpenSession(spec.session?.id ?? '', 'open')}
                  >
                    {spec.session.open ? 'Open' : 'Reopen'}
                  </button>
                )}
                {/*
                  * The way back out, and the reason removing the button does not trap anybody:
                  * the same picker Approve opens, reachable from the row that replaced it. It
                  * matters most in the `closed` state, where there is no conversation left to
                  * carry on in.
                  */}
                {dispatchTargets !== undefined && spec.archived === null && (
                  <button
                    type="button"
                    className={styles.specSessionElsewhere}
                    data-audit="specSessionElsewhere"
                    data-write="true"
                    aria-expanded={dispatchOpen === true}
                    title="Hand this task to a role, another conversation, or a new one"
                    onClick={() => onDispatchOpen?.(dispatchOpen !== true)}
                  >
                    Hand it elsewhere
                  </button>
                )}
              </div>
            )}
            {spec.session !== null && (
              <p className={styles.specHint} data-audit="specSessionHint">
                {sessionHint(spec.session, spec.archived !== null)}
              </p>
            )}

            {/*
              * Four disclosures, all shut. `open` is never set, `tasksHistory`'s rule: a card
              * that opens with four expanded documents in it is a card nobody can see the log of.
              * Each header names the file it is, which is how the vocabulary is taught.
              */}
            {SPEC_SECTIONS.map((section) => {
              const path = artifactPath(spec, section.id)
              // Drawn only when the artifact exists. The set is decided by the workflow schema in
              // `openspec/config.yaml`, so a project legitimately need not have a `design.md` —
              // and a disclosure onto nothing is a control that does nothing.
              if (path === null && section.id !== 'tasks') return null
              return (
                <details
                  key={section.id}
                  className={styles.specSection}
                  data-audit="specSection"
                  data-section={section.id}
                >
                  <summary className={styles.specSummaryRow} data-audit="specSectionSummary">
                    {section.label}
                    <span className={styles.specSectionFile}>{section.file}</span>
                  </summary>
                  {section.id === 'tasks' ? (
                    <ul className={styles.specTasks}>
                      {spec.tasks.map((item, index) => (
                        <li
                          key={`${index}:${item.description}`}
                          className={styles.specTaskRow}
                          data-audit="specTaskRow"
                          data-done={item.done ? 'true' : 'false'}
                        >
                          <Icon name={asIcon(item.done ? 'check' : 'square')} size={1} />
                          <span>{item.description}</span>
                        </li>
                      ))}
                      {/*
                        * Read-only, and one button out to the file. Ticking a box here would
                        * write into a file a live agent may be holding — and progress is a thing
                        * to read, not a control.
                        */}
                      {path !== null && (
                        <li className={styles.specTaskRow}>
                          <button
                            type="button"
                            className={styles.specOpen}
                            data-audit="specOpenFile"
                            onClick={() => onOpenSpecFile?.(path)}
                          >
                            Open {section.file}
                          </button>
                        </li>
                      )}
                    </ul>
                  ) : (
                    <button
                      type="button"
                      className={styles.specOpen}
                      data-audit="specOpenFile"
                      onClick={() => {
                        if (path !== null) onOpenSpecFile?.(path)
                      }}
                    >
                      Open {section.file}
                    </button>
                  )}
                </details>
              )
            })}

            <details className={styles.specSection} data-audit="specSection" data-section="deltas">
              <summary className={styles.specSummaryRow} data-audit="specSectionSummary">
                What this changes
                <span className={styles.specSectionFile}>specs/</span>
              </summary>
              {spec.deltas.map((delta) =>
                delta.requirements.map((requirement) => (
                  <div
                    key={`${delta.spec}:${delta.op}:${requirement.name}`}
                    className={styles.specDelta}
                    data-audit="specDeltaCard"
                    data-op={delta.op}
                    data-spec={delta.spec}
                  >
                    <div className={styles.specDeltaHead}>
                      <span className={styles.specDeltaOp}>{delta.op}</span>
                      <span className={styles.specDeltaName}>{requirement.name}</span>
                      <span className={styles.specDeltaSpec}>{delta.spec}</span>
                      {specEdit != null && onSpecEditOpen !== undefined && (
                        <button
                          type="button"
                          className={styles.specOpen}
                          data-audit="specEditOpen"
                          data-target={requirement.target}
                          data-write="true"
                          title="Edit this requirement"
                          onClick={() => onSpecEditOpen(requirement.target)}
                        >
                          <Icon name={asIcon('pencil')} size={1} label="Edit" />
                        </button>
                      )}
                    </div>
                    {specEdit?.target === requirement.target && specEdit.draft !== null ? (
                      <RequirementEditor
                        draft={specEdit.draft}
                        busy={specEdit.busy}
                        problem={specEdit.problem}
                        onDraft={specEdit.onDraft}
                        onSave={specEdit.onSave}
                        onCancel={specEdit.onCancel}
                      />
                    ) : (
                      <>
                        <TaskMarkdown text={requirement.text} />
                        {requirement.scenarios.map((scenario, index) => (
                          <div
                            key={`${index}:${scenario.title}`}
                            className={styles.specScenario}
                            data-audit="specScenario"
                            data-title={scenario.title}
                          >
                            <p className={styles.specScenarioTitle}>{scenario.title}</p>
                            <TaskMarkdown text={scenario.body} />
                          </div>
                        ))}
                        {/*
                          * The issues this requirement caused, beside it. Never a toast: an
                          * error about a paragraph belongs next to the paragraph, where the
                          * thing to change is already on screen.
                          */}
                        {requirement.issues.map((issue, index) => (
                          <p key={index} className={styles.specIssue} data-audit="specIssue">
                            {issue}
                          </p>
                        ))}
                      </>
                    )}
                  </div>
                )),
              )}
            </details>
          </div>
        )}
        {chip !== null && chip.lit && (
          <div className={styles.runStrip} data-audit="tasksRunStrip">
            <span className={chipClass(chip)} data-audit="tasksStripChip" data-lit="true">
              <span
                /* `phaseGlyph` returns an icon *name* (`loader-circle`), not a drawable
                   character — same as the list row's chip, it must go through `Icon`. And the
                   spinner turns, which it did not: `model.ts::SPINNING_GLYPH`. */
                className={cx(
                  styles.chipDot,
                  glyphSpins(phaseGlyph((chip.phase ?? '') as RunPhase)) && styles.chipSpin,
                )}
                aria-hidden="true"
              >
                <Icon name={asIcon(phaseGlyph((chip.phase ?? '') as RunPhase))} size={0} />
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
                  data-attach-drop={dropTargetKey({ kind: 'comment', task: task.id, comment: comment.id })}
                  data-drop-hot={
                    dropHot === dropTargetKey({ kind: 'comment', task: task.id, comment: comment.id })
                      ? 'true'
                      : undefined
                  }
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
                    {/* The wall clock first, the age in brackets — `clock`'s doc carries why
                        both are on the line. One span, so the head still reads as one
                        right-aligned fact. */}
                    <span className={styles.logTime}>
                      {clock(comment.atMs)} ({elapsed(nowMs, comment.atMs)} ago)
                    </span>
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
                      <MentionTextarea
                        className={styles.composerText}
                        roles={roles}
                        listboxId={`comment-edit-mentions-${comment.id}`}
                        tools
                        name="text"
                        defaultValue={comment.text}
                        rows={3}
                        autoFocus
                        onKeyDown={(event) => {
                          // Only for keys the mention popup declined — its own Escape closes
                          // the popup, not this editor.
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
                      {/* Rendered as markdown — through the AST and React's own escaping, never
                          `dangerouslySetInnerHTML`; the file header carries what changed. A
                          `<div>` now, because the rendering is blocks and a `<p>` cannot hold
                          them. */}
                      <div className={styles.logText} data-audit="tasksCommentText">
                        <TaskMarkdown text={comment.text} />
                      </div>
                      <AttachmentStrip
                        task={task.id}
                        attachments={comment.attachments}
                        previews={previews}
                        onView={onViewAttachment}
                        onOpen={onOpenAttachment}
                        onReveal={onRevealAttachment}
                        onDetach={onDetachAttachment}
                      />
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
                      {(onEditComment !== undefined ||
                        onDeleteComment !== undefined ||
                        pickInto !== undefined) && (
                        <div className={styles.logActions} data-audit="tasksCommentActions">
                          {pickInto !== undefined && (
                            <button
                              type="button"
                              className={styles.logAction}
                              data-audit="tasksCommentAttach"
                              data-write="true"
                              aria-label={`Attach files to this comment on ${task.id}`}
                              onClick={() => pickInto({ kind: 'comment', id: comment.id })}
                            >
                              Attach
                            </button>
                          )}
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
            data-attach-drop={dropTargetKey({ kind: 'composer', task: task.id })}
            data-drop-hot={
              dropHot === dropTargetKey({ kind: 'composer', task: task.id }) ? 'true' : undefined
            }
            onSubmit={(event) => {
              event.preventDefault()
              const form = event.currentTarget
              const text = String(new FormData(form).get('text') ?? '').trim()
              // An empty comment is not a comment. Appending one to a file the whole team reads
              // would be a line in the log saying nothing, and the log is append-only. A
              // comment that is nothing but its files — "here is the screenshot" — is not
              // empty. (M39)
              if (text === '' && composerStaged.length === 0) return
              onAddComment(task.id, text, composerStaged)
              onComposerStaged?.([])
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
              * `MentionTextarea` keeps it that way — an accepted mention is written into
              * `el.value` directly, which is exactly what the form's FormData reads.
              */}
            <MentionTextarea
              id={`task-comment-${task.id}`}
              className={styles.textarea}
              data-audit="tasksCommentField"
              data-write="true"
              roles={roles}
              listboxId={`task-comment-mentions-${task.id}`}
              tools
              name="text"
              defaultValue=""
              onPaste={(event) => {
                // A screenshot pasted into the composer is staged *with* the comment rather than
                // attached to the task at once: it belongs to the report being written. Claimed
                // here and stopped, so the card's own paste handler does not also take it.
                if (onStageClipboard === undefined || onComposerStaged === undefined) return
                if (!wantsImagePaste([...event.clipboardData.types])) return
                event.preventDefault()
                event.stopPropagation()
                void onStageClipboard().then((staged) => {
                  if (staged !== null) onComposerStaged([...composerStaged, staged])
                })
              }}
            />
            {onComposerStaged !== undefined && (
              <StagedChips
                staged={composerStaged}
                onRemove={(path) =>
                  onComposerStaged(composerStaged.filter((file) => file.path !== path))
                }
              />
            )}
            <div className={styles.composerActions}>
              {onPickAttachments !== undefined && onComposerStaged !== undefined && (
                <button
                  type="button"
                  className={styles.action}
                  data-audit="tasksComposerAttach"
                  data-write="true"
                  title="Attach files to this comment"
                  onClick={() =>
                    void onPickAttachments().then((picked) => {
                      if (picked.length > 0) onComposerStaged([...composerStaged, ...picked])
                    })
                  }
                >
                  <Icon name="paperclip" size={1} /> Attach…
                </button>
              )}
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
        {/*
          * The one prominent thing to do about this change, at the **bottom** of the card. (M28)
          *
          * It sat inside the spec block, in the middle, which put the card's most consequential
          * control between two disclosures and above the conversation it is usually decided
          * from. The eye reaches a decision last, after reading the statement of the work, its
          * checklist and whatever the log says about it — so that is where the button goes.
          *
          * Above the delete row and never below it: `DeleteControl` stays last for its own
          * stated reason, that a destructive control the hand reaches on the way to something
          * else is the thing this card refuses.
          *
          * Only for an OpenSpec task. `spec == null` is every ordinary one, and there the whole
          * bar is absent rather than disabled — approving a change is a gesture about a change.
          */}
        {/*
          * One row at the foot of the card: what to do about this change, and what to do about
          * the task. (M28)
          *
          * They were two bordered blocks stacked — each drawing its own rule, so a card with no
          * change on it still showed the second rule with a lone right-aligned Delete under it,
          * which read as a stray line. One row, one rule.
          *
          * Approve sits at the *left* and Delete stays hard right: they are the two ends of what
          * can be done here, and a destructive control the hand reaches on its way to something
          * else is what this card refuses.
          */}
        <div className={styles.detailActions} data-audit="tasksDetailActions">
          {spec != null && spec.action.id !== 'none' && (
            <button
              type="button"
              className={cx(
                styles.action,
                spec.action.enabled && styles.actionPrimary,
                specBusy === true && styles.actionBusy,
              )}
              data-audit="specPrimary"
              data-action={spec.action.id}
              data-write="true"
              disabled={!spec.action.enabled || specBusy === true}
              data-busy={specBusy === true ? 'true' : undefined}
              aria-busy={specBusy === true ? true : undefined}
              aria-expanded={spec.action.id === 'approve' ? dispatchOpen === true : undefined}
              title={spec.action.enabled ? spec.action.hint : spec.action.reason}
              onClick={() => {
                // Approve opens the picker rather than assigning: *where* the run happens is the
                // choice this gesture exists to make. Accept has nothing to choose.
                if (spec.action.id === 'approve') {
                  onDispatchOpen?.(dispatchOpen !== true)
                  return
                }
                if (spec.action.id === 'accept') onSpecPrimary?.(task.id, 'accept')
              }}
            >
              {/*
                * A turning spinner and a present participle while it runs, not a frozen label.
                *
                * `Integrate & Archive` merges a branch, archives the change, re-validates and
                * closes the task — and it shipped drawing nothing at all while it did, so the
                * only honest reading of the card was that the click had missed.
                *
                * The mark **rotates**, through `.actionSpinner`. The first version put the same
                * `loader-circle` here that the run strip draws, on the reasoning that a user has
                * already learnt what it means there — which was wrong twice: nothing in cide was
                * animating that mark, so it read as a static shape rather than as progress, and
                * it was wrapped in `.chipDot`, a run-strip class with no layout of its own, so
                * the button collapsed around a baseline-aligned svg. See `.actionBusy`.
                */}
              {specBusy === true && (
                <Icon
                  name={asIcon('loader-circle')}
                  size={1}
                  className={styles.actionSpinner ?? ''}
                />
              )}
              {specBusy === true ? busyLabel(spec.action.id) : spec.action.label}
            </button>
          )}
          {/*
            * The road *into* OpenSpec for a task that is not one. (M28)
            *
            * Drawn only when the host passes a handler, which it does only for a project whose
            * board is ready and whose `.claude/` has a propose command — the `spec` prop's own
            * optionality rule. This replaced the compose dialog's *New change from this task*,
            * which scaffolded a stub `openspec validate` refuses; writing a proposal needs the
            * codebase, so it is a conversation's job and this starts one on it.
            */}
          {task.change === null && onProposeChange !== undefined && (
            <button
              type="button"
              className={styles.action}
              data-audit="specProposeForTask"
              data-write="true"
              title="Runs OpenSpec's propose workflow in this project's Claude conversation, and asks it to link the change it writes back to this task."
              onClick={() => onProposeChange(task.id)}
            >
              Make a proposal
            </button>
          )}
          <span className={styles.actionsSpacer} />
          <DeleteControl
            task={task.id}
            label={task.title.trim() !== '' ? task.title : task.id}
            armed={deleteArmed}
            compact={false}
            onDeleteArm={onDeleteArm}
            onDelete={onDelete}
          />
        </div>

        {/*
          * The picker, under the row rather than inside it: it is a list, and a list inside a
          * flex row of buttons would either squash the row or scroll sideways.
          *
          * Each option says what choosing it *does*, because that is the whole of the choice — a
          * role runs unattended in its own worktree, a conversation already open is typed into.
          */}
        {dispatchOpen === true && dispatchTargets !== undefined && (
          <ul className={styles.targets} data-audit="taskDispatchTargets">
            {dispatchTargets.map((target) => (
              <li key={targetKey(target)}>
                <button
                  type="button"
                  className={styles.target}
                  data-audit="taskDispatchTarget"
                  data-target={targetKey(target)}
                  data-kind={target.kind}
                  data-write="true"
                  onClick={() => onDispatchTo?.(task.id, target)}
                >
                  <span className={styles.targetLabel}>{target.label}</span>
                  <span className={styles.targetDetail}>{target.detail}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
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
      <>
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
        {(props.assigneeHint ?? null) !== null && (
          <p className={styles.assigneeHint} data-audit="tasksAssigneeHint">
            {props.assigneeHint}
          </p>
        )}
      </>
    ),
    body: () => (
      <MentionTextarea
        id={id}
        className={styles.textarea}
        data-audit="taskEditor"
        data-field={field}
        data-write="true"
        roles={roles}
        listboxId={`task-body-mentions-${task.id}`}
        tools
        autoFocus
        aria-label={label}
        value={editing?.draft ?? ''}
        onValueChange={(draft) => props.onEditing?.({ field, draft })}
        onKeyDown={(event) => {
          // Enter is a newline in a body, so the shortcut is the modified one. Save is on
          // screen as well — a shortcut nobody can see is not a way out of a field. Plain
          // Enter with the mention popup open never reaches here — the popup claims it.
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
          {open ? 'Cancel' : <Icon name="pencil" size={1} />}
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
      ) : field === 'body' && !isFieldEmpty(task, field) ? (
        /*
         * The body at rest renders as markdown — the statement of the work is the one field
         * agents author in it. A `<div>` because the rendering is block elements, which a `<p>`
         * cannot legally hold; same hook and classes, so every digest keeps reading it.
         */
        <div
          className={cx(styles.fieldValue, styles.fieldValueBody)}
          data-audit="taskFieldValue"
          data-field={field}
        >
          <TaskMarkdown text={task.body} />
        </div>
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
