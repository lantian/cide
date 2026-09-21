/**
 * Everything that draws a **run** — the pieces a role row needs, and the whole row Recent uses.
 * (M18)
 *
 * ```
 * ● Developer · claude                        2m     ← RunRow, in Recent
 *   t-14 Add the retry bar        Open  ⏸  ⏹
 * ⚠ This turn may have timed out while paused.   Retry turn   Leave it
 *
 *   ◆ t-16 Sweep the phase table   1m   Open ⏸ ⏹    ← ActivityRow, under a role
 * ```
 *
 * # Two rows, because there are two questions
 *
 * [`RunRow`] is a run as a **record**: it leads with `agentLabel` — the role's name *copied at
 * dispatch* — because Recent answers "what happened this afternoon", and a row that re-derived
 * its name would rename itself when a config file changed and empty itself when a role was
 * deleted. That row must not depend on a definition still existing.
 *
 * [`ActivityRow`] is a run as **something happening now**, drawn under the role row that already
 * carries the name. So it leads with the phase dot and goes straight to the task, and the
 * duplicate identity line is gone. A single component with a `showAgent` flag was the
 * alternative and it is worse in the way that matters: the two differ in *which fact leads*,
 * which is the whole design of each, and a boolean hides that decision inside a component.
 *
 * What they share is drawn once, here: [`TaskLink`], [`RunControls`], [`RunNotes`] and
 * [`StaleBar`]. That is not tidiness — the Open/pause/resume/stop set is the panel's most
 * dangerous markup (one of them signals a child, one of them ends it), and two copies of it is
 * two answers to "which controls does this run offer".
 *
 * # Nothing here decides anything
 *
 * Every judgement arrives inside [`RunRowData`], decided in `model.ts`: the glyph, the label,
 * the tone, the task title, `canOpen`, `canPause`, the stale-turn sentence. A row that
 * re-derived `canOpen` from the phase would be a second answer to "is there a transcript to
 * attach a pane to", and the first time the two disagreed the user would get a pane with
 * nothing in it and no way to tell why.
 *
 * # A control that cannot work is not drawn
 *
 * A queued run renders **no Open element at all** — not a disabled one. `ProblemsPanel` makes
 * the same argument with its `rowTag`: a disabled control is a promise that some reachable
 * condition would make it work, and for a run with no session there is none; it has to be
 * dispatched first, and *that* control is on the role row. `ui/scripts/check-agents-render.mjs`
 * counts Open elements rather than checking a `disabled` attribute, precisely so a well-meaning
 * "grey it out instead" cannot pass. The role row above obeys the same rule one level up: with
 * no active run there is no `ActivityRow`, so there is no Open anywhere in it.
 *
 * The handler props are optional for the same reason and gate the same way: a host that cannot
 * open a pane must not be given a button that pretends it can.
 */
import {
  agentColor,
  glyphSpins,
  harnessLabel,
  isDonePhase,
  timeTitle,
  workedFor,
  type RunRow as RunRowData,
  type Tone,
} from './model'
import { Icon, asIcon } from '@/icons/Icon'

import styles from './AgentsPanel.module.css'

/**
 * One CSS class per `Tone`.
 *
 * `string | undefined` because Vite types a CSS module as `Record<string, string>` and this
 * project runs `noUncheckedIndexedAccess`; asserting the class away would be asserting that
 * the stylesheet still defines it, which is exactly what this file cannot promise. The gate
 * that can is the render check's `unclassed` counter.
 *
 * Exported because the **role** row draws a phase dot too — its summary, which is one of its
 * runs' glyph and tone — and a second table over there would be a second set of colours for one
 * vocabulary.
 */
export const TONE_CLASS: Record<Tone, string | undefined> = {
  idle: styles.toneIdle,
  busy: styles.toneBusy,
  attention: styles.toneAttention,
  paused: styles.tonePaused,
  done: styles.toneDone,
  error: styles.toneError,
}

/**
 * Join class names, dropping anything a missing stylesheet rule turned into `undefined`.
 *
 * A template literal is the trap this avoids: `` `${styles.row} ${maybe}` `` interpolates a
 * missing rule as the *string* `"undefined"`, which lands in the DOM as a real class token —
 * invisible to `tsc`, to `vite build`, and to an `unclassed` counter that only looks for an
 * empty attribute. The render check greps for that token as well.
 */
export function cx(...parts: Array<string | undefined | false>): string {
  return parts.filter((part): part is string => typeof part === 'string' && part !== '').join(' ')
}

export interface RunRowProps {
  /** Everything the row draws, already decided by `sections()`. */
  row: RunRowData
  /**
   * The clock, as a prop.
   *
   * A component that called `Date.now()` could not be digested by an SSR check — the elapsed
   * figure would change between two runs of the same fixture and the digest would be useless —
   * which is why `model.elapsed` takes the instant as a parameter and why every view in this
   * directory takes it as a prop rather than reading it.
   */
  nowMs: number
  /** Attach a pane to the run's transcript. Only offered when `row.canOpen`. */
  onOpen?: ((run: string) => void) | undefined
  /** SIGSTOP the child. Only offered when `row.canPause`. */
  onPause?: ((run: string) => void) | undefined
  /** SIGCONT it. Only offered while the run is paused. */
  onResume?: ((run: string) => void) | undefined
  /** End the run. Not offered once it has already ended. */
  onStop?: ((run: string) => void) | undefined
  /** Open the task this run is against — its card, over this panel; no view is switched. */
  onRevealTask?: ((task: string) => void) | undefined
  /** Re-send the turn the freeze may have killed. */
  onRetryTurn?: ((run: string) => void) | undefined
  /** Dismiss the stale-turn bar without re-sending anything. */
  onAckStaleTurn?: ((run: string) => void) | undefined
}

/**
 * One run of a role that is **doing something now**, drawn under that role's own line.
 *
 * No identity column: the role row directly above says whose run this is, and repeating
 * `agentLabel` under it would be the same word twice on two adjacent lines. What leads instead
 * is the phase dot, because with more than one run under a role the dots are the only thing that
 * says the two are in different states — the role's own summary describes the first of them.
 */
export function ActivityRow({
  row,
  nowMs,
  onOpen,
  onPause,
  onResume,
  onStop,
  onRevealTask,
  onRetryTurn,
  onAckStaleTurn,
}: RunRowProps) {
  const { run } = row
  return (
    <div
      className={styles.activity}
      data-audit="agentsRow"
      data-run={run.run}
      data-phase={run.phase}
    >
      <div className={styles.activityLine} data-audit="agentsRowTop">
        <span
          className={cx(
            styles.glyph,
            TONE_CLASS[row.tone],
            glyphSpins(row.glyph) && styles.glyphSpin,
          )}
          data-audit="agentsGlyph"
          data-tone={row.tone}
          aria-hidden="true"
        >
          <Icon name={asIcon(row.glyph)} size={1} />
        </span>
        <TaskLink row={row} onRevealTask={onRevealTask} />
        {/* The figure is **worked** time, not age since dispatch — see `workedFor`, which
            carries the three ways the old figure lied. `title` still leads with the phase in
            words, because the dot at the head of the line is the only other place this run's
            state is written and a dot is not readable by a screen reader or by somebody who has
            not learned the glyphs; `hint` outranks the word for the one phase whose word is not
            the whole truth — a paused child with a model call still finishing server-side; see
            `phaseHint`. `timeTitle` appends what the two clocks say, so the wall clock a paused
            run's row no longer draws is one hover away. */}
        <span
          className={styles.elapsed}
          data-audit="agentsElapsed"
          title={timeTitle(nowMs, run, row.hint ?? row.label)}
        >
          {workedFor(nowMs, run)}
        </span>
        <RunControls
          row={row}
          onOpen={onOpen}
          onPause={onPause}
          onResume={onResume}
          onStop={onStop}
        />
      </div>
      <RunNotes row={row} />
      <StaleBar row={row} onRetryTurn={onRetryTurn} onAckStaleTurn={onAckStaleTurn} />
    </div>
  )
}

/**
 * One run as a **record**, for Recent: who ran, on what, and how long it took.
 *
 * Leads with the label the run carried at dispatch, and needs no role definition to exist — see
 * the header. This is the row a finished run keeps after its role file has been deleted.
 */
export function RunRow({
  row,
  nowMs,
  onOpen,
  onPause,
  onResume,
  onStop,
  onRevealTask,
  onRetryTurn,
  onAckStaleTurn,
}: RunRowProps) {
  const { run } = row

  return (
    <div
      className={styles.row}
      data-audit="agentsRow"
      data-run={run.run}
      data-phase={run.phase}
    >
      <div className={styles.rowTop} data-audit="agentsRowTop">
        <span
          className={cx(
            styles.glyph,
            TONE_CLASS[row.tone],
            glyphSpins(row.glyph) && styles.glyphSpin,
          )}
          data-audit="agentsGlyph"
          data-tone={row.tone}
          aria-hidden="true"
        >
          <Icon name={asIcon(row.glyph)} size={1} />
        </span>
        {/* The label was copied at dispatch, never joined against the roster at render time:
            a row that re-derives its name renames itself when a config file changes and
            empties when a role is deleted, and a row that renames itself lies about what
            happened. That is what makes this row survivable for a role that no longer exists,
            which in Recent is an ordinary state rather than an edge case. */}
        <span
          className={styles.agentLabel}
          /*
           * Coloured off `run.agent` — the **id** — while the word drawn is `run.agentLabel`.
           * (M75) That is not an inconsistency: `agentColor`'s doc argues the id is the key
           * precisely so a row keeps its colour through a rename, which is the same promise the
           * frozen label above makes about the word. Both survive a deleted role, because
           * neither needs the roster.
           */
          style={{ color: agentColor(run.agent) }}
          data-audit="agentsRunLabel"
          title={row.hint ?? row.label}
        >
          {run.agentLabel}
        </span>
        <span className={styles.sep} aria-hidden="true">
          ·
        </span>
        <span className={styles.harness}>{harnessLabel(run.harness)}</span>
        {/* A record, so this figure is **final**: a terminal run has no open interval, and
            `workedFor` therefore answers the same thing however long ago it ended. The old
            `elapsed(nowMs, startedMs)` rose for ever on a row whose doc says "how long it
            took" — a run that took ninety seconds read `4h 00m` four hours later. */}
        <span
          className={styles.elapsed}
          data-audit="agentsElapsed"
          title={timeTitle(nowMs, run, row.hint ?? row.label)}
        >
          {workedFor(nowMs, run)}
        </span>
      </div>

      <div className={styles.rowBottom} data-audit="agentsRowBottom">
        <TaskLink row={row} onRevealTask={onRevealTask} />
        <RunControls
          row={row}
          onOpen={onOpen}
          onPause={onPause}
          onResume={onResume}
          onStop={onStop}
        />
      </div>

      <RunNotes row={row} />
      <StaleBar row={row} onRetryTurn={onRetryTurn} onAckStaleTurn={onAckStaleTurn} />
    </div>
  )
}

/**
 * The agent→task link, or the fact that there is not one.
 *
 * `no task` is drawn rather than left blank because "this run is unaccounted for" is worth
 * seeing: an agent burning tokens against nothing on the board is the state a user most wants to
 * catch, and an empty line reads as a rendering gap.
 *
 * `taskTitle` is `null` both when the run has no task and when the board has not been read, so a
 * run *with* a task id still gets its id drawn — the id is the fact, the title is the decoration.
 */
function TaskLink({
  row,
  onRevealTask,
}: {
  row: RunRowData
  onRevealTask?: ((task: string) => void) | undefined
}) {
  /* Lifted out so the click handler closes over a `string` rather than over a `string | null`
     whose narrowing TypeScript discards at the callback boundary. */
  const task = row.run.task
  if (task === null) {
    return (
      <span className={styles.noTask} data-audit="agentsNoTask">
        no task
      </span>
    )
  }
  if (onRevealTask === undefined) {
    return (
      <span className={styles.taskLink} data-audit="agentsTaskLink" data-task={task}>
        <span className={styles.taskId}>{task}</span>
        <span className={styles.taskTitle}>{row.taskTitle ?? 'Untitled task'}</span>
      </span>
    )
  }
  return (
    <button
      type="button"
      className={styles.taskLink}
      data-audit="agentsTaskLink"
      data-task={task}
      onClick={() => onRevealTask(task)}
      title={`Open ${task}`}
    >
      <span className={styles.taskId}>{task}</span>
      <span className={styles.taskTitle}>{row.taskTitle ?? 'Untitled task'}</span>
    </button>
  )
}

/**
 * Open, pause, resume, stop — **one copy, for both rows**.
 *
 * Every gate is a field of `row` that `model.ts` decided, except the two read off the phase
 * vocabulary directly: `canPause`'s own doc states the complement — "not `paused`, the row draws
 * Resume there instead" — and `isDonePhase` is the predicate Recent is built from, so "still
 * running in some sense" is exactly its negation.
 */
function RunControls({
  row,
  onOpen,
  onPause,
  onResume,
  onStop,
}: {
  row: RunRowData
  onOpen?: ((run: string) => void) | undefined
  onPause?: ((run: string) => void) | undefined
  onResume?: ((run: string) => void) | undefined
  onStop?: ((run: string) => void) | undefined
}) {
  const { run } = row
  // `interrupted` resumes too: the child died with a cide restart, and Resume requeues the run
  // so a new child of the *registry's* continues the same conversation. It is no longer the
  // row's only affordance — Open puts the real harness on that conversation for a person
  // instead (M42), and the registry refuses to continue a run while such a pane is open.
  const canResume = run.phase === 'paused' || run.phase === 'interrupted'
  const canStop = !isDonePhase(run.phase)

  return (
    <span className={styles.controls} data-audit="agentsControls">
      {/* Withheld, not disabled, for a queued run: `canOpen` is false exactly when there
          is nothing to show — no session to mirror and no conversation to re-open — and
          nothing the user could do on this row would change that. */}
      {row.canOpen && onOpen !== undefined && (
        <button
          type="button"
          className={styles.control}
          data-audit="agentsOpen"
          onClick={() => onOpen(run.run)}
          title="Open this run's conversation in a pane: the live session while its child works, or the real harness re-opened on it once the child has ended. A live run keeps going either way."
        >
          Open
        </button>
      )}
      {row.canPause && onPause !== undefined && (
        <button
          type="button"
          className={cx(styles.control, styles.controlGlyph)}
          data-audit="agentsPause"
          onClick={() => onPause(run.run)}
          title="Freeze this run. It keeps its worktree and its place in the queue."
          aria-label="Pause this run"
        >
          <Icon name="pause" size={1} />
        </button>
      )}
      {canResume && onResume !== undefined && (
        <button
          type="button"
          className={cx(styles.control, styles.controlGlyph)}
          data-audit="agentsResume"
          onClick={() => onResume(run.run)}
          title="Continue this run — on the current model settings, restarting it on a new child if they changed."
          aria-label="Resume this run"
        >
          <Icon name="play" size={1} />
        </button>
      )}
      {canStop && onStop !== undefined && (
        <button
          type="button"
          className={cx(styles.control, styles.controlGlyph, styles.controlStop)}
          data-audit="agentsStop"
          onClick={() => onStop(run.run)}
          title="End this run. Its transcript stays readable in Recent."
          aria-label="Stop this run"
        >
          <Icon name="square" size={1} />
        </button>
      )}
    </span>
  )
}

/**
 * The backend's own lines about this run — why it failed, which worktree it holds, what the
 * queue is waiting on. Rendered as text; they are sentences, not markup.
 */
function RunNotes({ row }: { row: RunRowData }) {
  const { run } = row
  return (
    <>
      {run.failure !== null && (
        <p className={cx(styles.rowNote, styles.rowFailure)} data-audit="agentsFailure">
          {run.failure}
        </p>
      )}
      {run.note !== null && (
        <p className={styles.rowNote} data-audit="agentsNote">
          {run.note}
        </p>
      )}
    </>
  )
}

/**
 * The stale-turn bar. Exactly two controls, and both are needed.
 *
 * cide cannot see the model request — only a process it stopped and continued — so this is a
 * suspicion phrased as one. Re-dispatching automatically would double-bill a turn that in fact
 * survived; dropping the question would leave a run that is quietly dead. It lives here rather
 * than in `chrome/notices.ts` because that module carries `text`/`hint`/`detail` and no action
 * affordance, and because the decision has to stay attached to the run it is about.
 */
function StaleBar({
  row,
  onRetryTurn,
  onAckStaleTurn,
}: {
  row: RunRowData
  onRetryTurn?: ((run: string) => void) | undefined
  onAckStaleTurn?: ((run: string) => void) | undefined
}) {
  if (row.staleTurn === null) return null
  return (
    <div className={styles.staleBar} data-audit="agentsStaleBar">
      <span className={styles.staleText}>
        <Icon name="triangle-alert" size={0} />
        {row.staleTurn}
      </span>
      {onRetryTurn !== undefined && (
        <button
          type="button"
          className={styles.staleAction}
          data-audit="agentsStaleAction"
          onClick={() => onRetryTurn(row.run.run)}
        >
          Retry turn
        </button>
      )}
      {onAckStaleTurn !== undefined && (
        <button
          type="button"
          className={styles.staleAction}
          data-audit="agentsStaleAction"
          onClick={() => onAckStaleTurn(row.run.run)}
        >
          Leave it
        </button>
      )}
    </div>
  )
}
